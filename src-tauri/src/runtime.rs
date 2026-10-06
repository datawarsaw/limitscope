//! Shared provider runtime — the single owner of provider refresh cycles.
//!
//! v0.5 "More Rust Behind the UI" step 1. Rust owns: cycle orchestration,
//! provider fetch invocation (the existing `get_*_usage` functions, called
//! in-process), per-cycle coalescing, one bounded transient retry, last-good
//! retention, the current normalized snapshot, and the refresh interval
//! timer. Both webviews (main dashboard, floating quota bar) are consumers:
//! they pull a snapshot once on attach and receive a full snapshot after
//! every completed cycle — no diff protocol, no per-window polling.
//!
//! Wire parity: the snapshot's provider entries serialize to exactly the
//! `ProviderUsage` shape the frontend already knows (camelCase, optional
//! fields omitted). The behavioral rules moved here from the retired TS
//! adapters are preserved verbatim:
//!
//! - transient retry: at most one retry, 750 ms + 0–250 ms jitter, only when
//!   the structured error says transient (explicit verdict, or legacy
//!   `network` code / 429–5xx status). Antigravity never retries (its
//!   passive backend has no retry wrapper today). No provider burns the fast
//!   retry when the server sent a `Retry-After` hint — the hint is turned
//!   into a provider cooldown instead. A transport timeout is likewise
//!   terminal for the current cycle: the stall that consumed the whole
//!   request budget will not clear 750 ms later, so the next
//!   cadence/scheduler cycle serves as the retry instead of doubling the
//!   worst-case cycle tail. Fast-failing transport errors (refused
//!   connections, DNS misses) keep the one bounded retry.
//! - server cooldown (v0.5 resilience): a 429/5xx failure carrying a
//!   parseable standard `Retry-After` header (delta-seconds or HTTP-date,
//!   one shared parser in `provider_error.rs`) puts the provider on an
//!   ephemeral cooldown — `now + min(hint, 24 h)`. While it lasts, every
//!   refresh source (scheduler, manual, wake, reconnect) skips the provider
//!   entirely; the cooldown is never persisted and never recorded as
//!   history. 5xx without a hint keeps the plain one-retry rule. Provider
//!   cadence (e.g. Grok's 15 minutes) and cooldown are separate gates: the
//!   next eligible fetch is the later of the two.
//! - last-good: a failed refresh keeps the provider's last good usage with
//!   `status: "error"` and `error: "Refresh failed: <message>"`; the
//!   retained `checkedAt` stays at the original fetch time. Without last
//!   good data the provider surfaces as a bare error entry.
//! - Grok cadence: the billing endpoint is polled at most once per 15
//!   minutes; within the window the cached last result (success or failure)
//!   is served with its original `checkedAt`. Local credential failures are
//!   never cached, so a re-auth takes effect on the next poll.
//! - account attribution: OpenCode Go (`key:XXXX` from the 4-char key hint)
//!   and Grok (`xai:<id>`) build the same masked, storage-safe identities
//!   the TS adapters built.
//! - Antigravity freshness: a "fresh" verdict is trusted only together with
//!   a parseable `sourceUpdatedAt`; everything else degrades to stale.
//!
//! Coalescing semantics (formerly `RefreshCoordinator`): at most one cycle
//! runs at a time. A request landing mid-cycle sets a single pending
//! follow-up flag — never a second parallel cycle, never an unbounded queue;
//! exactly one follow-up cycle runs afterwards. The frontend's loading
//! projection spans the whole chain (`runtime://cycle-started` fires once at
//! chain start, a snapshot closes it).
//!
//! History (v0.5 phase 2) is owned by `history.rs`: the runtime records one
//! observation batch per completed cycle from the same normalized snapshot
//! it broadcasts, so any number of consumer windows produce exactly one
//! history stream. Prediction and account-aware filtering stay in TS, which
//! consumes history read results only.
//!
//! Normalized health (v0.6): every provider entry carries one explicit
//! `health` value ([`ProviderHealth`]) computed here — live, stale, unknown,
//! cooldown, error, unavailable. The frontend reads that field instead of
//! reconstructing health from status/error/freshness; the legacy `status`
//! string stays on the wire but is derived from `health`, never independent.
//! The full contract (definitions, transitions, history/notification
//! eligibility) is documented in `docs/runtime-status-contract.md`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Serialize, Serializer};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use crate::antigravity::{self, AntigravityError, AntigravityUsage, DataSourceFreshness};
use crate::history::{QuotaHistoryStore, QuotaObservation};
use crate::last_good::ProviderLastGoodStore;
use crate::codex::{self, CodexResetCredits, CodexUsage};
use crate::diagnostics::{
    ErrorDiagnosticSource, ProviderDiagnosticSource, RuntimeDiagnosticSource,
};
use crate::grok::{self, GrokError, GrokUsage};
use crate::notifications::NotificationLane;
use crate::opencode_go::{self, OpenCodeGoUsage};
use crate::provider_error::{ProviderError, MAX_COOLDOWN_MS};
use crate::zai::{self, ZaiUsage};
use crate::zcode_plans::{self, ZCodePlansObservation};
use crate::zcode_reset::{self, ZCodeResetStatus};

/// Frontend event carrying a full runtime snapshot after every cycle.
pub const SNAPSHOT_EVENT: &str = "runtime://snapshot";
/// Frontend event fired once when a refresh chain starts (loading projection).
pub const CYCLE_STARTED_EVENT: &str = "runtime://cycle-started";

/// Default refresh interval; matches the TS settings default. A window
/// pushes the persisted interval on attach, so this only covers the gap
/// before the first attach.
const DEFAULT_INTERVAL_MINUTES: u64 = 5;

// ---------- wire DTOs (serialize to the current frontend shapes) ----------

/// Normalized provider health — the single explicit state the frontend reads.
/// Computed by the runtime; consumers never reconstruct it from
/// status/error/freshness fragments.
///
/// - [`ProviderHealth::Live`]: the current fetch succeeded, the data belongs
///   to the provider's current account, it passed provider normalization,
///   and it is within freshness policy. Never a "last-known" value.
/// - [`ProviderHealth::Stale`]: previously valid data retained for display
///   that freshness policy no longer considers current (today: Antigravity's
///   own source-snapshot verdict; tomorrow: a hydrated persisted last-good —
///   a persisted snapshot maps to stale, never live).
/// - [`ProviderHealth::Unknown`]: the provider answered successfully but
///   reported no usable quota windows (empty limits, or Grok's zero-window
///   success with retained data shown).
/// - [`ProviderHealth::Cooldown`]: a server `Retry-After` cooldown is active,
///   so the provider is intentionally not fetched. Retained last-good may
///   still be shown, but the health is cooldown, not error.
/// - [`ProviderHealth::Error`]: the current refresh attempt failed. With
///   retained last-good the data is still shown — the health stays error.
/// - [`ProviderHealth::Unavailable`]: nothing usable can be shown and the
///   provider cannot currently be evaluated because the required local
///   source/auth state is absent (no credential, expired auth, missing
///   cache). Ordinary transient errors never land here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHealth {
    Live,
    Stale,
    Unknown,
    Cooldown,
    Error,
    Unavailable,
}

impl ProviderHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderHealth::Live => "live",
            ProviderHealth::Stale => "stale",
            ProviderHealth::Unknown => "unknown",
            ProviderHealth::Cooldown => "cooldown",
            ProviderHealth::Error => "error",
            ProviderHealth::Unavailable => "unavailable",
        }
    }

    /// The legacy `status` vocabulary the current frontend type knows
    /// (`"ok" | "stale" | "error" | "unknown"`). Strictly derived from the
    /// health so the two fields can never disagree: cooldown, error, and
    /// unavailable all surface as the legacy `"error"` (there is no finer
    /// legacy word), live as `"ok"`.
    pub fn legacy_status(self) -> &'static str {
        match self {
            ProviderHealth::Live => "ok",
            ProviderHealth::Stale => "stale",
            ProviderHealth::Unknown => "unknown",
            ProviderHealth::Cooldown | ProviderHealth::Error | ProviderHealth::Unavailable => {
                "error"
            }
        }
    }
}

impl Serialize for ProviderHealth {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Keeps `status` and `health` in sync at the single write point.
fn set_health(dto: &mut ProviderUsageDto, health: ProviderHealth) {
    dto.health = health;
    dto.status = health.legacy_status();
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitDto {
    pub label: String,
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountAttributionDto {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

/// One provider's entry in the snapshot — the exact `ProviderUsage` wire
/// shape from `src/types.ts` (camelCase, optional fields omitted when None).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageDto {
    pub id: String,
    pub name: String,
    /// Legacy vocabulary `"ok" | "stale" | "error" | "unknown"`, derived
    /// from `health` (see [`ProviderHealth::legacy_status`]). Kept so the
    /// existing wire shape is unchanged; read `health` instead.
    pub status: &'static str,
    /// Normalized provider health (v0.6 contract) — always present, always
    /// consistent with `status`.
    pub health: ProviderHealth,
    pub checked_at: String,
    pub limits: Vec<UsageLimitDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountAttributionDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Stable failure category (the structured error's `code` vocabulary)
    /// when the entry carries a failure; never raw provider response text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    /// HTTP status of the failed refresh, when the failure came from a
    /// non-success response. A bare number — no headers, no body text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_freshness: Option<&'static str>,
    /// Optional Codex-only banked reset-credit observation (v0.7). Present
    /// only on a fresh, account-bound observation; never derived from
    /// windows, never hydrated from disk, stripped once stale. Providers
    /// other than Codex never set this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<CodexResetCredits>,
    /// Optional ZCode reset-card observation. ZCode reset cards are specific
    /// grants (a named window plus an expiry), deliberately distinct from the
    /// Codex banked-credit balance. Present only on the Z.ai entry when the
    /// passive reset-status observation answered; observed by the ordinary
    /// runtime cycle, never hydrated from disk, stripped once stale. A failed
    /// observation never fails the Z.ai quota refresh. Providers other than
    /// Z.ai never set this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zcode_reset_cards: Option<ZCodeResetStatus>,
    /// Optional ZCode plan/balance observation. Active ZCode plans/packages
    /// (e.g. Start Plan / promotional Trust Build packages) with their own
    /// plan-grouped ABSOLUTE balances — a deliberately different shape from
    /// the monitor endpoint's percentage-only windows, never merged into
    /// them, and never summed across plans or units. Present only on the
    /// Z.ai entry when the passive balance observation answered; observed by
    /// the ordinary runtime cycle, never hydrated from disk, stripped once
    /// stale. A failed observation never fails the Z.ai quota refresh.
    /// Providers other than Z.ai never set this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zcode_plans: Option<ZCodePlansObservation>,
    /// Internal, never serialized (`serde(skip)`): the Antigravity adapter's
    /// structured live-failure cause when a refresh degraded to cached
    /// windows. The success path records it in the diagnostics lane's
    /// last-failure map so the export bundle can explain why cached data was
    /// used; the entry's wire shape, health, and presentation are untouched.
    /// Other providers never set it.
    #[serde(skip)]
    pub fallback_failure: Option<ProviderFailure>,
}

/// Full runtime state pulled by `get_runtime_snapshot` and pushed after
/// every completed cycle. `seq` is a monotonic snapshot counter: consumers
/// apply a snapshot only when its `seq` is not older than the newest one
/// they applied, which makes the pull-after-subscribe race harmless.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub seq: u64,
    pub providers: Vec<ProviderUsageDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
    /// Whether the last completed cycle produced at least one usable source
    /// (the frontend's `refreshCycleSucceeded` verdict).
    pub cycle_succeeded: bool,
    pub cycle_in_flight: bool,
    pub refresh_interval_minutes: u64,
    /// Monotonic history revision, bumped whenever the Rust-owned quota
    /// history actually changed. Consumers re-pull `get_history` only when
    /// this differs from the revision they last pulled, so the history file
    /// is never broadcast on unchanged cycles.
    pub history_revision: u64,
}

// ---------- unified provider failure ----------

/// The failure information the retry rule and the error rendering need,
/// unified across the three backend error shapes (`ProviderError`,
/// `GrokError`, `AntigravityError`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderFailure {
    pub code: String,
    pub message: String,
    pub http_status: Option<u16>,
    pub transient: Option<bool>,
    pub retry_after_ms: Option<u64>,
    /// Masked identity of the credential the failed refresh attempted, when
    /// the backend could resolve it before failing. The last-good guard
    /// compares it against the retained snapshot's attribution: last-good
    /// data may only be retained for the same account.
    pub identity: Option<String>,
    /// The failure was a transport timeout: the client's request budget
    /// expired during connect, TLS, response, or body read. Input to the
    /// bounded-retry rule only — never serialized, never part of the wire.
    pub transport_timeout: bool,
}

impl ProviderFailure {
    /// Mirrors the TS adapters' `describeError`: message (with code) wins,
    /// then the provider's fallback line (with code). The wording matches
    /// the retired TS adapters exactly.
    fn describe(&self, fallback: &str) -> String {
        if !self.message.is_empty() {
            if self.code.is_empty() {
                self.message.clone()
            } else {
                format!("{} ({})", self.message, self.code)
            }
        } else if !self.code.is_empty() {
            format!("{} ({})", fallback, self.code)
        } else {
            fallback.to_string()
        }
    }

    /// Mirrors `isTransientCommandError`: an explicit structured verdict
    /// always wins; without one, fall back to the `network` code and 429/5xx
    /// statuses; anything else fails safely as non-transient.
    fn is_transient(&self) -> bool {
        if let Some(verdict) = self.transient {
            return verdict;
        }
        if self.code == "network" {
            return true;
        }
        match self.http_status {
            Some(status) => status == 429 || (500..=599).contains(&status),
            None => false,
        }
    }
}

fn failure_from_provider_error(error: ProviderError) -> ProviderFailure {
    ProviderFailure {
        code: error.code,
        message: error.message,
        http_status: error.http_status,
        transient: error.transient,
        retry_after_ms: error.retry_after_ms,
        identity: error.identity_hint,
        transport_timeout: error.transport_timeout,
    }
}

fn failure_from_grok_error(error: GrokError) -> ProviderFailure {
    ProviderFailure {
        code: error.code,
        message: error.message,
        http_status: error.http_status,
        transient: error.transient,
        retry_after_ms: error.retry_after_ms,
        identity: error.identity_hint,
        transport_timeout: error.transport_timeout,
    }
}

fn failure_from_antigravity_error(error: AntigravityError) -> ProviderFailure {
    ProviderFailure {
        code: error.code,
        message: error.message,
        http_status: None,
        transient: None,
        retry_after_ms: None,
        identity: None,
        transport_timeout: false,
    }
}

/// Failure codes meaning "the provider cannot currently be evaluated because
/// the required local source/auth state is absent" — the [`ProviderHealth::
/// Unavailable`] family. Everything else (transport, HTTP, schema,
/// entitlement verdicts) is an ordinary [`ProviderHealth::Error`].
///
/// The set is the union of the backends' deterministic local-state codes:
/// missing/expired/unreadable credentials, rejected auth, missing login
/// state, missing local cache sources, and the `*_not_installed` family.
/// Codes are never renamed for uniformity (see `provider_error.rs`), so the
/// list is exhaustive over the current vocabulary and suffix-matches the
/// provider-prefixed `*_not_installed` codes.
const SOURCE_ABSENT_CODES: [&str; 13] = [
    "auth_expired",
    "auth_invalid",
    "auth_unreadable",
    "auth_file_missing",
    "auth_failed",
    "not_logged_in",
    "credential_missing",
    "credential_expired",
    "credential_ambiguous",
    "opencodex_config_unreadable",
    "cache_missing",
    "no_account",
    "quota_missing",
];

fn failure_is_source_absent(code: &str) -> bool {
    SOURCE_ABSENT_CODES.contains(&code) || code.ends_with("_not_installed")
}

/// The canonical health a bare (non-retained) failure projection carries —
/// the same mapping `apply_failure_at` uses, exposed for the redacted
/// diagnostics report so the operator-facing output can never drift from the
/// runtime's health contract.
pub(crate) fn failure_health(failure: &ProviderFailure) -> ProviderHealth {
    if failure_is_source_absent(&failure.code) {
        ProviderHealth::Unavailable
    } else {
        ProviderHealth::Error
    }
}

// ---------- provider kinds, specs, and normalization ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Codex,
    Zai,
    OpenCodeGo,
    Antigravity,
    Grok,
}

impl ProviderKind {
    pub(crate) fn id(self) -> &'static str {
        match self {
            ProviderKind::Codex => "openai-codex",
            ProviderKind::Zai => "zai",
            ProviderKind::OpenCodeGo => "opencode-go",
            ProviderKind::Antigravity => "antigravity",
            ProviderKind::Grok => "grok",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            ProviderKind::Codex => "OpenAI / Codex",
            ProviderKind::Zai => "Z.ai",
            ProviderKind::OpenCodeGo => "OpenCode Go",
            ProviderKind::Antigravity => "Google Antigravity",
            ProviderKind::Grok => "Grok (xAI)",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        match id {
            "openai-codex" => Some(ProviderKind::Codex),
            "zai" => Some(ProviderKind::Zai),
            "opencode-go" => Some(ProviderKind::OpenCodeGo),
            "antigravity" => Some(ProviderKind::Antigravity),
            "grok" => Some(ProviderKind::Grok),
            _ => None,
        }
    }

    fn fallback_error(self) -> &'static str {
        match self {
            ProviderKind::Codex => "OpenAI / Codex usage could not be fetched.",
            ProviderKind::Zai => "Z.ai usage could not be fetched.",
            ProviderKind::OpenCodeGo => "OpenCode Go usage could not be fetched.",
            ProviderKind::Antigravity => "Google Antigravity usage could not be fetched.",
            ProviderKind::Grok => "Grok usage could not be fetched.",
        }
    }
}

pub type FetchFuture = Pin<Box<dyn Future<Output = Result<ProviderUsageDto, ProviderFailure>> + Send>>;
pub type FetchFn = Arc<dyn Fn() -> FetchFuture + Send + Sync>;

/// One provider as the runtime sees it: its kind plus a fetch closure that
/// invokes the existing backend fetch and normalizes the payload. Tests
/// inject mock closures; production installs closures around the real
/// `get_*_usage` functions — parsers are never rewritten here.
#[derive(Clone)]
pub struct ProviderSpec {
    pub kind: ProviderKind,
    pub fetch: FetchFn,
}

// ---------- runtime panic containment ----------

/// The structured failure a contained provider-job panic becomes. A panic is
/// a local defect, never a transient condition: no retry burn, no cooldown,
/// and no resolved identity (last-good retention is therefore unchanged).
/// The `unexpected` code is the backends' established catch-all — no
/// parallel error vocabulary.
///
/// The panic payload is deliberately not forwarded. This message is the one
/// free-form string that reaches the WebView verbatim, and a payload is not
/// guaranteed display-safe: std's own slice/expect panics interpolate the
/// offending value (for example a malformed credential string) into the
/// message, which would bypass the adapters' exact-value scrubbing. The
/// detail stays on stderr via the default panic hook in dev runs instead.
fn failure_from_panic() -> ProviderFailure {
    ProviderFailure {
        code: "unexpected".to_string(),
        message: "provider fetch panicked (payload withheld)".to_string(),
        http_status: None,
        transient: Some(false),
        retry_after_ms: None,
        identity: None,
        transport_timeout: false,
    }
}

/// The fallback failure for a job task that died without a result when the
/// lost-task panic count is ambiguous (several jobs lost in the same cycle):
/// still the ordinary structured failure, still no disappearance.
fn job_lost_failure() -> ProviderFailure {
    ProviderFailure {
        code: "unexpected".to_string(),
        message: "provider task ended without a result".to_string(),
        http_status: None,
        transient: Some(false),
        retry_after_ms: None,
        identity: None,
        transport_timeout: false,
    }
}

/// Containment boundary for one provider fetch call: the closure call and
/// every poll of its future run inside `catch_unwind`, so a panicking
/// provider job becomes the ordinary structured failure instead of
/// unwinding the whole job task. `FetchFuture` is `Pin<Box<..>>` (Unpin),
/// so the wrapper needs no pin projection.
struct FetchPanicBoundary {
    inner: FetchFuture,
}

impl Future for FetchPanicBoundary {
    type Output = Result<ProviderUsageDto, ProviderFailure>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let inner = Pin::new(&mut this.inner);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.poll(cx))) {
            Ok(poll) => poll,
            Err(_) => Poll::Ready(Err(failure_from_panic())),
        }
    }
}

/// One fetch call behind the panic boundary ([`FetchPanicBoundary`]): both
/// the closure call itself and every poll of its future are contained, so a
/// panicking fetch is indistinguishable from any other failed refresh
/// downstream.
async fn fetch_contained(spec: &ProviderSpec) -> Result<ProviderUsageDto, ProviderFailure> {
    let fetch = Arc::clone(&spec.fetch);
    FetchPanicBoundary {
        inner: Box::pin(async move { (fetch)().await }),
    }
    .await
}

fn clamp_percent(value: f64) -> f64 {
    // The Rust backends already clamp; repeated defensively at the boundary,
    // exactly like the TS adapters did.
    value.clamp(0.0, 100.0)
}

/// The single normalization chokepoint: every provider's windows pass here
/// to become `UsageLimitDto`s. The reset plausibility gate applies per
/// window, per account — an implausible announced bound is dropped, not
/// trusted: the window keeps its data with `reset_at: None`, exactly the
/// Z.ai epoch-bounds degradation (`zai::epoch_ms_to_rfc3339`), generalized
/// to every provider. A rejected bound on one window never affects its
/// siblings.
fn limits_from(
    limits: &[(String, f64, Option<String>)],
    now_ms: i64,
) -> Vec<UsageLimitDto> {
    limits
        .iter()
        .map(|(label, used, reset)| UsageLimitDto {
            label: label.clone(),
            used_percent: clamp_percent(*used),
            reset_at: reset
                .as_deref()
                .filter(|reset| {
                    crate::reset_plausibility::plausible_reset_at(label, reset, now_ms)
                })
                .map(str::to_string),
        })
        .collect()
}

fn base_usage(
    kind: ProviderKind,
    health: ProviderHealth,
    limits: Vec<UsageLimitDto>,
) -> ProviderUsageDto {
    ProviderUsageDto {
        id: kind.id().to_string(),
        name: kind.name().to_string(),
        status: health.legacy_status(),
        health,
        // filled by the caller with the runtime clock
        checked_at: String::new(),
        limits,
        account: None,
        plan_type: None,
        error: None,
        error_category: None,
        error_http_status: None,
        source_updated_at: None,
        data_freshness: None,
        reset_credits: None,
        zcode_reset_cards: None,
        zcode_plans: None,
        fallback_failure: None,
    }
}

fn normalize_codex(
    result: Result<CodexUsage, ProviderError>,
    now_ms: i64,
) -> Result<ProviderUsageDto, ProviderFailure> {
    match result {
        Ok(usage) => {
            let limits = limits_from(
                &usage
                    .limits
                    .iter()
                    .map(|l| (l.label.clone(), l.used_percent, l.reset_at.clone()))
                    .collect::<Vec<_>>(),
                now_ms,
            );
            let health = if limits.is_empty() {
                ProviderHealth::Unknown
            } else {
                ProviderHealth::Live
            };
            // The credit capability rides the same account-bound observation
            // as the windows: quota Live with credits unavailable is the
            // normal independent-failure projection, never a quota error.
            let mut dto = base_usage(ProviderKind::Codex, health, limits);
            dto.account = build_codex_attribution(usage.account.as_ref());
            dto.plan_type = usage.plan_type.and_then(|p| {
                let trimmed = p.trim().to_lowercase();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            });
            dto.reset_credits = usage.reset_credits;
            Ok(dto)
        }
        Err(error) => Err(failure_from_provider_error(error)),
    }
}

/// Codex attribution: the masked tail of the `tokens.account_id` the request
/// was scoped with — the single local Codex session is the account. The
/// identity format matches what the backend stamps on post-resolution
/// failures, so the last-good guard compares like-for-like. The same bound
/// account carries the banked-credit observation of the same fetch.
fn build_codex_attribution(account: Option<&codex::CodexAccount>) -> Option<AccountAttributionDto> {
    let hint = account?.account_hint.trim();
    if hint.is_empty() {
        return None;
    }
    Some(AccountAttributionDto {
        label: format!("account \u{2022}\u{2022}{hint}"),
        note: Some(
            "Only the single local Codex login (~/.codex/auth.json) is shown; other ChatGPT accounts are not included."
                .to_string(),
        ),
        identity: Some(format!("chatgpt:{hint}")),
    })
}

/// Reset-credit freshness guard (v0.7, Codex only): a retained snapshot may
/// carry its banked-credit observation forward only while the observation is
/// within its TTL. A stale count is dropped so it can never read as a
/// current balance; the quota windows themselves are unaffected.
fn trim_stale_reset_credits(dto: &mut ProviderUsageDto, now: DateTime<Utc>) {
    if dto.id != ProviderKind::Codex.id() {
        return;
    }
    let fresh = dto
        .reset_credits
        .as_ref()
        .is_some_and(|credits| codex::reset_credits_fresh(&credits.checked_at, now));
    if !fresh {
        dto.reset_credits = None;
    }
}

/// Reset-card freshness guard (Z.ai only): a retained snapshot may carry its
/// reset-card observation forward only while the observation is within its
/// TTL (`zcode_reset::OBSERVATION_TTL_SECS`, mirroring the Codex credit
/// budget). A stale observation is dropped so spent cards can never read as
/// available; the quota windows themselves are unaffected.
fn trim_stale_zcode_reset_cards(dto: &mut ProviderUsageDto, now: DateTime<Utc>) {
    if dto.id != ProviderKind::Zai.id() {
        return;
    }
    let fresh = dto
        .zcode_reset_cards
        .as_ref()
        .is_some_and(|cards| zcode_reset::cards_fresh(&cards.observed_at, now));
    if !fresh {
        dto.zcode_reset_cards = None;
    }
}

/// Plan-observation freshness guard (Z.ai only): a retained snapshot may
/// carry its plan/balance observation forward only while the observation is
/// within its TTL (`zcode_plans::OBSERVATION_TTL_SECS`, mirroring the
/// reset-card budget). A stale observation is dropped so a consumed bucket
/// can never read as available; the quota windows themselves are unaffected.
fn trim_stale_zcode_plans(dto: &mut ProviderUsageDto, now: DateTime<Utc>) {
    if dto.id != ProviderKind::Zai.id() {
        return;
    }
    let fresh = dto
        .zcode_plans
        .as_ref()
        .is_some_and(|plans| zcode_plans::plans_fresh(&plans.observed_at, now));
    if !fresh {
        dto.zcode_plans = None;
    }
}

/// `reset_cards` and `plans` arrive as independent `Option`s: a failed
/// supplemental observation (`None`) must never fail the Z.ai quota refresh,
/// and a failed quota refresh propagates the quota failure while any fresh
/// supplemental data is simply dropped for the cycle (the retained last-good
/// entry keeps serving its still-fresh copy).
fn normalize_zai(
    result: Result<ZaiUsage, ProviderError>,
    reset_cards: Option<ZCodeResetStatus>,
    plans: Option<ZCodePlansObservation>,
    now_ms: i64,
) -> Result<ProviderUsageDto, ProviderFailure> {
    match result {
        Ok(usage) => {
            let limits = limits_from(
                &usage
                    .limits
                    .iter()
                    .map(|l| (l.label.clone(), l.used_percent, l.reset_at.clone()))
                    .collect::<Vec<_>>(),
                now_ms,
            );
            let health = if limits.is_empty() {
                ProviderHealth::Unknown
            } else {
                ProviderHealth::Live
            };
            let mut dto = base_usage(ProviderKind::Zai, health, limits);
            dto.account = build_zai_attribution(usage.account.as_ref());
            dto.zcode_reset_cards = reset_cards;
            dto.zcode_plans = plans;
            Ok(dto)
        }
        Err(error) => Err(failure_from_provider_error(error)),
    }
}

/// Z.ai attribution: the masked tail of the credential that answered —
/// several stored candidates may be attempted per refresh, and the card (and
/// the identity) names the one actually used, closing the winning-key
/// invisibility gap without exposing any key material.
fn build_zai_attribution(account: Option<&zai::ZaiAccount>) -> Option<AccountAttributionDto> {
    let hint = account?.key_hint.trim();
    if hint.is_empty() {
        return None;
    }
    Some(AccountAttributionDto {
        label: format!("key \u{2022}\u{2022}{hint}"),
        note: Some(
            "Attributed to the Z.ai credential that answered; stored candidates are tried in order."
                .to_string(),
        ),
        identity: Some(format!("key:{hint}")),
    })
}

/// OpenCode Go attribution (MIC-297): exactly four hint characters build the
/// masked identity; anything else stays unattributed.
fn build_opencode_attribution(key_hint: Option<&str>) -> Option<AccountAttributionDto> {
    let hint = key_hint?.trim();
    if hint.chars().count() != 4 {
        return None;
    }
    Some(AccountAttributionDto {
        label: format!("key ••{hint}"),
        note: Some(
            "Only the account whose OpenCode Go key is stored locally is shown; other OpenCode accounts are not included."
                .to_string(),
        ),
        identity: Some(format!("key:{hint}")),
    })
}

/// `pub(crate)` so the test-only diagnostics report can build rows through
/// the real normalizer.
pub(crate) fn normalize_opencode_go(
    result: Result<OpenCodeGoUsage, ProviderError>,
    now_ms: i64,
) -> Result<ProviderUsageDto, ProviderFailure> {
    match result {
        Ok(usage) => {
            let limits = limits_from(
                &usage
                    .limits
                    .iter()
                    .map(|l| (l.label.clone(), l.used_percent, l.reset_at.clone()))
                    .collect::<Vec<_>>(),
                now_ms,
            );
            let health = if limits.is_empty() {
                ProviderHealth::Unknown
            } else {
                ProviderHealth::Live
            };
            let mut dto = base_usage(ProviderKind::OpenCodeGo, health, limits);
            dto.account = build_opencode_attribution(
                usage.account.as_ref().map(|account| account.key_hint.as_str()),
            );
            Ok(dto)
        }
        Err(error) => Err(failure_from_provider_error(error)),
    }
}

/// Only a parseable ISO-8601 stamp is a freshness signal; an unreadable
/// stamp is no stamp at all (TS `parseSourceUpdatedAt` parity).
fn parse_source_updated_at(value: Option<&str>) -> Option<String> {
    let value = value?;
    match DateTime::parse_from_rfc3339(value) {
        Ok(_) => Some(value.to_string()),
        Err(_) => None,
    }
}

fn normalize_antigravity(
    result: Result<AntigravityUsage, AntigravityError>,
    now_ms: i64,
) -> Result<ProviderUsageDto, ProviderFailure> {
    match result {
        Ok(usage) => {
            let limits = limits_from(
                &usage
                    .limits
                    .iter()
                    .map(|l| (l.label.clone(), l.used_percent, l.reset_at.clone()))
                    .collect::<Vec<_>>(),
                now_ms,
            );
            let source_updated_at = parse_source_updated_at(usage.source_updated_at.as_deref());
            // A "fresh" verdict is trusted only with a usable stamp; an
            // absent verdict, a fresh verdict without a stamp, and an
            // unreadable stamp all degrade to stale.
            let data_freshness = if matches!(usage.data_freshness, DataSourceFreshness::Fresh)
                && source_updated_at.is_some()
            {
                "fresh"
            } else {
                "stale"
            };
            // Empty windows stay Unknown even when the verdict is stale —
            // there is nothing to call stale. Health carries the state;
            // dataFreshness stays the independent source-snapshot signal.
            let health = if limits.is_empty() {
                ProviderHealth::Unknown
            } else if data_freshness == "stale" {
                ProviderHealth::Stale
            } else {
                ProviderHealth::Live
            };
            let mut dto = base_usage(ProviderKind::Antigravity, health, limits);
            dto.source_updated_at = source_updated_at;
            dto.data_freshness = Some(data_freshness);
            // The adapter's cache-fallback construction carries the fresh
            // live-failure cause as diagnostic evidence. It rides the
            // internal, never-serialized field so the diagnostics lane can
            // record it while the entry's wire shape and health semantics
            // stay exactly as accepted.
            dto.fallback_failure = usage
                .fallback_failure
                .map(failure_from_antigravity_error);
            Ok(dto)
        }
        Err(error) => Err(failure_from_antigravity_error(error)),
    }
}

/// Grok attribution: the backend's masked account hint maps onto the shared
/// v0.3.1 contract (masked id + owning store; identity keys on the
/// store-agnostic masked id).
fn build_grok_attribution(account: Option<&grok::GrokAccount>) -> Option<AccountAttributionDto> {
    let account = account?;
    let id = account.id.trim();
    if id.is_empty() {
        return None;
    }
    let source = account.source.trim();
    Some(AccountAttributionDto {
        label: if source.is_empty() {
            id.to_string()
        } else {
            format!("{id} · {source}")
        },
        note: Some(
            "Only the active xAI account's stored credential is shown; other xAI accounts are not included."
                .to_string(),
        ),
        identity: Some(format!("xai:{id}")),
    })
}

fn normalize_grok(
    result: Result<GrokUsage, GrokError>,
    now_ms: i64,
) -> Result<ProviderUsageDto, ProviderFailure> {
    match result {
        Ok(usage) => {
            let limits = limits_from(
                &usage
                    .limits
                    .iter()
                    .map(|l| (l.label.clone(), l.used_percent, l.reset_at.clone()))
                    .collect::<Vec<_>>(),
                now_ms,
            );
            let mut dto = base_usage(ProviderKind::Grok, ProviderHealth::Live, limits);
            dto.account = build_grok_attribution(usage.account.as_ref());
            Ok(dto)
        }
        Err(error) => Err(failure_from_grok_error(error)),
    }
}

/// The production provider set — the exact registry the TS `registry.ts`
/// pinned (same ids, same order, no simulated providers). Order is wire
/// order: the frontend renders and summarizes in this order.
pub fn production_specs() -> Vec<ProviderSpec> {
    // Each closure stamps the fetch-time clock for the reset plausibility
    // gate inside the normalizer — the gate evaluates bounds against the
    // moment the provider actually answered.
    vec![
        ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(|| {
                Box::pin(async {
                    let now_ms = Utc::now().timestamp_millis();
                    normalize_codex(codex::get_codex_usage().await, now_ms)
                })
            }),
        },
        ProviderSpec {
            kind: ProviderKind::Zai,
            fetch: Arc::new(|| {
                Box::pin(async {
                    let now_ms = Utc::now().timestamp_millis();
                    // The supplemental observations ride the existing Z.ai
                    // job of the same cycle (`tokio::join!` runs all three on
                    // this one task — no second scheduler, no extra cycle);
                    // each failure is consumed independently of the quota
                    // result.
                    let (usage, reset_cards, plans) = tokio::join!(
                        zai::get_zai_usage(),
                        zcode_reset::fetch_reset_status(),
                        zcode_plans::fetch_plan_balances()
                    );
                    normalize_zai(usage, reset_cards.ok(), plans.ok(), now_ms)
                })
            }),
        },
        ProviderSpec {
            kind: ProviderKind::OpenCodeGo,
            fetch: Arc::new(|| {
                Box::pin(async {
                    let now_ms = Utc::now().timestamp_millis();
                    normalize_opencode_go(opencode_go::get_opencode_go_usage().await, now_ms)
                })
            }),
        },
        ProviderSpec {
            kind: ProviderKind::Antigravity,
            fetch: Arc::new(|| {
                Box::pin(async {
                    let now_ms = Utc::now().timestamp_millis();
                    normalize_antigravity(antigravity::get_antigravity_usage().await, now_ms)
                })
            }),
        },
        ProviderSpec {
            kind: ProviderKind::Grok,
            fetch: Arc::new(|| {
                Box::pin(async {
                    let now_ms = Utc::now().timestamp_millis();
                    normalize_grok(grok::get_grok_usage().await, now_ms)
                })
            }),
        },
    ]
}

// ---------- retry, cadence, cooldown, and last-good constants ----------

const RETRY_DELAY_MS: u64 = 750;
const RETRY_JITTER_MS: u64 = 250;
/// Grok's own polling cadence: the quota is a 7-day credit pool on an
/// undocumented CLI plane, so it is polled at most once per 15 minutes
/// regardless of the global refresh interval.
const GROK_MIN_POLL_INTERVAL_MS: i64 = 15 * 60_000;
/// Grok failures resolved without network traffic; never cached, so a
/// re-auth takes effect on the next poll.
const GROK_LOCAL_CREDENTIAL_CODES: [&str; 3] =
    ["credential_missing", "credential_expired", "credential_ambiguous"];

/// A server-directed provider cooldown from a `Retry-After` hint. Ephemeral
/// runtime state: never persisted, never spans an app restart, and its
/// `failure` is reused verbatim for the skip projection so a cooling-down
/// provider keeps surfacing the same honest rate-limit error it failed with.
#[derive(Debug, Clone, PartialEq)]
struct ProviderCooldown {
    until: DateTime<Utc>,
    failure: ProviderFailure,
    failure_at: DateTime<Utc>,
}

/// The automatic resilience triggers. Unlike manual refreshes they arrive
/// in bursts (both webviews fire `online`; OS resume can emit repeated
/// events), so each kind carries its own small dedupe window and every
/// trigger converges into the same single-owner cycle machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutoTrigger {
    /// The system resumed from sleep/suspend (scheduler wake detection).
    Wake,
    /// Connectivity returned (webview `online` event → trigger only).
    Reconnect,
    /// A known quota reset's post-reset guard elapsed (scheduler reset-wake
    /// detection): the one cycle that observes the post-reset state without
    /// waiting for the next scheduled poll.
    ResetWake,
}

/// Burst window for the automatic triggers: repeat events of one kind
/// inside this span coalesce into the cycle the first one started.
const TRIGGER_DEDUPE_MS: i64 = 5_000;

// ---------- suspicious-drop confirmation (v0.8 Lane B runtime trust) ----------

/// A candidate sample reading this much lower than the newest stored
/// observation of its logical window is held for confirmation instead of
/// being trusted. Parity with the engine's `resetDropPoints` and
/// `history::is_reset_boundary`'s >5-point drop: those only ever see
/// confirmed history.
const SUSPICIOUS_DROP_PERCENT: f64 = 5.0;

/// Provider jitter tolerance: a follow-up sample back within this distance
/// of the baseline refutes the held candidate — the drop read like a
/// transient glitch, not a real boundary.
const DROP_CONFIRM_TOLERANCE_PERCENT: f64 = 2.0;

/// How long a held candidate may wait for its resolving sample. Evaluated
/// lazily at classification time — no timers, no background task. Expiry
/// discards the candidate (it was never written) and the window's next
/// regular cycle re-classifies from scratch.
const PENDING_CONFIRMATION_TTL_MS: i64 = 15 * 60_000;

/// Global safety cap on simultaneously held candidates across all windows
/// and providers. A suspicious candidate arriving while the cap is full is
/// discarded, never written.
const PENDING_CONFIRMATIONS_MAX: usize = 8;

/// The logical window key — exactly the history store's identity:
/// `(providerId, account, windowLabel)`. One window's hold never gates
/// another window's (or another account's) recording.
type WindowKey = (String, Option<String>, String);

/// One suspicious-drop candidate held pending confirmation. In-memory only:
/// never persisted, never written to history, dropped at cold start and by
/// the Local Data clear paths.
#[derive(Debug, Clone)]
struct PendingConfirmation {
    /// The held candidate `C`, withheld from history until a second sample
    /// resolves it.
    candidate: QuotaObservation,
    /// The baseline `P` the candidate was classified against: the newest
    /// stored observation of the window at hold time.
    baseline: QuotaObservation,
    /// Hold time (runtime clock, epoch ms) — drives the lazy TTL.
    held_at_ms: i64,
}

// ---------- runtime core ----------

struct RuntimeInner {
    usages: Vec<ProviderUsageDto>,
    last_good: HashMap<&'static str, ProviderUsageDto>,
    /// Active server-directed cooldowns by provider id (ephemeral).
    cooldowns: HashMap<&'static str, ProviderCooldown>,
    /// Last accepted automatic trigger per kind (burst dedupe).
    last_auto_trigger: HashMap<AutoTrigger, DateTime<Utc>>,
    grok_last_result: Option<ProviderUsageDto>,
    grok_last_attempt_at: Option<DateTime<Utc>>,
    last_updated_at: Option<DateTime<Utc>>,
    last_cycle_started_at: Option<DateTime<Utc>>,
    last_cycle_completed_at: Option<DateTime<Utc>>,
    last_failures: HashMap<&'static str, (ProviderFailure, DateTime<Utc>)>,
    last_successes: HashMap<&'static str, DateTime<Utc>>,
    cycle_succeeded: bool,
    cycle_in_flight: bool,
    rerun_pending: bool,
    /// Set with `rerun_pending` when a suspicious-drop hold requests its
    /// confirmation follow-up; consumed by `finish_cycle` into the one-cycle
    /// Grok cadence-cache bypass.
    confirmation_follow_up_requested: bool,
    /// True for exactly the follow-up cycle that carries a confirmation
    /// request: the provider's cadence cache is bypassed so the confirming
    /// sample is a fresh upstream observation, not a cached echo of the
    /// sample under confirmation.
    grok_cache_bypass: bool,
    seq: u64,
    interval_minutes: u64,
    /// Suspicious-drop candidates held pending confirmation, keyed by
    /// logical window. In-memory only (dropped at cold start — held
    /// candidates were never persisted, so nothing is lost).
    pending_confirmations: HashMap<WindowKey, PendingConfirmation>,
}


/// Events the chain runner surfaces to its owner (the app handle in
/// production; a collector in tests).
pub enum RuntimeEvent {
    CycleStarted,
    Snapshot(RuntimeSnapshot),
}

pub struct RuntimeCore {
    inner: Mutex<RuntimeInner>,
    specs: Vec<ProviderSpec>,
    retry_delay_ms: Box<dyn Fn() -> u64 + Send + Sync>,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    /// The Rust-owned quota history store, when one is attached (production
    /// always attaches; tests without history leave it off).
    history: Option<Arc<QuotaHistoryStore>>,
    /// The threshold notification lane, when one is attached (production
    /// always attaches; tests inject a collecting sink or leave it off).
    notifications: Option<Arc<NotificationLane>>,
    /// Persisted normalized last-good provider state for v0.6 cold-start hydration.
    last_good_store: Option<Arc<ProviderLastGoodStore>>,
}

impl RuntimeCore {
    pub fn new(specs: Vec<ProviderSpec>, interval_minutes: u64) -> Self {
        Self::with_injections(
            specs,
            interval_minutes,
            Box::new(|| RETRY_DELAY_MS + rand_below(RETRY_JITTER_MS)),
            Box::new(Utc::now),
        )
    }

    /// Attaches the quota history store this runtime records into. Chainable
    /// so tests can keep using `with_injections` unchanged.
    pub fn with_history_store(mut self, store: Option<Arc<QuotaHistoryStore>>) -> Self {
        self.history = store;
        self
    }

    /// Attaches the threshold notification lane evaluated once per completed
    /// cycle, next to the history recording.
    pub fn with_last_good_store(
        mut self,
        store: Option<Arc<ProviderLastGoodStore>>,
    ) -> Self {
        if let Some(store) = &store {
            let (usages, last_good) = store.hydrate(&self.specs, (self.now)());
            let mut inner = self.inner.lock().unwrap();
            inner.usages = usages;
            inner.last_good = last_good;
        }
        self.last_good_store = store;
        self
    }

    pub fn with_notification_lane(mut self, lane: Option<Arc<NotificationLane>>) -> Self {
        self.notifications = lane;
        self
    }

    pub fn with_injections(
        specs: Vec<ProviderSpec>,
        interval_minutes: u64,
        retry_delay_ms: Box<dyn Fn() -> u64 + Send + Sync>,
        now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    ) -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                usages: Vec::new(),
                last_good: HashMap::new(),
                cooldowns: HashMap::new(),
                last_auto_trigger: HashMap::new(),
                grok_last_result: None,
                grok_last_attempt_at: None,
                last_updated_at: None,
                last_cycle_started_at: None,
                last_cycle_completed_at: None,
                last_failures: HashMap::new(),
                last_successes: HashMap::new(),
                cycle_succeeded: false,
                cycle_in_flight: false,
                rerun_pending: false,
                confirmation_follow_up_requested: false,
                grok_cache_bypass: false,
                seq: 0,
                interval_minutes,
                pending_confirmations: HashMap::new(),
            }),
            specs,
            retry_delay_ms,
            now,
            history: None,
            notifications: None,
            last_good_store: None,
        }
    }

    fn now(&self) -> DateTime<Utc> {
        (self.now)()
    }

    fn now_iso(&self) -> String {
        self.now().to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    // ---- cycle lifecycle (RefreshCoordinator semantics) ----

    /// Marks a cycle as starting, or — when one is already in flight — sets
    /// the single pending follow-up flag. Returns whether the caller owns
    /// the new cycle (and must run the chain).
    pub fn try_begin_cycle(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.cycle_in_flight {
            inner.rerun_pending = true;
            return false;
        }
        inner.cycle_in_flight = true;
        true
    }

    /// The automatic-trigger entry (wake, reconnect): a per-kind burst
    /// dedupe window absorbs event storms, then the flow is exactly
    /// [`Self::try_begin_cycle`] — one owner, one pending follow-up. A
    /// trigger landing inside the dedupe window is dropped: the cycle the
    /// first trigger started (or its follow-up) already covers it. Manual
    /// refreshes never pass through here and are never deduped.
    pub fn try_begin_auto_cycle(&self, trigger: AutoTrigger) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let now = self.now();
        if let Some(last) = inner.last_auto_trigger.get(&trigger) {
            if (now - *last).num_milliseconds() < TRIGGER_DEDUPE_MS {
                return false;
            }
        }
        inner.last_auto_trigger.insert(trigger, now);
        if inner.cycle_in_flight {
            inner.rerun_pending = true;
            return false;
        }
        inner.cycle_in_flight = true;
        true
    }

    /// Called after each completed cycle: decides whether exactly one
    /// follow-up cycle runs (a request landed mid-cycle), and otherwise
    /// closes the chain. A follow-up that carries a suspicious-drop
    /// confirmation request runs with the Grok cadence-cache bypass set —
    /// the plan's "at most one bounded confirmation fetch per candidate"
    /// (§5.2.2) is a fetch; serving the provider's cached last result would
    /// echo the very sample under confirmation instead of observing it
    /// again. Cooldowns and retry rules are untouched.
    pub fn finish_cycle(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.rerun_pending {
            inner.rerun_pending = false;
            inner.grok_cache_bypass = inner.confirmation_follow_up_requested;
            inner.confirmation_follow_up_requested = false;
            return true;
        }
        inner.cycle_in_flight = false;
        inner.grok_cache_bypass = false;
        false
    }

    /// Runs the full chain: one fetch cycle plus at most one coalesced
    /// follow-up, emitting one snapshot event per completed cycle and one
    /// cycle-started event at chain start (the frontend's loading span).
    pub async fn run_chain<E: FnMut(RuntimeEvent)>(self: &Arc<Self>, mut on_event: E) {
        on_event(RuntimeEvent::CycleStarted);
        loop {
            // Scheduler panic guard: each cycle runs in its own task, so one
            // panic unwinds that task instead of the chain. finish_cycle
            // below still closes the in-flight flag (a panicked cycle never
            // reached it), the snapshot still closes the frontend loading
            // span, and the next tick starts a fresh cycle — one panic
            // cannot permanently stop collection. The panicked cycle's own
            // results are discarded; the emitted snapshot shows the last
            // completed cycle's state.
            let core = self.clone();
            let _ = tokio::spawn(async move { core.run_cycle().await }).await;
            let follow_up = self.finish_cycle();
            on_event(RuntimeEvent::Snapshot(self.snapshot()));
            if !follow_up {
                break;
            }
        }
    }

    // ---- one fetch cycle ----

    /// Fetches every registered provider concurrently (independent jobs, like
    /// the former `Promise.all`), then updates the shared state under one
    /// lock. Provider order in the snapshot is always the spec order.
    pub async fn run_cycle(self: &Arc<Self>) {
        self.inner.lock().unwrap().last_cycle_started_at = Some(self.now());
        let mut jobs = tokio::task::JoinSet::new();
        for (index, spec) in self.specs.iter().enumerate() {
            let core = self.clone();
            let spec = spec.clone();
            jobs.spawn(async move { (index, core.run_provider_job(&spec).await) });
        }
        let mut results: Vec<(usize, ProviderUsageDto)> = Vec::new();
        let mut lost_task_panics: usize = 0;
        while let Some(joined) = jobs.join_next().await {
            match joined {
                Ok(pair) => results.push(pair),
                Err(error) => {
                    // A job task died without a result (a panic outside the
                    // fetch boundary). Only the fact is recorded: the payload
                    // is never read, attributed, or retained — it is not
                    // guaranteed display-safe (see `failure_from_panic`).
                    if error.is_panic() {
                        lost_task_panics += 1;
                    }
                }
            }
        }
        results.sort_by_key(|(index, _)| *index);

        // Panic containment (belt over the fetch boundary's braces): every
        // registered provider must surface in the cycle. A spec whose job
        // ended without a result is projected through the ordinary failure
        // path — last-good retention, error metadata, and diagnostics behave
        // exactly like any failed refresh; a provider can never silently
        // disappear.
        let completed: HashSet<usize> = results.iter().map(|(index, _)| *index).collect();
        for (index, spec) in self.specs.iter().enumerate() {
            if completed.contains(&index) {
                continue;
            }
            let failure = if lost_task_panics == 1 {
                failure_from_panic()
            } else {
                job_lost_failure()
            };
            let dto = self.apply_failure(spec.kind, failure);
            results.push((index, dto));
        }
        results.sort_by_key(|(index, _)| *index);

        let usages: Vec<ProviderUsageDto> = results.into_iter().map(|(_, dto)| dto).collect();
        // History recording (phase 2): one batch per completed cycle, from
        // the same normalized snapshot that is about to be broadcast.
        // Recording before the snapshot state update means any snapshot
        // read (and the snapshot event that follows) already reflects the
        // new history revision. Deduplication by snapshot time keeps a
        // repeated identical cycle from growing the stream.
        //
        // Lane B acceptance: candidates are classified before they reach the
        // store. A sample that reads as a suspicious usage drop is withheld
        // from the batch until a second sample confirms it (the hold
        // requests exactly one coalesced confirmation follow-up); history,
        // prediction, and analytics therefore never see an unconfirmed
        // cycle boundary. Held/rejected samples still ride the broadcast
        // snapshot as Live — users see current reality; only the history
        // write is gated, and the history revision moves only on accepted
        // writes.
        if let Some(history) = &self.history {
            let observations = crate::history::observations_from_usages(&usages);
            let now_ms = self.now().timestamp_millis();
            // One read of the detailed tier per cycle: the newest stored
            // observation per logical window is the classification baseline
            // (taken before this cycle's record, so a hold always classifies
            // against exactly what history contains).
            let stored = history.history_range(None, None, None, "24h");
            let (accepted, confirmation_requested) =
                self.admit_observations(&stored, observations, now_ms);
            let _ = history.record(accepted);
            if confirmation_requested {
                self.request_confirmation_follow_up();
            }
        }
        // Threshold notifications (v0.5 step 4): one evaluation per
        // completed cycle, right next to the history recording, so the main
        // and floating webviews can never produce duplicate notifications.
        if let Some(lane) = &self.notifications {
            lane.process(&usages, self.now());
        }
        let completed_at = self.now();
        let mut inner = self.inner.lock().unwrap();
        // A cycle is globally successful when at least one real source
        // returned usable data (live or stale) — the same verdict the
        // frontend's refreshCycleSucceeded applied.
        inner.cycle_succeeded = usages
            .iter()
            .any(|usage| matches!(usage.health, ProviderHealth::Live | ProviderHealth::Stale));
        if inner.cycle_succeeded {
            inner.last_updated_at = Some(completed_at);
        }
        inner.last_cycle_completed_at = Some(completed_at);
        inner.usages = usages;
        inner.seq += 1;
    }

    async fn run_provider_job(self: &Arc<Self>, spec: &ProviderSpec) -> ProviderUsageDto {
        // Server-directed cooldown first: while it lasts the provider is not
        // fetched at all, by any trigger. Eligibility is the later of this
        // and the provider cadence below.
        if let Some(cooldown) = self.active_cooldown(spec.kind.id()) {
            return self.cooldown_skip_dto(spec.kind, cooldown);
        }
        // Grok cadence: serve the cached last result inside the 15-minute
        // window — except on a confirmation follow-up cycle, which must
        // observe fresh upstream state (a served cache would return the
        // held candidate's own glitch and "confirm" the drop with it);
        // otherwise stamp the attempt time before fetching.
        if spec.kind == ProviderKind::Grok {
            let bypass = self.inner.lock().unwrap().grok_cache_bypass;
            if !bypass {
                if let Some(cached) = self.grok_cached_result() {
                    return cached;
                }
            }
            self.inner.lock().unwrap().grok_last_attempt_at = Some(self.now());
        }

        let outcome = self.fetch_with_retry(spec).await;
        let mut final_failure: Option<ProviderFailure> = None;
        let dto = match outcome {
            Ok(dto) => {
                // A structurally successful refresh clears any stale
                // cooldown entry; the provider is simply eligible again.
                self.inner.lock().unwrap().cooldowns.remove(spec.kind.id());
                self.apply_success(spec.kind, dto)
            }
            Err(failure) => {
                self.maybe_apply_cooldown(spec.kind, &failure);
                final_failure = Some(failure.clone());
                self.apply_failure(spec.kind, failure)
            }
        };

        if spec.kind == ProviderKind::Grok {
            // Cache whatever the attempt produced — success or failure —
            // except local credential failures, which must never mask a
            // re-auth until the next poll.
            let credential_failure = final_failure
                .map(|failure| GROK_LOCAL_CREDENTIAL_CODES.contains(&failure.code.as_str()))
                .unwrap_or(false);
            if !credential_failure {
                self.inner.lock().unwrap().grok_last_result = Some(dto.clone());
            }
        }
        dto
    }

    async fn fetch_with_retry(
        &self,
        spec: &ProviderSpec,
    ) -> Result<ProviderUsageDto, ProviderFailure> {
        let first = fetch_contained(spec).await;
        let Err(first_failure) = first else {
            return first;
        };
        if !self.should_retry(spec.kind, &first_failure) {
            return Err(first_failure);
        }
        tokio::time::sleep(Duration::from_millis((self.retry_delay_ms)())).await;
        fetch_contained(spec).await
    }

    fn should_retry(&self, kind: ProviderKind, failure: &ProviderFailure) -> bool {
        // The passive Antigravity backend has no retry wrapper today.
        if kind == ProviderKind::Antigravity {
            return false;
        }
        if !failure.is_transient() {
            return false;
        }
        // A server-directed Retry-After cannot be honored by a fast retry
        // for any provider: the hint is recorded as a cooldown instead, and
        // the next cadence/scheduler cycle serves as the retry.
        if failure.retry_after_ms.unwrap_or(0) != 0 {
            return false;
        }
        // A transport timeout is terminal for the current cycle. The stall
        // that consumed the whole request budget will not clear in the
        // ~750 ms before a fast retry, and burning a second full budget
        // would double the worst-case cycle tail (~31 s → ~15 s) while the
        // atomic snapshot waits for every provider job. The next
        // cadence/scheduler/trigger cycle serves as the retry. Fast-failing
        // transport errors (refused connections, DNS misses) keep the one
        // bounded retry — they cost milliseconds, not the budget.
        !failure.transport_timeout
    }

    // ---- suspicious-drop confirmation (v0.8 Lane B) ----

    /// Admits one cycle's candidate observations into history. A candidate
    /// that reads as a suspicious usage drop (> [`SUSPICIOUS_DROP_PERCENT`]
    /// below the newest stored observation of its logical window, without an
    /// announced reset) is held: withheld from the recorded batch until a
    /// second sample confirms the drop, refutes it, or the lazy TTL
    /// discards it. Returns the batch to record plus whether at least one
    /// new hold requests a confirmation follow-up cycle.
    ///
    /// Deterministic accept / reject / defer, per candidate:
    /// - accept: normal sample, or a confirming second sample (the held
    ///   candidate and the confirming sample are written time-ordered —
    ///   `record()` sorts — so history shows the true boundary and the
    ///   confirming sample);
    /// - reject: a refuting second sample records normally and the held
    ///   candidate is discarded (never written);
    /// - defer: an inconclusive second sample (inside the jitter band), or
    ///   a cached echo of the held sample (same stamp/value/bound — not a
    ///   second observation), writes nothing and requests nothing — the
    ///   pending waits for the next regular sample or its TTL.
    fn admit_observations(
        &self,
        stored: &[QuotaObservation],
        candidates: Vec<QuotaObservation>,
        now_ms: i64,
    ) -> (Vec<QuotaObservation>, bool) {
        let mut accepted = Vec::new();
        let mut confirmation_requested = false;
        let mut inner = self.inner.lock().unwrap();
        let baselines = newest_by_window(stored);

        // Lazy TTL expiry: a held candidate whose confirmation window ran
        // out is discarded — never written — and stops gating its window.
        let expired: Vec<WindowKey> = inner
            .pending_confirmations
            .iter()
            .filter(|(_, pending)| now_ms - pending.held_at_ms > PENDING_CONFIRMATION_TTL_MS)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            inner.pending_confirmations.remove(&key);
        }

        for candidate in candidates {
            let key: WindowKey = (
                candidate.provider_id.clone(),
                candidate.account.clone(),
                candidate.window_label.clone(),
            );
            if let Some(pending) = inner.pending_confirmations.get_mut(&key) {
                // A provider-cached echo of the held sample is not a second
                // upstream observation: same stamp, value, and bound means
                // the cycle was served from the provider's cadence cache
                // (or replayed the same response). Confirming from it would
                // validate the glitch with its own echo — defer instead;
                // the pending waits for a genuinely fresh sample.
                if candidate.observed_at == pending.candidate.observed_at
                    && candidate.used_percent == pending.candidate.used_percent
                    && candidate.reset_at == pending.candidate.reset_at
                {
                    continue;
                }
                // A confirmation is already in flight for this window: the
                // second sample is whatever the runtime sees next — the
                // confirmation cycle's fetch, or any later regular cycle's.
                if confirms_drop(&candidate, &pending.baseline) {
                    accepted.push(pending.candidate.clone());
                    accepted.push(candidate);
                    inner.pending_confirmations.remove(&key);
                } else if refutes_drop(&candidate, &pending.baseline) {
                    accepted.push(candidate);
                    inner.pending_confirmations.remove(&key);
                }
                continue;
            }

            // No pending: classify against the newest stored observation of
            // the window. Absent — or older than the detailed tier, which
            // the 24 h read already implements — classifies as normal: a
            // drop after a long gap is an expected reset signature, not a
            // glitch.
            let Some(baseline) = baselines.get(&key).map(|obs| (*obs).clone()) else {
                accepted.push(candidate);
                continue;
            };
            if !is_suspicious_drop(&candidate, &baseline) {
                accepted.push(candidate);
                continue;
            }
            // Suspicious: hold. The global cap bounds in-memory state; a
            // candidate arriving while the cap is full is discarded, never
            // written.
            if inner.pending_confirmations.len() >= PENDING_CONFIRMATIONS_MAX {
                continue;
            }
            inner.pending_confirmations.insert(
                key,
                PendingConfirmation {
                    candidate,
                    baseline,
                    held_at_ms: now_ms,
                },
            );
            confirmation_requested = true;
        }
        (accepted, confirmation_requested)
    }

    /// Requests the single coalesced confirmation follow-up. Every
    /// production hold happens inside `run_cycle` — a cycle is always in
    /// flight there — so this lands on the one pending-follow-up flag the
    /// cycle machinery already owns: N holds across N windows coalesce into
    /// exactly one follow-up, and a request during an in-flight cycle never
    /// starts a second parallel cycle. No refresh storm is possible: a held
    /// candidate requests a rerun once (it never re-enters the hold branch),
    /// and if the rerun's sample still does not resolve it, only the TTL or
    /// a later regular cycle remains. Direct `run_cycle` calls (tests) leave
    /// the flag alone; the pending then resolves on the next regular cycle,
    /// the same bounded fallback an unresolved follow-up has.
    fn request_confirmation_follow_up(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.cycle_in_flight {
            inner.rerun_pending = true;
            // Mark the follow-up as a confirmation fetch: finish_cycle turns
            // this into the one-cycle cadence-cache bypass, so the confirming
            // sample is freshly fetched instead of the provider's cached
            // echo of the held candidate.
            inner.confirmation_follow_up_requested = true;
        }
    }

    /// Drops every held candidate. The Local Data clear paths (usage
    /// history, provider cache) and cold start must not leave gated windows
    /// behind: with the pendings gone, the next cycle classifies each
    /// window from scratch against whatever history remains.
    pub fn clear_pending_confirmations(&self) {
        self.inner.lock().unwrap().pending_confirmations.clear();
    }

    // ---- server-directed cooldown (Retry-After) ----

    /// The provider's active cooldown, if it has not expired yet.
    fn active_cooldown(&self, id: &'static str) -> Option<ProviderCooldown> {
        let inner = self.inner.lock().unwrap();
        let cooldown = inner.cooldowns.get(id)?;
        (self.now() < cooldown.until).then(|| cooldown.clone())
    }

    /// Records a cooldown from a failure that carried a `Retry-After` hint,
    /// capped at the shared safety maximum. Failures without a usable hint
    /// (including all 429/5xx without the header) leave cooldown state
    /// untouched — the bounded one-retry rule still covers them.
    fn maybe_apply_cooldown(&self, kind: ProviderKind, failure: &ProviderFailure) {
        let wait_ms = failure.retry_after_ms.unwrap_or(0);
        if wait_ms == 0 {
            return;
        }
        let now = self.now();
        let until = now + chrono::Duration::milliseconds(wait_ms.min(MAX_COOLDOWN_MS) as i64);
        self.inner.lock().unwrap().cooldowns.insert(
            kind.id(),
            ProviderCooldown {
                until,
                failure: failure.clone(),
                failure_at: now,
            },
        );
    }

    /// The projection a cooling-down provider surfaces: the failed refresh's
    /// shape (last-good retained, or a bare error entry) with the health
    /// explicitly `cooldown` — the provider is not broken, it is deferred by
    /// the server. History samples only live/fresh refreshes, so a skipped
    /// fetch can never invent a quota observation. The legacy `status` stays
    /// `"error"` (the legacy vocabulary has no cooldown word).
    fn cooldown_skip_dto(&self, kind: ProviderKind, cooldown: ProviderCooldown) -> ProviderUsageDto {
        // Integration resolution (v0.6 rehearsal): the status-contract lane
        // owns the canonical health overlay (Cooldown), the diagnostics lane
        // owns projecting with the failure's original timestamp so the
        // redacted diagnostics timeline shows when the refresh actually
        // failed, not when the cooldown skip was surfaced. Both are kept.
        let mut dto = self.apply_failure_at(kind, cooldown.failure, cooldown.failure_at);
        set_health(&mut dto, ProviderHealth::Cooldown);
        dto
    }

    // ---- success / failure state transitions ----

    fn apply_success(&self, kind: ProviderKind, dto: ProviderUsageDto) -> ProviderUsageDto {
        let mut dto = dto;
        dto.checked_at = self.now_iso();
        // A structurally successful refresh that degraded to cached data
        // (today: only the Antigravity adapter) carries the fresh
        // live-failure cause on its internal, never-serialized field.
        // Recording it in the diagnostics lane's last-failure map is the
        // only observable effect: the export bundle can explain why cached
        // data was used, while the entry's wire shape, health, history, and
        // notification eligibility stay exactly as for any other success.
        if let Some(live_failure) = dto.fallback_failure.clone() {
            self.inner
                .lock()
                .unwrap()
                .last_failures
                .insert(kind.id(), (live_failure, self.now()));
        }
        self.inner
            .lock()
            .unwrap()
            .last_successes
            .insert(kind.id(), self.now());
        match kind {
            // A structurally successful refresh stores last-good data —
            // even an "unknown" one (the TS adapters stored whatever the
            // success produced).
            ProviderKind::Codex | ProviderKind::Zai | ProviderKind::OpenCodeGo
            | ProviderKind::Antigravity => {
                self.inner
                    .lock()
                    .unwrap()
                    .last_good
                    .insert(kind.id(), dto.clone());
                if let Some(store) = &self.last_good_store {
                    store.record_success(kind, &dto);
                }
            }
            ProviderKind::Grok => {
                if dto.limits.is_empty() {
                    // Zero-window success: usage is unknown upstream, never
                    // 0%. The last good snapshot is retained (not retired)
                    // and surfaced as "unknown"; without last good data the
                    // bare unknown entry stands.
                    set_health(&mut dto, ProviderHealth::Unknown);
                    let inner = self.inner.lock().unwrap();
                    if let Some(last_good) = inner.last_good.get(kind.id()) {
                        let mut retained = last_good.clone();
                        set_health(&mut retained, ProviderHealth::Unknown);
                        return retained;
                    }
                } else {
                    self.inner
                        .lock()
                        .unwrap()
                        .last_good
                        .insert(kind.id(), dto.clone());
                    if let Some(store) = &self.last_good_store {
                        store.record_success(kind, &dto);
                    }
                }
            }
        }
        dto
    }

    /// Failure projection. With retained last-good the health is `error` —
    /// the refresh failed, and the retained data stays honestly marked
    /// (never live, never ordinary stale). Without last-good the health is
    /// `unavailable` when the failure says the required local source/auth
    /// state is absent (no credential, expired auth, missing cache), and
    /// plain `error` otherwise (transport, HTTP, schema, entitlement).
    /// Both paths carry the normalized error metadata (category = the
    /// structured error's stable code, HTTP status when there was one).
    fn apply_failure(&self, kind: ProviderKind, failure: ProviderFailure) -> ProviderUsageDto {
        self.apply_failure_at(kind, failure, self.now())
    }

    fn apply_failure_at(
        &self,
        kind: ProviderKind,
        failure: ProviderFailure,
        failure_at: DateTime<Utc>,
    ) -> ProviderUsageDto {
        let message = failure.describe(kind.fallback_error());
        // Integration resolution (v0.6 rehearsal): the status-contract lane
        // owns the normalized error metadata (category + bare HTTP status);
        // the diagnostics lane owns recording the structured failure with
        // its original timestamp for the redacted diagnostics source. Both
        // are kept — neither behavior subsumes the other.
        self.inner.lock().unwrap().last_failures.insert(
            kind.id(),
            (failure.clone(), failure_at),
        );
        let error_category = if failure.code.is_empty() {
            None
        } else {
            Some(failure.code.clone())
        };
        let error_http_status = failure.http_status;
        let inner = self.inner.lock().unwrap();
        if let Some(last_good) = inner.last_good.get(kind.id()) {
            // Account guard (MIC-297 follow-up): last-good data may only be
            // retained for the account the failed refresh actually attempted.
            // When the backend resolved a credential before failing and its
            // masked identity differs from the retained snapshot's
            // attribution — the stored credential changed between cycles —
            // the retained snapshot belongs to another account and must not
            // stand in for the new one, even marked errored. Without a
            // resolved identity (the credential could not be read at all)
            // retention is unchanged.
            //
            // Integration resolution (v0.6 rehearsal): the identity guard
            // decides WHETHER retention happens; the canonical health
            // contract decides HOW a retained entry is presented — health
            // `error` via the single write point, plus the normalized error
            // metadata (category, bare HTTP status).
            let retained_identity = last_good
                .account
                .as_ref()
                .and_then(|account| account.identity.as_deref());
            let same_account = match (failure.identity.as_deref(), retained_identity) {
                (Some(attempted), Some(retained)) => attempted == retained,
                _ => true,
            };
            if same_account {
                // checkedAt stays at the original fetch time, so the card
                // shows how old the retained data is; the entry is
                // explicitly errored and can never read as healthy.
                let mut retained = last_good.clone();
                set_health(&mut retained, ProviderHealth::Error);
                retained.error = Some(format!("Refresh failed: {message}"));
                retained.error_category = error_category;
                retained.error_http_status = error_http_status;
                // A retained banked balance must not outlive its freshness
                // budget: stale counts stop reading as current here, while
                // the windows keep their existing retention semantics.
                trim_stale_reset_credits(&mut retained, failure_at);
                trim_stale_zcode_reset_cards(&mut retained, failure_at);
                trim_stale_zcode_plans(&mut retained, failure_at);
                return retained;
            }
        }
        drop(inner);
        let bare_health = failure_health(&failure);
        ProviderUsageDto {
            checked_at: self.now_iso(),
            error: Some(message),
            error_category,
            error_http_status,
            ..base_usage(kind, bare_health, Vec::new())
        }
    }

    fn grok_cached_result(&self) -> Option<ProviderUsageDto> {
        let inner = self.inner.lock().unwrap();
        let attempt = inner.grok_last_attempt_at?;
        let cached = inner.grok_last_result.clone()?;
        let age_ms = (self.now() - attempt).num_milliseconds();
        (age_ms < GROK_MIN_POLL_INTERVAL_MS).then_some(cached)
    }

    // ---- snapshot and settings ----

    pub fn snapshot(&self) -> RuntimeSnapshot {
        let history_revision = self
            .history
            .as_ref()
            .map(|store| store.revision())
            .unwrap_or(0);
        let inner = self.inner.lock().unwrap();
        RuntimeSnapshot {
            seq: inner.seq,
            providers: inner.usages.clone(),
            last_updated_at: inner
                .last_updated_at
                .map(|at| at.to_rfc3339_opts(SecondsFormat::Millis, true)),
            cycle_succeeded: inner.cycle_succeeded,
            cycle_in_flight: inner.cycle_in_flight,
            refresh_interval_minutes: inner.interval_minutes,
            history_revision,
        }
    }

    /// Read-only source data for the diagnostics safety layer. These are the
    /// runtime's normalized DTOs and structured failures only; provider
    /// response payloads are never retained here and cannot cross this API.
    pub fn diagnostic_source(&self, now: DateTime<Utc>) -> RuntimeDiagnosticSource {
        let inner = self.inner.lock().unwrap();
        let providers = self
            .specs
            .iter()
            .map(|spec| {
                let id = spec.kind.id();
                let usage = inner
                    .usages
                    .iter()
                    .find(|usage| usage.id == id)
                    .cloned()
                    .unwrap_or_else(|| base_usage(spec.kind, ProviderHealth::Unknown, Vec::new()));
                let cooldown_until = inner
                    .cooldowns
                    .get(id)
                    .filter(|cooldown| now < cooldown.until)
                    .map(|cooldown| cooldown.until);
                let (last_error, last_error_at) = inner
                    .last_failures
                    .get(id)
                    .map(|(failure, at)| {
                        (
                            Some(ErrorDiagnosticSource {
                                code: failure.code.clone(),
                                message: failure.message.clone(),
                                http_status: failure.http_status,
                            }),
                            Some(*at),
                        )
                    })
                    .unwrap_or((None, None));
                ProviderDiagnosticSource {
                    usage,
                    cooldown_until,
                    last_success_at: inner.last_successes.get(id).copied(),
                    last_error,
                    last_error_at,
                }
            })
            .collect();
        RuntimeDiagnosticSource {
            snapshot_seq: inner.seq,
            last_cycle_started_at: inner.last_cycle_started_at,
            last_cycle_completed_at: inner.last_cycle_completed_at,
            last_usable_data_at: inner.last_updated_at,
            refresh_interval_minutes: inner.interval_minutes,
            cycle_in_flight: inner.cycle_in_flight,
            follow_up_pending: inner.rerun_pending,
            providers,
        }
    }

    /// Pushes the persisted interval into the scheduler. The scheduler
    /// recomputes its next tick from now (matching the previous per-hook
    /// `setInterval` rebuild on change).
    pub fn set_interval(&self, minutes: u64) -> Option<u64> {
        let minutes = minutes.max(1);
        let mut inner = self.inner.lock().unwrap();
        if inner.interval_minutes == minutes {
            return None;
        }
        inner.interval_minutes = minutes;
        Some(minutes)
    }

    /// The scheduler's reset-wake query: the earliest guarded wake instant
    /// any currently shown window's `reset_at` justifies (see
    /// [`earliest_reset_wake_at`]), or `None` when no reset is eligible.
    /// A read over the same live snapshot the cycles publish.
    pub fn earliest_reset_wake_at(&self) -> Option<DateTime<Utc>> {
        let inner = self.inner.lock().unwrap();
        earliest_reset_wake_at(&inner.usages, self.now())
    }
}

/// Nanosecond-derived jitter in `0..ms` — no new dependency, and the intent
/// (bounded 0–250 ms jitter) matches the TS adapter.
fn rand_below(ms: u64) -> u64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    if ms == 0 { 0 } else { nanos % ms }
}

// ---------- suspicious-drop classification (v0.8 Lane B) ----------

/// A candidate is suspicious when it reads more than
/// [`SUSPICIOUS_DROP_PERCENT`] below the baseline **without** announcing a
/// reset. An announced reset (forward `reset_at` move > 1 min) is a
/// legitimate new cycle by definition — never held, never confirmed.
fn is_suspicious_drop(candidate: &QuotaObservation, baseline: &QuotaObservation) -> bool {
    candidate.used_percent < baseline.used_percent - SUSPICIOUS_DROP_PERCENT
        && !reset_moved_forward_more_than_a_minute(&baseline.reset_at, &candidate.reset_at)
}

/// True when `second` confirms the drop the held candidate reported: it
/// reads at least [`SUSPICIOUS_DROP_PERCENT`] below the baseline again, or
/// announces a reset (the provider legitimately restarted the window
/// between the two samples).
fn confirms_drop(second: &QuotaObservation, baseline: &QuotaObservation) -> bool {
    second.used_percent <= baseline.used_percent - SUSPICIOUS_DROP_PERCENT
        || reset_moved_forward_more_than_a_minute(&baseline.reset_at, &second.reset_at)
}

/// True when `second` refutes the held candidate: the provider is back
/// within [`DROP_CONFIRM_TOLERANCE_PERCENT`] of the baseline, so the held
/// drop read like a transient glitch. Values inside the band between the
/// refute and confirm thresholds are inconclusive and defer.
fn refutes_drop(second: &QuotaObservation, baseline: &QuotaObservation) -> bool {
    second.used_percent >= baseline.used_percent - DROP_CONFIRM_TOLERANCE_PERCENT
}

fn reset_moved_forward_more_than_a_minute(
    prev: &Option<String>,
    curr: &Option<String>,
) -> bool {
    let (Some(prev), Some(curr)) = (prev, curr) else {
        return false;
    };
    match (
        crate::history::parse_epoch_ms(prev),
        crate::history::parse_epoch_ms(curr),
    ) {
        (Some(prev_ms), Some(curr_ms)) => curr_ms.saturating_sub(prev_ms) > 60_000,
        _ => false,
    }
}

/// The newest stored observation per logical window. `stored` arrives in the
/// history store's canonical order (provider, account, window, time
/// ascending), so the last entry seen for a key is its newest observation.
fn newest_by_window(stored: &[QuotaObservation]) -> HashMap<WindowKey, &QuotaObservation> {
    let mut by_window: HashMap<WindowKey, &QuotaObservation> = HashMap::new();
    for observation in stored {
        by_window.insert(
            (
                observation.provider_id.clone(),
                observation.account.clone(),
                observation.window_label.clone(),
            ),
            observation,
        );
    }
    by_window
}

// ---------- app handle wiring ----------

#[derive(Clone)]
pub struct RuntimeHandle {
    core: Arc<RuntimeCore>,
    app: AppHandle,
    interval_tx: watch::Sender<u64>,
    /// The shared history store; the history commands act on the same store
    /// the runtime records into.
    history: Option<Arc<QuotaHistoryStore>>,
    /// The threshold notification lane the runtime evaluates; the settings
    /// command acts on the same lane.
    notifications: Option<Arc<NotificationLane>>,
    /// Persisted last-good provider state for v0.6 cold start.
    last_good: Option<Arc<ProviderLastGoodStore>>,
}

impl RuntimeHandle {
    /// Requests a refresh: starts a cycle (chain) if none is running,
    /// otherwise coalesces into the single pending follow-up.
    pub fn request_refresh(&self) {
        self.spawn_chain_if(self.core.try_begin_cycle());
    }

    /// Requests one shared cycle after the system wakes from sleep or
    /// suspend. Burst events coalesce; the cycle honors provider cadence
    /// and server cooldowns like every other source.
    pub fn request_refresh_on_wake(&self) {
        self.spawn_chain_if(self.core.try_begin_auto_cycle(AutoTrigger::Wake));
    }

    /// Requests one shared cycle after connectivity returns. The webview
    /// only triggers this — the Rust runtime stays the only fetch owner,
    /// and repeated `online` events coalesce.
    pub fn request_refresh_on_reconnect(&self) {
        self.spawn_chain_if(self.core.try_begin_auto_cycle(AutoTrigger::Reconnect));
    }

    /// Requests one shared cycle just after a known quota reset's
    /// post-reset guard elapsed. Joins the automatic-trigger model: the
    /// per-kind burst dedupe absorbs repeats, and the cycle honors provider
    /// cadence and server cooldowns like every other source.
    pub fn request_refresh_on_reset_wake(&self) {
        self.spawn_chain_if(self.core.try_begin_auto_cycle(AutoTrigger::ResetWake));
    }

    fn spawn_chain_if(&self, owns_cycle: bool) {
        if !owns_cycle {
            return;
        }
        let core = self.core.clone();
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            core.run_chain(|event| match event {
                RuntimeEvent::CycleStarted => {
                    let _ = app.emit(CYCLE_STARTED_EVENT, ());
                }
                RuntimeEvent::Snapshot(snapshot) => {
                    let _ = app.emit(SNAPSHOT_EVENT, &snapshot);
                }
            })
            .await;
        });
    }

    pub fn set_refresh_interval(&self, minutes: u64) {
        if let Some(next) = self.core.set_interval(minutes) {
            let _ = self.interval_tx.send(next);
        }
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.core.snapshot()
    }

    /// The quota history store shared with the runtime's recorder, for the
    /// history IPC commands.
    pub fn history_store(&self) -> Option<&Arc<QuotaHistoryStore>> {
        self.history.as_ref()
    }

    /// The persisted last-good cache shared with the runtime's hydration
    /// path, for the v0.7 local-data cache clear.
    pub fn last_good_store(&self) -> Option<&Arc<ProviderLastGoodStore>> {
        self.last_good.as_ref()
    }

    /// Drops held suspicious-drop candidates (v0.8 Lane B). The Local Data
    /// clear paths call this so a cleared history or cache never leaves
    /// gated windows behind.
    pub fn clear_pending_confirmations(&self) {
        self.core.clear_pending_confirmations();
    }

    pub fn diagnostic_source(&self, now: DateTime<Utc>) -> RuntimeDiagnosticSource {
        self.core.diagnostic_source(now)
    }

    pub fn notification_counts(&self) -> Option<(bool, usize, usize)> {
        self.notifications
            .as_ref()
            .map(|lane| lane.diagnostic_counts())
    }

    /// Forwards the quota-notifications toggle to the runtime's lane. Like
    /// the refresh interval, the persisted TS setting is pushed on attach
    /// and on every change; the lane persists it too, so the scheduler's
    /// immediate startup cycle (which can precede any attach) respects it.
    pub fn set_notifications_enabled(&self, enabled: bool) {
        if let Some(lane) = &self.notifications {
            lane.set_enabled(enabled);
        }
    }
}

// ---------- wake/resume detection (scheduler) ----------

/// The scheduler sleeps in chunks no longer than this, so a suspend that
/// freezes the monotonic clock is noticed within one chunk of wall time.
const SCHEDULER_CHUNK_MS: u64 = 30_000;
/// Wall-clock overshoot beyond a completed chunk that counts as "the
/// system slept and the timers froze" rather than scheduling jitter.
const WAKE_DRIFT_TOLERANCE_MS: u64 = 2_000;

/// Post-reset guard: the wake for a known quota reset fires this long after
/// the announced `reset_at`, so the refresh observes the post-reset state
/// instead of racing the provider's boundary propagation. Small on purpose —
/// it is latency on top of the reset, not polling policy.
const RESET_WAKE_GUARD_MS: i64 = 20_000;

/// The earliest reset-wake instant across every window the runtime currently
/// shows, or `None` when no reset justifies waking early. Eligibility reuses
/// the single ingestion-time plausibility rule
/// ([`crate::reset_plausibility::plausible_reset_at`]) re-evaluated against
/// the caller's clock, then requires the guarded wake instant itself to
/// still be in the future. That second check is what keeps one stale reset
/// timestamp from ever producing repeated wakeups: once its guard has
/// elapsed the stamp is spent and can never win a deadline again.
fn earliest_reset_wake_at(
    usages: &[ProviderUsageDto],
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let now_ms = now.timestamp_millis();
    usages
        .iter()
        .flat_map(|provider| provider.limits.iter())
        .filter_map(|limit| {
            let reset_at = limit.reset_at.as_deref()?;
            if !crate::reset_plausibility::plausible_reset_at(&limit.label, reset_at, now_ms) {
                return None;
            }
            let reset_ms = DateTime::parse_from_rfc3339(reset_at)
                .ok()?
                .timestamp_millis();
            let wake_ms = reset_ms.saturating_add(RESET_WAKE_GUARD_MS);
            if wake_ms <= now_ms {
                return None;
            }
            DateTime::from_timestamp_millis(wake_ms)
        })
        .min()
}

/// The effective next scheduler wake: the normal interval deadline, pulled
/// earlier by the earliest eligible reset wake when one is due sooner. A tie
/// keeps the normal tick — when both are due the one cycle the tick starts
/// already covers the reset.
fn combined_scheduler_deadline(
    interval_deadline: tokio::time::Instant,
    reset_deadline: Option<tokio::time::Instant>,
) -> (tokio::time::Instant, SchedulerOutcome) {
    match reset_deadline {
        Some(reset) if reset < interval_deadline => (reset, SchedulerOutcome::ResetWake),
        _ => (interval_deadline, SchedulerOutcome::Tick),
    }
}

/// True when the wall clock ran substantially longer than the monotonic
/// timer chunk — the signature of a suspend/resume inside the chunk.
///
/// This is the smallest reliable Windows mechanism the stack affords: Tauri
/// exposes no native resume event, and a power-broadcast hook would need a
/// message-only window plus a new Win32 dependency. Two properties make the
/// fallback safe either way: if the platform's monotonic clock *does* run
/// through sleep, the interval deadline simply fires on wake (a normal
/// tick), and if this predicate over-fires (e.g. a manual clock change),
/// the result is one coalesced, cadence- and cooldown-respecting cycle.
fn suspend_detected(wall_elapsed_ms: u64, chunk_ms: u64) -> bool {
    wall_elapsed_ms > chunk_ms.saturating_add(WAKE_DRIFT_TOLERANCE_MS)
}

/// The scheduler outcomes of one wait pass.
enum SchedulerOutcome {
    /// The interval deadline arrived: the normal scheduled refresh.
    Tick,
    /// A suspend was detected mid-chunk: the wake refresh.
    Wake,
    /// A known quota reset's post-reset guard elapsed before the interval:
    /// the reset-wake refresh.
    ResetWake,
    /// The refresh interval changed: recompute the deadline from now
    /// without a tick (the previous per-hook timer rebuild semantics).
    Recompute,
}

/// Starts the shared runtime: builds the production core, spawns the
/// interval scheduler (one immediate cycle, then one per interval), and
/// returns the handle for commands and tray actions.
pub fn start(app: AppHandle) -> RuntimeHandle {
    use tauri::Manager;

    let (interval_tx, interval_rx) = watch::channel(DEFAULT_INTERVAL_MINUTES);
    // Rust-owned quota history: one bounded JSON file in the app-data dir.
    // A missing directory is handled by the store's persist path; an
    // unresolvable data dir (never seen in practice) degrades to a runtime
    // without history persistence instead of blocking startup.
    let history = app
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| Arc::new(QuotaHistoryStore::open(dir.join(crate::history::HISTORY_FILE_NAME))));
    // Threshold notifications: one bounded JSON file, one evaluation point
    // in the runtime cycle, native delivery through the Tauri notification
    // plugin (best-effort; a failed toast never fails a cycle).
    let notifications = app.path().app_data_dir().ok().map(|dir| {
        let app = app.clone();
        Arc::new(NotificationLane::open(
            dir.join(crate::notifications::NOTIFICATIONS_FILE_NAME),
            Box::new(move |delivered| {
                crate::notifications::deliver_native(&app, delivered);
            }),
        ))
    });
    let last_good = app
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| Arc::new(crate::last_good::ProviderLastGoodStore::open(dir.join(crate::last_good::LAST_GOOD_FILE_NAME))));
    let handle = RuntimeHandle {
        core: Arc::new(
            RuntimeCore::new(production_specs(), DEFAULT_INTERVAL_MINUTES)
                .with_last_good_store(last_good.clone())
                .with_history_store(history.clone())
                .with_notification_lane(notifications.clone()),
        ),
        app,
        interval_tx,
        history,
        notifications,
        last_good,
    };
    let scheduler_handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval_rx = interval_rx;
        // One cycle right away replaces the per-webview "refresh on mount":
        // data is ready whether or not a window is open.
        scheduler_handle.request_refresh();
        loop {
            let minutes = *interval_rx.borrow();
            let interval_deadline =
                tokio::time::Instant::now() + Duration::from_secs(minutes.saturating_mul(60));
            let outcome;
            loop {
                // The reset-wake deadline is re-derived every pass: a
                // completed cycle can learn (or spend) a reset timestamp at
                // any moment, and each pass sleeps at most one chunk, so a
                // newly announced earlier reset is picked up within a chunk.
                let reset_deadline = scheduler_handle
                    .core
                    .earliest_reset_wake_at()
                    .map(|wake_at| {
                        // Eligibility already required a future instant; the
                        // floor only covers the evaluation-to-conversion race.
                        let until = (wake_at - Utc::now()).to_std().unwrap_or(Duration::ZERO);
                        tokio::time::Instant::now() + until
                    });
                let (deadline, deadline_outcome) =
                    combined_scheduler_deadline(interval_deadline, reset_deadline);
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    outcome = deadline_outcome;
                    break;
                }
                let chunk = (deadline - now).min(Duration::from_millis(SCHEDULER_CHUNK_MS));
                let wall_before = std::time::SystemTime::now();
                tokio::select! {
                    _ = tokio::time::sleep(chunk) => {
                        let wall_elapsed_ms =
                            wall_before.elapsed().map(|d| d.as_millis() as u64).unwrap_or(0);
                        if suspend_detected(wall_elapsed_ms, chunk.as_millis() as u64) {
                            outcome = SchedulerOutcome::Wake;
                            break;
                        }
                    }
                    _ = interval_rx.changed() => {
                        // Recompute the deadline from now, like the previous
                        // per-hook interval timer rebuild did.
                        outcome = SchedulerOutcome::Recompute;
                        break;
                    }
                }
            }
            match outcome {
                SchedulerOutcome::Tick => scheduler_handle.request_refresh(),
                SchedulerOutcome::Wake => {
                    // The wake refresh replaces the pending tick; the next
                    // interval starts counting from now.
                    scheduler_handle.request_refresh_on_wake();
                }
                SchedulerOutcome::ResetWake => {
                    // Like the suspend wake, the reset wake is a real cycle,
                    // so it also replaces the pending tick and the next
                    // interval starts counting from now.
                    scheduler_handle.request_refresh_on_reset_wake();
                }
                SchedulerOutcome::Recompute => {}
            }
        }
    });
    handle
}

// ---------- tauri commands ----------

#[tauri::command]
pub fn get_runtime_snapshot(handle: tauri::State<RuntimeHandle>) -> RuntimeSnapshot {
    handle.snapshot()
}

#[tauri::command]
pub fn request_refresh(handle: tauri::State<RuntimeHandle>) {
    handle.request_refresh();
}

/// The webview `online` hook: JS is only a trigger; the shared runtime
/// owns the fetch. Reconnect bursts coalesce per-kind (see
/// [`AutoTrigger`]).
#[tauri::command]
pub fn request_refresh_on_reconnect(handle: tauri::State<RuntimeHandle>) {
    handle.request_refresh_on_reconnect();
}

#[tauri::command]
pub fn set_refresh_interval(handle: tauri::State<RuntimeHandle>, minutes: u64) {
    handle.set_refresh_interval(minutes);
}

/// The quota-notifications toggle (settings drawer). Forwards the persisted
/// TS setting to the runtime's lane on attach and on every change — the
/// lane stays the single notification owner.
#[tauri::command]
pub fn set_quota_notifications_enabled(handle: tauri::State<RuntimeHandle>, enabled: bool) {
    handle.set_notifications_enabled(enabled);
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zcode_reset::{ZCodeResetCard, ZCodeResetTarget};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::TimeZone;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap()
    }

    fn test_core(specs: Vec<ProviderSpec>) -> Arc<RuntimeCore> {
        Arc::new(RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        ))
    }

    /// A runtime core whose clock a test can move forward deterministically
    /// (no real sleeps for cooldown or cadence waits).
    fn manual_clock_core(
        specs: Vec<ProviderSpec>,
    ) -> (Arc<RuntimeCore>, Arc<Mutex<DateTime<Utc>>>) {
        manual_clock_core_from(specs, fixed_now())
    }

    fn manual_clock_core_from(
        specs: Vec<ProviderSpec>,
        start: DateTime<Utc>,
    ) -> (Arc<RuntimeCore>, Arc<Mutex<DateTime<Utc>>>) {
        let clock = Arc::new(Mutex::new(start));
        let getter_clock = clock.clone();
        let core = Arc::new(RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(move || *getter_clock.lock().unwrap()),
        ));
        (core, clock)
    }

    fn advance(clock: &Mutex<DateTime<Utc>>, ms: i64) {
        *clock.lock().unwrap() += chrono::Duration::milliseconds(ms);
    }

    fn ok_usage(id: &str, name: &str, percent: f64) -> ProviderUsageDto {
        ProviderUsageDto {
            id: id.to_string(),
            name: name.to_string(),
            status: "ok",
            health: ProviderHealth::Live,
            checked_at: fixed_now().to_rfc3339_opts(SecondsFormat::Millis, true),
            limits: vec![UsageLimitDto {
                label: "Weekly".to_string(),
                used_percent: percent,
                reset_at: None,
            }],
            account: None,
            plan_type: None,
            error: None,
            error_category: None,
            error_http_status: None,
            source_updated_at: None,
            data_freshness: None,
            reset_credits: None,
            zcode_reset_cards: None,
            zcode_plans: None,
            fallback_failure: None,
        }
    }

    // ---------- v0.7 Codex banked reset credits ----------

    fn codex_success_with_credits(
        banked: u32,
        applicable: Option<u32>,
        checked_at: &str,
        tail: &str,
    ) -> CodexUsage {
        CodexUsage {
            limits: vec![crate::codex::CodexLimitWindow {
                label: "Weekly".to_string(),
                used_percent: 12.5,
                reset_at: None,
            }],
            plan_type: Some("team".to_string()),
            account: Some(crate::codex::CodexAccount {
                account_hint: tail.to_string(),
            }),
            reset_credits: Some(CodexResetCredits {
                banked_credits: banked,
                currently_applicable: applicable,
                checked_at: checked_at.to_string(),
                source: crate::codex::RESET_CREDITS_SOURCE,
            }),
        }
    }

    #[test]
    fn normalize_codex_carries_account_bound_credits() {
        let checked_at = fixed_now().to_rfc3339_opts(SecondsFormat::Secs, true);
        let dto = normalize_codex(
            Ok(codex_success_with_credits(3, Some(0), &checked_at, "ab12")),
            fixed_now().timestamp_millis(),
        )
        .expect("codex success must normalize");
        assert_eq!(dto.health, ProviderHealth::Live);
        // Combined-contract identity: the same chatgpt:{hint} vocabulary the
        // failure stamps carry, so the last-good guard compares like-for-like
        // and one identity binds windows, credits, and retention.
        assert_eq!(
            dto.account.as_ref().and_then(|a| a.identity.as_deref()),
            Some("chatgpt:ab12")
        );
        let credits = dto.reset_credits.expect("credits must propagate");
        assert_eq!(credits.banked_credits, 3);
        assert_eq!(credits.currently_applicable, Some(0));
    }

    #[test]
    fn normalize_codex_without_credits_stays_live_and_unavailable() {
        let mut usage = codex_success_with_credits(
            3,
            Some(0),
            &fixed_now().to_rfc3339_opts(SecondsFormat::Secs, true),
            "ab12",
        );
        usage.reset_credits = None;
        let dto = normalize_codex(Ok(usage), fixed_now().timestamp_millis())
            .expect("codex success must normalize");
        assert_eq!(dto.health, ProviderHealth::Live);
        assert_eq!(dto.reset_credits, None);
    }

    #[test]
    fn retained_snapshot_keeps_fresh_credits_and_drops_stale() {
        let now = fixed_now();
        let fresh_at = now.to_rfc3339_opts(SecondsFormat::Secs, true);
        let stale_at = (now - chrono::Duration::minutes(16))
            .to_rfc3339_opts(SecondsFormat::Secs, true);
        let mut fresh = normalize_codex(
            Ok(codex_success_with_credits(3, Some(0), &fresh_at, "ab12")),
            fixed_now().timestamp_millis(),
        )
        .expect("codex success must normalize");
        trim_stale_reset_credits(&mut fresh, now);
        assert_eq!(
            fresh.reset_credits.as_ref().map(|c| c.banked_credits),
            Some(3),
            "fresh credits must survive retention"
        );
        let mut stale = normalize_codex(
            Ok(codex_success_with_credits(3, Some(0), &stale_at, "ab12")),
            fixed_now().timestamp_millis(),
        )
        .expect("codex success must normalize");
        trim_stale_reset_credits(&mut stale, now);
        assert_eq!(
            stale.reset_credits, None,
            "stale credits must stop reading as current"
        );
        // The guard is Codex-only: other providers never carry the field and
        // must pass through untouched.
        let mut other = ok_usage("zai", "Z.ai", 10.0);
        trim_stale_reset_credits(&mut other, now);
        assert_eq!(other.reset_credits, None);
    }

    // ---------- ZCode reset cards (Z.ai entry, passive observation) ----------

    /// Two five-hour cards (one with, one without a usable expiry) and one
    /// weekly card: the minimal shape that exercises both buckets.
    fn observed_reset_cards(observed_at: DateTime<Utc>) -> ZCodeResetStatus {
        ZCodeResetStatus {
            five_hour_cards: vec![
                ZCodeResetCard {
                    target: ZCodeResetTarget::FiveHour,
                    expires_at: Some("2026-10-16T08:00:00Z".to_string()),
                },
                ZCodeResetCard {
                    target: ZCodeResetTarget::FiveHour,
                    expires_at: None,
                },
            ],
            weekly_cards: vec![ZCodeResetCard {
                target: ZCodeResetTarget::Weekly,
                expires_at: Some("2026-10-23T08:00:00Z".to_string()),
            }],
            observed_at,
        }
    }

    fn zai_quota_ok() -> Result<ZaiUsage, ProviderError> {
        Ok(ZaiUsage {
            limits: vec![crate::zai::ZaiLimitWindow {
                label: "5-hour".to_string(),
                used_percent: 10.0,
                reset_at: None,
            }],
            account: None,
        })
    }

    #[test]
    fn normalize_zai_carries_reset_cards_on_the_quota_entry() {
        let dto = normalize_zai(
            zai_quota_ok(),
            Some(observed_reset_cards(fixed_now())),
            None,
            fixed_now().timestamp_millis(),
        )
        .expect("quota success must normalize");
        assert_eq!(dto.health, ProviderHealth::Live);
        let cards = dto.zcode_reset_cards.expect("cards must ride the entry");
        assert_eq!(cards.five_hour_cards.len(), 2);
        assert_eq!(cards.weekly_cards.len(), 1);
        assert_eq!(cards.five_hour_cards[0].target, ZCodeResetTarget::FiveHour);
        assert_eq!(cards.five_hour_cards[0].expires_at.as_deref(), Some("2026-10-16T08:00:00Z"));
        // The card without a usable stamp still counts, expiry-less.
        assert_eq!(cards.five_hour_cards[1].expires_at, None);
    }

    /// Independence (reset-card side): a failed observation arrives as `None`
    /// and never fails or degrades the quota refresh.
    #[test]
    fn zai_reset_card_failure_never_fails_the_quota_refresh() {
        let dto = normalize_zai(zai_quota_ok(), None, None, fixed_now().timestamp_millis())
            .expect("quota success must normalize regardless of the card observation");
        assert_eq!(dto.health, ProviderHealth::Live);
        assert_eq!(dto.limits.len(), 1);
        assert_eq!(dto.zcode_reset_cards, None);
    }

    /// Independence (quota side): a failed quota refresh propagates the quota
    /// verdict even when fresh card data exists — cards never stand in for
    /// quota, and they are dropped for the cycle (the retained last-good
    /// entry keeps serving its own still-fresh copy).
    #[test]
    fn zai_quota_failure_is_not_masked_by_fresh_reset_cards() {
        let failure = normalize_zai(
            Err(ProviderError::transient("network", "offline")),
            Some(observed_reset_cards(fixed_now())),
            None,
            fixed_now().timestamp_millis(),
        )
        .unwrap_err();
        assert_eq!(failure.code, "network");
        assert_eq!(failure.transient, Some(true));
    }

    #[test]
    fn retained_zai_snapshot_keeps_fresh_cards_and_drops_stale() {
        let now = fixed_now();
        let mut fresh = normalize_zai(
            zai_quota_ok(),
            Some(observed_reset_cards(now - chrono::Duration::minutes(10))),
            None,
            now.timestamp_millis(),
        )
        .expect("quota success must normalize");
        trim_stale_zcode_reset_cards(&mut fresh, now);
        assert!(
            fresh.zcode_reset_cards.is_some(),
            "fresh cards must survive retention"
        );

        let mut stale = normalize_zai(
            zai_quota_ok(),
            Some(observed_reset_cards(
                now - chrono::Duration::seconds(zcode_reset::OBSERVATION_TTL_SECS + 60),
            )),
            None,
            now.timestamp_millis(),
        )
        .expect("quota success must normalize");
        trim_stale_zcode_reset_cards(&mut stale, now);
        assert_eq!(
            stale.zcode_reset_cards, None,
            "stale cards must stop reading as available"
        );

        // The guard is Z.ai-only: other providers never carry the field and
        // must pass through untouched.
        let mut other = ok_usage("openai-codex", "OpenAI / Codex", 10.0);
        trim_stale_zcode_reset_cards(&mut other, now);
        assert_eq!(other.zcode_reset_cards, None);
    }

    /// The wire contract keeps the grant semantics intact: both card buckets
    /// with their per-card target and expiry, never flattened to a bare
    /// count, and the internal observation stamp never serializes.
    #[test]
    fn zcode_reset_card_wire_preserves_targets_and_expiry_without_flattening() {
        let dto = normalize_zai(
            zai_quota_ok(),
            Some(observed_reset_cards(fixed_now())),
            None,
            fixed_now().timestamp_millis(),
        )
        .expect("quota success must normalize");
        let wire = serde_json::to_string(&dto).unwrap();
        assert!(wire.contains("\"zcodeResetCards\":"), "wire: {wire}");
        assert!(wire.contains("\"fiveHourCards\":["), "wire: {wire}");
        assert!(wire.contains("\"weeklyCards\":["), "wire: {wire}");
        assert!(wire.contains("\"target\":\"fiveHour\""), "wire: {wire}");
        assert!(wire.contains("\"target\":\"weekly\""), "wire: {wire}");
        assert!(wire.contains("\"expiresAt\":\"2026-10-16T08:00:00Z\""), "wire: {wire}");
        // No flattened vocabulary, no credential-shaped field, no internal stamp.
        assert!(!wire.contains("availableResets"), "wire: {wire}");
        assert!(!wire.contains("observedAt"), "wire: {wire}");

        // Without an observation the field is omitted entirely.
        let bare = normalize_zai(zai_quota_ok(), None, None, fixed_now().timestamp_millis())
            .expect("quota success must normalize");
        let wire = serde_json::to_string(&bare).unwrap();
        assert!(!wire.contains("zcodeResetCards"), "wire: {wire}");
    }

    fn observed_plan_observation(now: chrono::DateTime<chrono::Utc>) -> ZCodePlansObservation {
        ZCodePlansObservation {
            plans: vec![zcode_plans::ZCodePlan {
                plan_id: "plan-trust-build".to_string(),
                user_plan_id: Some("up-1".to_string()),
                name: Some("ZCode Trust Build".to_string()),
                status: "active".to_string(),
                ends_at: Some("2026-10-26T07:33:20Z".to_string()),
                balances: vec![zcode_plans::ZCodePlanBalance {
                    user_plan_id: Some("up-1".to_string()),
                    entitlement_id: Some("ent-token".to_string()),
                    bucket_id: Some("bucket-1".to_string()),
                    model: Some("GLM-5.3-Flash".to_string()),
                    meter: None,
                    unit: Some("token".to_string()),
                    limit: Some(100_000_000.0),
                    used: Some(5_200_000.0),
                    remaining: Some(94_800_000.0),
                    period: Some("one_time".to_string()),
                    period_end: None,
                    expires_at: Some("2026-10-26T07:33:20Z".to_string()),
                }],
            }],
            observed_at: now,
        }
    }

    #[test]
    fn normalize_zai_carries_plan_observation_on_the_quota_entry() {
        let dto = normalize_zai(
            zai_quota_ok(),
            None,
            Some(observed_plan_observation(fixed_now())),
            fixed_now().timestamp_millis(),
        )
        .expect("quota success must normalize");
        assert_eq!(dto.health, ProviderHealth::Live);
        let plans = dto.zcode_plans.expect("plans must ride the entry");
        assert_eq!(plans.plans.len(), 1);
        let plan = &plans.plans[0];
        assert_eq!(plan.plan_id, "plan-trust-build");
        assert_eq!(plan.name.as_deref(), Some("ZCode Trust Build"));
        assert_eq!(plan.balances.len(), 1);
        // Absolute balances ride unchanged — never squeezed into a percent.
        assert_eq!(plan.balances[0].limit, Some(100_000_000.0));
        assert_eq!(plan.balances[0].remaining, Some(94_800_000.0));
        assert_eq!(plan.balances[0].unit.as_deref(), Some("token"));
    }

    /// Independence (supplemental side): a failed plan observation arrives
    /// as `None` and never fails or degrades the quota refresh — with or
    /// without the other supplemental observation succeeding.
    #[test]
    fn zai_plan_observation_failure_never_fails_the_quota_refresh() {
        let dto = normalize_zai(zai_quota_ok(), None, None, fixed_now().timestamp_millis())
            .expect("quota success must normalize regardless of the plan observation");
        assert_eq!(dto.health, ProviderHealth::Live);
        assert_eq!(dto.limits.len(), 1);
        assert_eq!(dto.zcode_plans, None);
        assert_eq!(dto.zcode_reset_cards, None);
    }

    /// Independence (quota side): a failed quota refresh propagates the
    /// quota verdict even when fresh plan data exists — plans never stand
    /// in for quota, and they are dropped for the cycle.
    #[test]
    fn zai_quota_failure_is_not_masked_by_fresh_plans() {
        let failure = normalize_zai(
            Err(ProviderError::transient("network", "offline")),
            None,
            Some(observed_plan_observation(fixed_now())),
            fixed_now().timestamp_millis(),
        )
        .unwrap_err();
        assert_eq!(failure.code, "network");
        assert_eq!(failure.transient, Some(true));
    }

    #[test]
    fn retained_zai_snapshot_keeps_fresh_plans_and_drops_stale() {
        let now = fixed_now();
        let mut fresh = normalize_zai(
            zai_quota_ok(),
            None,
            Some(observed_plan_observation(now - chrono::Duration::minutes(10))),
            now.timestamp_millis(),
        )
        .expect("quota success must normalize");
        trim_stale_zcode_plans(&mut fresh, now);
        assert!(
            fresh.zcode_plans.is_some(),
            "fresh plan observations must survive retention"
        );

        let mut stale = normalize_zai(
            zai_quota_ok(),
            None,
            Some(observed_plan_observation(
                now - chrono::Duration::seconds(zcode_plans::OBSERVATION_TTL_SECS + 60),
            )),
            now.timestamp_millis(),
        )
        .expect("quota success must normalize");
        trim_stale_zcode_plans(&mut stale, now);
        assert_eq!(
            stale.zcode_plans, None,
            "stale plan observations must stop reading as available"
        );

        // The guard is Z.ai-only: other providers never carry the field.
        let mut other = ok_usage("openai-codex", "OpenAI / Codex", 10.0);
        trim_stale_zcode_plans(&mut other, now);
        assert_eq!(other.zcode_plans, None);
    }

    /// The wire contract keeps plan-grouped absolute balances intact: the
    /// plan grouping, the stable identifiers, and the absolute values are
    /// all present, the internal stamp is not, and a cycle without an
    /// observation omits the field entirely.
    #[test]
    fn zcode_plan_wire_preserves_plan_grouping_and_absolute_values() {
        let dto = normalize_zai(
            zai_quota_ok(),
            None,
            Some(observed_plan_observation(fixed_now())),
            fixed_now().timestamp_millis(),
        )
        .expect("quota success must normalize");
        let wire = serde_json::to_string(&dto).unwrap();
        assert!(wire.contains("\"zcodePlans\":"), "wire: {wire}");
        assert!(wire.contains("\"planId\":\"plan-trust-build\""), "wire: {wire}");
        assert!(wire.contains("\"userPlanId\":\"up-1\""), "wire: {wire}");
        assert!(wire.contains("\"entitlementId\":\"ent-token\""), "wire: {wire}");
        assert!(wire.contains("\"bucketId\":\"bucket-1\""), "wire: {wire}");
        assert!(wire.contains("\"remaining\":94800000.0"), "wire: {wire}");
        assert!(wire.contains("\"unit\":\"token\""), "wire: {wire}");
        assert!(wire.contains("\"period\":\"one_time\""), "wire: {wire}");
        assert!(!wire.contains("observedAt"), "wire: {wire}");

        let bare = normalize_zai(zai_quota_ok(), None, None, fixed_now().timestamp_millis())
            .expect("quota success must normalize");
        let wire = serde_json::to_string(&bare).unwrap();
        assert!(!wire.contains("zcodePlans"), "wire: {wire}");
    }

    fn counting_spec(
        kind: ProviderKind,
        calls: Arc<AtomicUsize>,
        outcome: impl Fn(usize) -> Result<ProviderUsageDto, ProviderFailure> + Send + Sync + 'static,
    ) -> ProviderSpec {
        ProviderSpec {
            kind,
            fetch: Arc::new(move || {
                let calls = calls.clone();
                let nth = calls.fetch_add(1, Ordering::SeqCst) + 1;
                let result = outcome(nth);
                Box::pin(async move { result }) as FetchFuture
            }),
        }
    }

    fn transient_failure(code: &str) -> ProviderFailure {
        ProviderFailure {
            code: code.to_string(),
            message: "network down".to_string(),
            http_status: None,
            transient: Some(true),
            retry_after_ms: None,
            identity: None,
            transport_timeout: false,
        }
    }

    fn permanent_failure() -> ProviderFailure {
        ProviderFailure {
            code: "auth_expired".to_string(),
            message: "session expired".to_string(),
            http_status: None,
            transient: Some(false),
            retry_after_ms: None,
            identity: None,
            transport_timeout: false,
        }
    }

    fn rate_limited_failure(retry_after_ms: Option<u64>) -> ProviderFailure {
        ProviderFailure {
            code: "unexpected_response".to_string(),
            message: "rate limited".to_string(),
            http_status: Some(429),
            transient: Some(true),
            retry_after_ms,
            identity: None,
            transport_timeout: false,
        }
    }

    fn server_error_failure(retry_after_ms: Option<u64>) -> ProviderFailure {
        ProviderFailure {
            code: "unexpected_response".to_string(),
            message: "server exploded".to_string(),
            http_status: Some(503),
            transient: Some(true),
            retry_after_ms,
            identity: None,
            transport_timeout: false,
        }
    }

    /// A budget-burning transport stall: transient, but terminal for the
    /// cycle that observed it (the bounded-retry rule's timeout exception).
    fn transport_timeout_failure() -> ProviderFailure {
        ProviderFailure {
            code: "network".to_string(),
            message: "operation timed out".to_string(),
            http_status: None,
            transient: Some(true),
            retry_after_ms: None,
            identity: None,
            transport_timeout: true,
        }
    }

    #[allow(dead_code)]
    async fn collect_chain(core: &Arc<RuntimeCore>) -> Vec<RuntimeSnapshot> {
        let core = core.clone();
        let snapshots = Arc::new(Mutex::new(Vec::<RuntimeSnapshot>::new()));
        let sink = snapshots.clone();
        core.run_chain(move |event| {
            if let RuntimeEvent::Snapshot(snapshot) = event {
                sink.lock().unwrap().push(snapshot);
            }
        })
        .await;
        let snapshots = snapshots.lock().unwrap().clone();
        snapshots
    }

    // 13. provider order remains stable / 11. simulated providers excluded /
    // 12. Grok remains production
    #[test]
    fn production_registry_matches_the_expected_set_and_order() {
        let kinds: Vec<ProviderKind> = production_specs().iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ProviderKind::Codex,
                ProviderKind::Zai,
                ProviderKind::OpenCodeGo,
                ProviderKind::Antigravity,
                ProviderKind::Grok,
            ]
        );
        let ids: Vec<&str> = kinds.iter().map(|k| k.id()).collect();
        assert_eq!(
            ids,
            ["openai-codex", "zai", "opencode-go", "antigravity", "grok"]
        );
        assert!(ids.contains(&"grok"), "Grok must remain production");
        // No simulated providers: the production runtime never carries the
        // mock adapters (Claude stays a dev/test-only mock outside Rust).
        assert!(!serde_json::to_string(&ids).unwrap().contains("claude"));
    }

    // 1. one cycle fetches all registered providers once
    #[tokio::test]
    async fn one_cycle_fetches_every_provider_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| {
            Ok(ok_usage("openai-codex", "OpenAI / Codex", nth as f64))
        });
        let core = test_core(vec![spec, grok_spec()]);
        core.run_cycle().await;
        let snapshot = core.snapshot();
        assert_eq!(snapshot.providers.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    fn grok_spec() -> ProviderSpec {
        ProviderSpec {
            kind: ProviderKind::Grok,
            fetch: Arc::new(|| {
                Box::pin(async { Ok(ok_usage("grok", "Grok (xAI)", 10.0)) }) as FetchFuture
            }),
        }
    }

    // 2./3. concurrent refresh requests coalesce; no parallel second cycle
    #[tokio::test]
    async fn concurrent_refresh_requests_coalesce_into_one_follow_up() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let gate_fetch = gate.clone();
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                let gate = gate_fetch.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let _permit = gate.acquire_owned().await.unwrap();
                    Ok(ok_usage("openai-codex", "OpenAI / Codex", 42.0)) as Result<_, ProviderFailure>
                }) as FetchFuture
            }),
        };
        let core = test_core(vec![spec]);
        assert!(core.try_begin_cycle(), "first request starts a cycle");
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });

        // Wait until the cycle's fetch is parked on the gate.
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // Requests landing mid-cycle coalesce: no second parallel cycle, and
        // at most one pending follow-up no matter how many requests arrive.
        assert!(!core.try_begin_cycle());
        assert!(!core.try_begin_cycle());
        assert!(!core.try_begin_cycle());

        // Release both cycles' fetches (the coalesced follow-up included).
        gate.add_permits(2);
        let _ = chain.await;

        // Exactly one follow-up ran: two cycles, two fetches total.
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let snapshot = core.snapshot();
        assert!(!snapshot.cycle_in_flight);
        assert_eq!(snapshot.seq, 2);
    }

    // 4. one transient retry only
    #[tokio::test]
    async fn transient_failure_is_retried_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Err(transient_failure("network")),
            _ => Ok(ok_usage("openai-codex", "OpenAI / Codex", 7.0)),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(core.snapshot().providers[0].status, "ok");
    }

    // 4a. a transport timeout is terminal for the cycle that observed it:
    // no fast in-cycle retry (the stall that consumed the whole request
    // budget will not clear 750 ms later), but the next normal cycle
    // retries fresh — no cooldown, no persisted penalty, and last-good
    // retention behaves exactly as for any other failure.
    #[tokio::test]
    async fn transport_timeout_failure_is_terminal_for_its_cycle() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("openai-codex", "OpenAI / Codex", 33.0)),
            2 => Err(transport_timeout_failure()),
            _ => Ok(ok_usage("openai-codex", "OpenAI / Codex", 34.0)),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await; // success seeds last good
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        core.run_cycle().await; // the timeout cycle: exactly one fetch, no burn
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a transport timeout must not burn the fast in-cycle retry"
        );
        let failed = &core.snapshot().providers[0];
        assert_eq!(failed.status, "error");
        assert_eq!(failed.limits, ok_usage("openai-codex", "OpenAI / Codex", 33.0).limits);
        assert_eq!(
            failed.error.as_deref(),
            Some("Refresh failed: operation timed out (network)")
        );

        core.run_cycle().await; // the next cycle retries fresh — and succeeds
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "the next cycle must fetch again (timeout is not a cooldown)"
        );
        assert_eq!(core.snapshot().providers[0].status, "ok");
    }

    // 4b. a fast transport failure (unmarked) still gets the one
    // bounded retry — the timeout exception must not swallow it.
    #[tokio::test]
    async fn unmarked_fast_transport_failure_keeps_the_one_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::OpenCodeGo, calls.clone(), |nth| match nth {
            1 => Err(transient_failure("network")),
            _ => Ok(ok_usage("opencode", "OpenCode Go", 7.0)),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a fast transport refusal keeps the bounded retry"
        );
    }

    // 5. non-transient errors do not retry
    #[tokio::test]
    async fn permanent_failure_is_not_retried() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Zai, calls.clone(), |_| {
            Err(permanent_failure())
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let provider = &core.snapshot().providers[0];
        assert_eq!(provider.status, "error");
        // Without last-good data the bare described message stands (the
        // "Refresh failed:" prefix is reserved for retained last-good data).
        assert_eq!(
            provider.error.as_deref(),
            Some("session expired (auth_expired)")
        );
    }

    // 6./7. last-good retained on failure; retained value stays errored
    #[tokio::test]
    async fn last_good_is_retained_and_marked_errored_on_failure() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("openai-codex", "OpenAI / Codex", 33.0)),
            _ => Err(transient_failure("network")),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await; // success seeds last good
        let good_checked_at = core.snapshot().providers[0].checked_at.clone();

        core.run_cycle().await; // failure (after its one retry) retains it
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.status, "error");
        assert_eq!(retained.limits, ok_usage("openai-codex", "OpenAI / Codex", 33.0).limits);
        assert_eq!(retained.checked_at, good_checked_at, "retained checkedAt stays at the original fetch time");
        assert_eq!(
            retained.error.as_deref(),
            Some("Refresh failed: network down (network)")
        );
        // The retained entry must never read as healthy.
        assert_ne!(retained.status, "ok");
        assert_ne!(retained.status, "stale");
    }

    // ---- runtime panic containment ----
    //
    // One provider panic or one cycle-body panic must never silently remove
    // a provider from a cycle, and never permanently stop automatic
    // refreshes. Panics become the ordinary structured failure path.

    /// A fetch future that panics when polled — the shape of arbitrary
    /// provider backend code blowing up mid-refresh.
    async fn panicking_fetch() -> Result<ProviderUsageDto, ProviderFailure> {
        panic!("boom: broken provider state");
    }

    // 1. a panicking fetch closure becomes a structured failure and stays in
    //    the cycle; siblings still succeed in the same cycle
    #[tokio::test]
    async fn panicking_fetch_becomes_a_structured_failure_and_stays_in_the_cycle() {
        let panic_spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(|| Box::pin(panicking_fetch()) as FetchFuture),
        };
        let zai_calls = Arc::new(AtomicUsize::new(0));
        let ok_spec = counting_spec(ProviderKind::Zai, zai_calls.clone(), |_| {
            Ok(ok_usage("zai", "Z.ai", 12.0))
        });
        let core = test_core(vec![panic_spec, ok_spec]);
        core.run_cycle().await;
        let snapshot = core.snapshot();
        // The panicked provider is projected, not dropped: both entries, in
        // spec order.
        assert_eq!(
            snapshot.providers.len(),
            2,
            "a panicked provider must not disappear from the cycle"
        );
        assert_eq!(snapshot.providers[0].id, "openai-codex");
        assert_eq!(snapshot.providers[1].id, "zai");
        let panicked = &snapshot.providers[0];
        assert_eq!(panicked.status, "error");
        assert_eq!(panicked.health, ProviderHealth::Error);
        // The panic payload is never forwarded: the panic message is the one
        // string that reaches the WebView unscrubbed, and a payload can
        // interpolate runtime values (credential material included). Only the
        // fixed containment sentence rides the snapshot, still in the shared
        // `unexpected` category — no parallel error model.
        let error = panicked.error.as_deref().unwrap_or_default();
        assert!(
            error.contains("provider fetch panicked (payload withheld)"),
            "got: {error}"
        );
        assert!(!error.contains("boom"), "payload text leaked: {error}");
        assert_eq!(panicked.error_category.as_deref(), Some("unexpected"));
        assert!(panicked.limits.is_empty(), "no quota data can be invented");
        // The sibling provider still succeeded in the same cycle.
        let sibling = &snapshot.providers[1];
        assert_eq!(sibling.status, "ok");
        assert_eq!(sibling.health, ProviderHealth::Live);
        assert!(snapshot.cycle_succeeded);
        assert_eq!(zai_calls.load(Ordering::SeqCst), 1);
    }

    // 2. last-good retention applies to the panic failure; no retry burn, no
    //    cooldown, and the provider recovers on the next cycle
    #[tokio::test]
    async fn panicking_fetch_keeps_last_good_and_recovers_without_retry_or_cooldown() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                Box::pin(async move {
                    let nth = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    match nth {
                        1 => Ok(ok_usage("openai-codex", "OpenAI / Codex", 33.0)),
                        2 => panic!("boom on the second cycle"),
                        _ => Ok(ok_usage("openai-codex", "OpenAI / Codex", 40.0)),
                    }
                }) as FetchFuture
            }),
        };
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await; // success seeds last good
        let good_checked_at = core.snapshot().providers[0].checked_at.clone();
        advance(&clock, 60_000);

        core.run_cycle().await; // the panicking cycle
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.status, "error");
        assert_eq!(
            retained.limits,
            ok_usage("openai-codex", "OpenAI / Codex", 33.0).limits,
            "the panic failure retains last-good like any failed refresh"
        );
        assert_eq!(
            retained.checked_at, good_checked_at,
            "retained checkedAt stays at the original fetch time"
        );
        let error = retained.error.as_deref().unwrap_or_default();
        assert!(error.starts_with("Refresh failed:"), "got: {error}");
        assert!(
            error.contains("provider fetch panicked (payload withheld)"),
            "got: {error}"
        );
        assert!(
            !error.contains("boom"),
            "panic payload text must not reach the snapshot: {error}"
        );
        // A panic is a local defect: no retry burn, no cooldown.
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the panicking cycle must not burn the transient retry"
        );
        advance(&clock, 60_000);

        core.run_cycle().await; // the next cycle simply fetches again
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "no cooldown may defer the provider after a contained panic"
        );
        let recovered = &core.snapshot().providers[0];
        assert_eq!(recovered.status, "ok");
        assert_eq!(recovered.health, ProviderHealth::Live);
        assert_eq!(recovered.error, None);
        assert_eq!(recovered.limits[0].used_percent, 40.0);
    }

    // 3. a panic inside the cycle body itself (the notification lane's
    //    delivery sink) cannot wedge the in-flight flag or kill the chain:
    //    the snapshot still closes the loading span and the next refresh
    //    still runs to completion
    #[tokio::test]
    async fn chain_survives_a_panicking_cycle_body_and_keeps_refreshing() {
        let lane_path = std::env::temp_dir().join(format!(
            "runtime-panic-lane-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        ));
        let _ = std::fs::remove_file(&lane_path);
        let lane = Arc::new(crate::notifications::NotificationLane::open(
            lane_path.clone(),
            Box::new(|_: &[crate::notifications::QuotaNotification]| {
                panic!("native delivery exploded");
            }),
        ));
        lane.set_enabled(true);

        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            let percent = match nth {
                1 => 50.0,
                _ => 85.0,
            };
            Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                percent,
                Some(&week_ahead),
            ))
        });
        let clock = Arc::new(Mutex::new(fixed_now()));
        let getter_clock = clock.clone();
        let core = Arc::new(
            RuntimeCore::with_injections(
                vec![spec],
                5,
                Box::new(|| 0),
                Box::new(move || *getter_clock.lock().unwrap()),
            )
            .with_notification_lane(Some(lane)),
        );

        // Cycle 1 through the chain: 50% — the crossing baseline, no
        // delivery, no panic.
        let snapshots: Arc<Mutex<Vec<RuntimeSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let sink = snapshots.clone();
            core.run_chain(move |event| {
                if let RuntimeEvent::Snapshot(snapshot) = event {
                    sink.lock().unwrap().push(snapshot);
                }
            })
            .await;
        }
        advance(&clock, 60_000);

        // Cycle 2: 85% crosses NEAR LIMIT — the delivery sink panics inside
        // the cycle. The chain must survive: the flag is not wedged and the
        // snapshot still closes the loading span (showing the last
        // completed cycle's state, since the panicked cycle's results were
        // discarded mid-cycle).
        {
            let sink = snapshots.clone();
            core.run_chain(move |event| {
                if let RuntimeEvent::Snapshot(snapshot) = event {
                    sink.lock().unwrap().push(snapshot);
                }
            })
            .await;
        }
        let all = snapshots.lock().unwrap().clone();
        assert_eq!(
            all.len(),
            2,
            "the chain must finish despite the panicked cycle"
        );
        let after_panic = &all[1];
        assert!(
            !after_panic.cycle_in_flight,
            "the panic must not wedge the in-flight flag"
        );
        assert_eq!(after_panic.providers[0].status, "ok");
        advance(&clock, 60_000);

        // The scheduler's next refresh: the crossing was already recorded
        // fired-before-deliver, so the sink is not called again and the
        // cycle completes normally — collection continues after the panic.
        core.run_cycle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "the scheduler must still fetch after the panicked cycle"
        );
        let snapshot = core.snapshot();
        assert_eq!(snapshot.providers[0].health, ProviderHealth::Live);
        assert_eq!(snapshot.providers[0].limits[0].used_percent, 85.0);
        assert!(!snapshot.cycle_in_flight);
        let _ = std::fs::remove_file(&lane_path);
    }

    // ---- account guard (MIC-297 follow-up) ----
    //
    // OpenCode Go reads the one stored credential each cycle. When that
    // credential changes between cycles (re-auth, removed account), the
    // retained last-good of the OLD account must never stand in for the NEW
    // one on a failed refresh — not even marked errored.

    fn opencode_usage(percent: f64, hint: &str) -> ProviderUsageDto {
        let mut usage = ok_usage("opencode-go", "OpenCode Go", percent);
        usage.account = Some(AccountAttributionDto {
            label: format!("key \u{2022}\u{2022}{hint}"),
            note: None,
            identity: Some(format!("key:{hint}")),
        });
        usage
    }

    #[tokio::test]
    async fn opencode_last_good_is_retained_when_the_attempted_account_matches() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::OpenCodeGo, calls.clone(), |nth| match nth {
            1 => Ok(opencode_usage(100.0, "avmF")),
            _ => Err(transient_failure("network")),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await; // success seeds last good
        core.run_cycle().await; // same-account failure retains it
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.status, "error");
        assert_eq!(
            retained.account,
            opencode_usage(100.0, "avmF").account,
            "the same account's last-good is retained"
        );
        assert_eq!(retained.limits, opencode_usage(100.0, "avmF").limits);
    }

    #[tokio::test]
    async fn opencode_last_good_is_retained_without_a_resolved_identity() {
        // The credential could not be resolved at all before the failure
        // (no identity on the failure): retention behavior is unchanged.
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::OpenCodeGo, calls.clone(), |nth| match nth {
            1 => Ok(opencode_usage(100.0, "avmF")),
            _ => Err(transient_failure("network")),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await;
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.status, "error");
        assert_eq!(retained.account, opencode_usage(100.0, "avmF").account);
    }

    #[tokio::test]
    async fn opencode_last_good_is_dropped_when_the_stored_account_changed() {
        // First cycle reports the old exhausted account; the stored
        // credential then changes (re-auth to the remaining account) and the
        // next refresh fails after resolving the new credential. The old
        // account's snapshot must not survive the failure.
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::OpenCodeGo, calls.clone(), |nth| match nth {
            1 => Ok(opencode_usage(100.0, "avmF")),
            _ => Err({
                let mut failure = transient_failure("network");
                failure.identity = Some("key:yZw0".to_string());
                failure
            }),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await;
        let entry = &core.snapshot().providers[0];
        assert_eq!(entry.status, "error");
        assert!(
            entry.limits.is_empty(),
            "another account's last-good must not stand in"
        );
        assert!(
            entry.account.is_none(),
            "no cross-account attribution survives the guard"
        );
    }

    #[test]
    fn failure_identity_maps_from_the_provider_error_hint() {
        let error = ProviderError::transient("network", "offline")
            .with_identity_hint(Some("key:avmF".to_string()));
        assert_eq!(
            failure_from_provider_error(error).identity,
            Some("key:avmF".to_string())
        );
        assert_eq!(
            failure_from_provider_error(ProviderError::transient("network", "offline")).identity,
            None
        );
    }

    #[test]
    fn opencode_identity_formats_match_between_success_and_failure_paths() {
        // build_opencode_attribution and the OpenCode backend both derive
        // "key:<hint>"; a drift between them would silently disable the
        // account guard.
        let attribution = build_opencode_attribution(Some("avmF")).unwrap();
        assert_eq!(attribution.identity.as_deref(), Some("key:avmF"));
    }

    // ---- identity hardening (R1): the same guard across every provider ----
    //
    // Codex, Z.ai, and Grok now stamp the masked identity of the credential
    // the failed refresh actually attempted (Codex: the auth.json
    // account-id tail; Z.ai: the candidate key in flight; Grok: the
    // store-agnostic masked account id). The guard semantics are the
    // runtime's own and identical for all five providers: same attempted
    // identity → retained last-good stays eligible; a different one →
    // dropped; an unresolved identity → conservative retention (unchanged).

    fn codex_usage(percent: f64, hint: &str) -> ProviderUsageDto {
        let mut usage = ok_usage("openai-codex", "OpenAI / Codex", percent);
        usage.account = build_codex_attribution(Some(&codex::CodexAccount {
            account_hint: hint.to_string(),
        }));
        usage
    }

    fn zai_usage(percent: f64, hint: &str) -> ProviderUsageDto {
        let mut usage = ok_usage("zai", "Z.ai", percent);
        usage.account = build_zai_attribution(Some(&zai::ZaiAccount {
            key_hint: hint.to_string(),
        }));
        usage
    }

    fn grok_usage(percent: f64, id: &str) -> ProviderUsageDto {
        let mut usage = ok_usage("grok", "Grok (xAI)", percent);
        usage.account = build_grok_attribution(Some(&grok::GrokAccount {
            id: id.to_string(),
            source: "opencodex".to_string(),
        }));
        usage
    }

    fn failure_with_identity(
        mut failure: ProviderFailure,
        identity: Option<&str>,
    ) -> ProviderFailure {
        failure.identity = identity.map(str::to_string);
        failure
    }

    fn codex_failure(identity: Option<&str>) -> ProviderFailure {
        // Built through the real backend → failure mapping, so the test pins
        // the identity hand-off, not a hand-written ProviderFailure.
        failure_with_identity(
            failure_from_provider_error(ProviderError::new("auth_expired", "session expired")),
            identity,
        )
    }

    fn zai_failure(identity: Option<&str>) -> ProviderFailure {
        failure_with_identity(
            failure_from_provider_error(ProviderError::new("auth_invalid", "rejected")),
            identity,
        )
    }

    fn grok_failure(identity: Option<&str>) -> ProviderFailure {
        failure_with_identity(
            failure_from_grok_error(GrokError {
                code: "auth_failed".to_string(),
                message: "rejected".to_string(),
                http_status: Some(401),
                transient: Some(false),
                retry_after_ms: None,
                identity_hint: None,
                transport_timeout: false,
            }),
            identity,
        )
    }

    // 1. Codex same-account failure retention
    #[tokio::test]
    async fn codex_last_good_is_retained_for_the_same_account() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(codex_usage(38.0, "56789abc")),
            _ => Err(codex_failure(Some("chatgpt:56789abc"))),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await;
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.status, "error");
        assert_eq!(
            retained.account,
            codex_usage(38.0, "56789abc").account,
            "the same account's last-good is retained"
        );
        assert_eq!(retained.limits, codex_usage(38.0, "56789abc").limits);
    }

    // 2. Codex account-change drop (where identity is available)
    #[tokio::test]
    async fn codex_last_good_is_dropped_when_the_account_changed() {
        // auth.json now holds another ChatGPT account and the refresh failed
        // after resolving it: the previous account's windows must not stand in.
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(codex_usage(38.0, "56789abc")),
            _ => Err(codex_failure(Some("chatgpt:0badf00d"))),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await;
        let entry = &core.snapshot().providers[0];
        assert_eq!(entry.status, "error");
        assert!(
            entry.limits.is_empty(),
            "another account's last-good must not stand in"
        );
        assert!(entry.account.is_none(), "no cross-account attribution survives");
    }

    // 3. Z.ai same-credential retention
    #[tokio::test]
    async fn zai_last_good_is_retained_for_the_same_credential() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Zai, calls.clone(), |nth| match nth {
            1 => Ok(zai_usage(14.0, "9012")),
            _ => Err(zai_failure(Some("key:9012"))),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await;
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.status, "error");
        assert_eq!(retained.account, zai_usage(14.0, "9012").account);
        assert_eq!(retained.limits, zai_usage(14.0, "9012").limits);
    }

    // 4. Z.ai candidate change drop (candidate attribution itself is pinned
    //    by the zai backend tests: the failure follows the attempted key)
    #[tokio::test]
    async fn zai_last_good_is_dropped_when_the_winning_credential_changed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Zai, calls.clone(), |nth| match nth {
            1 => Ok(zai_usage(14.0, "9012")),
            _ => Err(zai_failure(Some("key:77aa"))),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await;
        let entry = &core.snapshot().providers[0];
        assert_eq!(entry.status, "error");
        assert!(
            entry.limits.is_empty(),
            "another credential's last-good must not stand in"
        );
        assert!(entry.account.is_none(), "no cross-credential attribution survives");
    }

    // 6. Grok same-account retention. The clock advances past the 15-minute
    //    cadence window between the cycles, so the second cycle actually
    //    re-fetches (inside the window the cached first result is served and
    //    the guard would never see a failure).
    #[tokio::test]
    async fn grok_last_good_is_retained_for_the_same_account() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |nth| match nth {
            1 => Ok(grok_usage(71.0, "7a2d5abe…")),
            _ => Err(grok_failure(Some("xai:7a2d5abe…"))),
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        advance(&clock, 15 * 60_000 + 1);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the cadence gate must have opened");
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.status, "error");
        assert_eq!(retained.account, grok_usage(71.0, "7a2d5abe…").account);
        assert_eq!(retained.limits, grok_usage(71.0, "7a2d5abe…").limits);
    }

    // 7. Grok account change drop. The identity keys on the store-agnostic
    //    masked id by contract: the same account re-resolved through a
    //    different store is the same identity (retention holds), a different
    //    account is not (drop). The clock advances past the cadence window so
    //    the failing cycle really fetches.
    #[tokio::test]
    async fn grok_last_good_is_dropped_when_the_account_changed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |nth| match nth {
            1 => Ok(grok_usage(71.0, "7a2d5abe…")),
            _ => Err(grok_failure(Some("xai:0badf00d"))),
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        advance(&clock, 15 * 60_000 + 1);
        core.run_cycle().await;
        let entry = &core.snapshot().providers[0];
        assert_eq!(entry.status, "error");
        assert!(
            entry.limits.is_empty(),
            "another account's last-good must not stand in"
        );
        assert!(entry.account.is_none(), "no cross-account attribution survives");
    }

    // 8. OpenCode Go's existing guard is unchanged — pinned by the
    //    opencode_last_good_* tests above; the identity hand-off for the
    //    remaining backends mirrors the same mapping here.
    #[test]
    fn codex_and_zai_identity_formats_match_their_success_attribution() {
        // A drift between a backend's stamped failure identity and the
        // runtime attribution would silently disable the guard.
        assert_eq!(
            build_codex_attribution(Some(&codex::CodexAccount {
                account_hint: "56789abc".to_string(),
            }))
            .unwrap()
            .identity
            .as_deref(),
            Some("chatgpt:56789abc")
        );
        assert_eq!(
            build_zai_attribution(Some(&zai::ZaiAccount {
                key_hint: "5678".to_string(),
            }))
            .unwrap()
            .identity
            .as_deref(),
            Some("key:5678")
        );
    }

    // 9. Antigravity stays unattributed: its failures carry no identity, so
    //    retention keeps the existing conservative behavior (WEAK
    //    activeIndex attribution is accepted for v0.6; no Google identity is
    //    invented).
    #[tokio::test]
    async fn antigravity_failures_stay_unattributed_and_conservative() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Antigravity, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("antigravity", "Google Antigravity", 12.0)),
            _ => Err(failure_from_antigravity_error(antigravity::AntigravityError {
                code: "cache_missing".to_string(),
                message: "cache missing".to_string(),
            })),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await;
        let snapshot = core.snapshot();
        let entry = &snapshot.providers[0];
        // Retention is conservative: the failure carried no identity, so the
        // retained snapshot (itself unattributed) still stands, errored.
        assert_eq!(entry.status, "error");
        assert_eq!(entry.account, None);
        assert!(!entry.limits.is_empty(), "unattributed retention is unchanged");
        // The failure mapping itself never invents an identity.
        let mapped = failure_from_antigravity_error(antigravity::AntigravityError {
            code: "auth_failed".to_string(),
            message: "rejected".to_string(),
        });
        assert_eq!(mapped.identity, None);
    }

    // last_updated_at anchors to the newest successful cycle only
    #[tokio::test]
    async fn last_updated_at_only_moves_on_successful_cycles() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Zai, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("zai", "Z.ai", 11.0)),
            _ => Err(permanent_failure()),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        assert!(core.snapshot().last_updated_at.is_some());
        assert!(core.snapshot().cycle_succeeded);

        core.run_cycle().await;
        let snapshot = core.snapshot();
        assert!(!snapshot.cycle_succeeded, "all-error cycle is not a success");
        assert!(snapshot.last_updated_at.is_some(), "retained state keeps its anchor");
    }

    // 8. interval change updates scheduler configuration
    #[tokio::test]
    async fn interval_change_updates_snapshot_configuration() {
        let core = test_core(vec![grok_spec()]);
        assert_eq!(core.snapshot().refresh_interval_minutes, 5);
        assert_eq!(core.set_interval(15), Some(15));
        assert_eq!(core.snapshot().refresh_interval_minutes, 15);
        // No-op when unchanged; floors at one minute.
        assert_eq!(core.set_interval(15), None);
        assert_eq!(core.set_interval(0), Some(1));
    }

    // 9. snapshot pull returns latest state / 10. payload is serializable
    #[tokio::test]
    async fn snapshot_pulls_latest_state_and_serializes_to_the_wire_shape() {
        let core = test_core(vec![grok_spec()]);
        core.run_cycle().await;
        let snapshot = core.snapshot();
        assert_eq!(snapshot.seq, 1);
        let wire = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(wire["seq"], 1);
        assert_eq!(wire["cycleSucceeded"], true);
        assert_eq!(wire["cycleInFlight"], false);
        assert_eq!(wire["refreshIntervalMinutes"], 5);
        assert!(wire.get("last_updated_at").is_none(), "snake_case must never leak");
        let provider = &wire["providers"][0];
        assert_eq!(provider["id"], "grok");
        assert_eq!(provider["name"], "Grok (xAI)");
        assert_eq!(provider["status"], "ok");
        assert_eq!(provider["checkedAt"], fixed_now().to_rfc3339_opts(SecondsFormat::Millis, true));
        assert_eq!(provider["limits"][0]["usedPercent"], 10.0);
    }

    // Grok cadence: within the window the cached result is served
    #[tokio::test]
    async fn grok_cadence_serves_cached_result_within_the_window() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |nth| {
            Ok(ok_usage("grok", "Grok (xAI)", nth as f64))
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await; // seconds later: cadence window still open
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // The cached result keeps its original checkedAt.
        assert_eq!(core.snapshot().providers[0].limits[0].used_percent, 1.0);
    }

    // Grok: 429 Retry-After hint skips the fast in-call retry
    #[tokio::test]
    async fn grok_retry_after_hint_skips_the_fast_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |_| {
            Err(ProviderFailure {
                code: "unexpected_response".to_string(),
                message: "rate limited".to_string(),
                http_status: Some(429),
                transient: Some(true),
                retry_after_ms: Some(60_000),
                identity: None,
                transport_timeout: false,
            })
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no fast retry under Retry-After");
    }

    // Grok: local credential failures are never cached
    #[tokio::test]
    async fn grok_credential_failures_are_not_cached() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |nth| match nth {
            1 => Err(ProviderFailure {
                code: "credential_missing".to_string(),
                message: "no xAI credential".to_string(),
                http_status: None,
                transient: Some(false),
                retry_after_ms: None,
                identity: None,
                transport_timeout: false,
            }),
            _ => Ok(ok_usage("grok", "Grok (xAI)", 5.0)),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await; // must not be served from the cadence cache
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(core.snapshot().providers[0].status, "ok");
    }

    // The Antigravity cache fallback arrives as a success (cached windows,
    // existing health semantics) while the fresh live fetch behind it failed.
    // The structured cause must be observable in the diagnostics lane only:
    // the wire entry stays a plain success with no error metadata and no new
    // field, and the last-good/history/notification behavior is untouched.
    #[tokio::test]
    async fn antigravity_fallback_cause_is_diagnostic_evidence_only() {
        let payload = normalize_antigravity(
            Ok(AntigravityUsage {
                limits: vec![antigravity::AntigravityLimitWindow {
                    label: "Gemini".to_string(),
                    used_percent: 12.5,
                    reset_at: None,
                }],
                source_updated_at: Some("2026-09-18T02:13:32.735Z".to_string()),
                data_freshness: DataSourceFreshness::Stale,
                fallback_failure: Some(antigravity::AntigravityError {
                    code: "network".to_string(),
                    message: "Antigravity live quota fetch failed: the quota endpoint could not be reached.".to_string(),
                }),
            }),
            fixed_now().timestamp_millis(),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Antigravity, calls.clone(), move |_| {
            Ok(payload.clone())
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;

        // Wire: a plain stale success — no error metadata, no internal leak.
        let entry = &core.snapshot().providers[0];
        assert_eq!(entry.health, ProviderHealth::Stale);
        assert_eq!(entry.status, "stale");
        assert!(entry.error.is_none());
        assert!(entry.error_category.is_none());
        let wire = serde_json::to_value(core.snapshot()).unwrap();
        let provider = &wire["providers"][0];
        assert!(provider.get("error").is_none());
        assert!(provider.get("errorCategory").is_none());
        assert!(provider.get("fallbackFailure").is_none());
        assert!(!wire.to_string().contains("fallback_failure"));

        // Diagnostics lane: the fresh live-failure cause is retained.
        let diagnostics = core.diagnostic_source(fixed_now());
        let source = diagnostics
            .providers
            .iter()
            .find(|provider| provider.usage.id == ProviderKind::Antigravity.id())
            .unwrap();
        let last_error = source
            .last_error
            .as_ref()
            .expect("the fallback cause must reach the diagnostics lane");
        assert_eq!(last_error.code, "network");
        assert_eq!(last_error.http_status, None);
        assert!(source.last_error_at.is_some());
        // The refresh itself still counts as a success.
        assert!(source.last_success_at.is_some());
    }

    // The shared DTO gained an internal field; other providers' success
    // paths must never set it and must record no diagnostics error.
    #[tokio::test]
    async fn other_providers_never_carry_a_fallback_cause() {
        let codex = normalize_codex(
            Ok(CodexUsage {
                limits: vec![crate::codex::CodexLimitWindow {
                    label: "Weekly".to_string(),
                    used_percent: 42.0,
                    reset_at: None,
                }],
                plan_type: None,
                account: None,
                reset_credits: None,
            }),
            fixed_now().timestamp_millis(),
        )
        .unwrap();
        assert!(codex.fallback_failure.is_none());

        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |_| {
            Ok(ok_usage("grok", "Grok (xAI)", 30.0))
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        assert!(core.snapshot().providers[0].fallback_failure.is_none());
        assert!(
            core.diagnostic_source(fixed_now()).providers[0]
                .last_error
                .is_none()
        );
    }

    // Antigravity freshness: a fresh verdict without a usable stamp is stale
    #[test]
    fn antigravity_fresh_verdict_requires_a_parseable_stamp() {
        let payload = |stamp: Option<&str>, verdict: DataSourceFreshness| {
            normalize_antigravity(
                Ok(AntigravityUsage {
                    limits: vec![antigravity::AntigravityLimitWindow {
                        label: "AI credits".to_string(),
                        used_percent: 50.0,
                        reset_at: None,
                    }],
                    source_updated_at: stamp.map(str::to_string),
                    data_freshness: verdict,
                    fallback_failure: None,
                }),
                fixed_now().timestamp_millis(),
            )
            .unwrap()
        };
        let unreadable = payload(Some("not-a-timestamp"), DataSourceFreshness::Fresh);
        assert_eq!(unreadable.status, "stale", "unreadable stamp must never read as fresh");
        assert_eq!(unreadable.data_freshness, Some("stale"));
        assert_eq!(unreadable.source_updated_at, None);
        let no_stamp = payload(None, DataSourceFreshness::Fresh);
        assert_eq!(no_stamp.status, "stale", "fresh verdict without a stamp degrades to stale");
        let fresh = payload(Some("2026-09-29T11:00:00.000Z"), DataSourceFreshness::Fresh);
        assert_eq!(fresh.status, "ok");
        assert_eq!(fresh.data_freshness, Some("fresh"));
        assert_eq!(fresh.source_updated_at.as_deref(), Some("2026-09-29T11:00:00.000Z"));
        let stale_verdict = payload(Some("2026-09-29T11:00:00.000Z"), DataSourceFreshness::Stale);
        assert_eq!(stale_verdict.status, "stale");
    }

    // Attribution parity: OpenCode Go key hint and Grok masked account
    #[tokio::test]
    async fn account_attribution_matches_the_adapter_contract() {
        let opencode = ProviderSpec {
            kind: ProviderKind::OpenCodeGo,
            fetch: Arc::new(|| {
                Box::pin(async {
                    let mut dto = ok_usage("opencode-go", "OpenCode Go", 20.0);
                    dto.account = build_opencode_attribution(Some("3456"));
                    Ok(dto) as Result<_, ProviderFailure>
                }) as FetchFuture
            }),
        };
        let grok = ProviderSpec {
            kind: ProviderKind::Grok,
            fetch: Arc::new(|| {
                Box::pin(async {
                    let mut dto = ok_usage("grok", "Grok (xAI)", 21.0);
                    dto.account = build_grok_attribution(Some(&grok::GrokAccount {
                        id: "acct12345".to_string(),
                        source: "grok-cli".to_string(),
                    }));
                    Ok(dto) as Result<_, ProviderFailure>
                }) as FetchFuture
            }),
        };
        let core = test_core(vec![opencode, grok]);
        core.run_cycle().await;
        let providers = &core.snapshot().providers;
        let opencode_account = providers[0].account.as_ref().unwrap();
        assert_eq!(opencode_account.label, "key ••3456");
        assert_eq!(opencode_account.identity.as_deref(), Some("key:3456"));
        assert_eq!(build_opencode_attribution(Some("ab")), None, "short hints stay unattributed");
        let grok_account = providers[1].account.as_ref().unwrap();
        assert_eq!(grok_account.label, "acct12345 · grok-cli");
        assert_eq!(grok_account.identity.as_deref(), Some("xai:acct12345"));
    }

    // A chain emits one cycle-started and one snapshot per completed cycle
    #[tokio::test]
    async fn chain_emits_snapshot_per_cycle_and_reports_flight_state() {
        let core = test_core(vec![grok_spec()]);
        core.try_begin_cycle();
        let core_clone = core.clone();
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = events.clone();
        core_clone
            .run_chain(move |event| match event {
                RuntimeEvent::CycleStarted => sink.lock().unwrap().push("started".to_string()),
                RuntimeEvent::Snapshot(_) => sink.lock().unwrap().push("snapshot".to_string()),
            })
            .await;
        let events = events.lock().unwrap().clone();
        assert_eq!(events, vec!["started", "snapshot"]);
        let snapshot = core.snapshot();
        assert!(!snapshot.cycle_in_flight);
        assert_eq!(snapshot.seq, 1);
    }

    // ---- server-directed cooldown (Retry-After) ----

    // 5./6. 429 + Retry-After sets a cooldown and burns no fast retry.
    #[tokio::test]
    async fn four_twenty_nine_with_retry_after_sets_cooldown_without_fast_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |_| {
            Err(rate_limited_failure(Some(10 * 60_000)))
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the hint forbids the immediate second request"
        );
        // The rate-limit failure is surfaced through the existing contract.
        let provider = &core.snapshot().providers[0];
        assert_eq!(provider.status, "error");
        assert_eq!(
            provider.error.as_deref(),
            Some("rate limited (unexpected_response)")
        );

        // A later cycle inside the cooldown window skips the provider
        // entirely — the scheduled refresh and a manual refresh (both just
        // request a cycle) are gated by the same check inside the runtime.
        advance(&clock, 60_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "cooldown skip performs no fetch");
        assert_eq!(core.snapshot().providers[0].status, "error");
    }

    // 7. 429 without Retry-After preserves the existing bounded retry.
    #[tokio::test]
    async fn four_twenty_nine_without_retry_after_keeps_the_bounded_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Err(rate_limited_failure(None)),
            _ => Ok(ok_usage("openai-codex", "OpenAI / Codex", 8.0)),
        });
        let (core, _clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the plain fast retry ran");
        assert_eq!(core.snapshot().providers[0].status, "ok");
        // No cooldown was recorded: the next cycle fetches normally again.
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    // 8. 5xx without Retry-After gets at most one normal transient retry
    // and never a cooldown.
    #[tokio::test]
    async fn five_oh_three_without_retry_after_gets_one_plain_retry_and_no_cooldown() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 | 2 => Err(server_error_failure(None)),
            _ => Ok(ok_usage("openai-codex", "OpenAI / Codex", 9.0)),
        });
        let (core, _clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one transient retry only");
        assert_eq!(core.snapshot().providers[0].status, "error");
        // No cooldown: the next cycle goes back on the wire immediately.
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(core.snapshot().providers[0].status, "ok");
    }

    // 9. 5xx + Retry-After sets a cooldown (within the cap).
    #[tokio::test]
    async fn five_oh_three_with_retry_after_sets_cooldown() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |_| {
            Err(server_error_failure(Some(5 * 60_000)))
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "hint skips the fast retry");
        advance(&clock, 60_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "cooldown skips the fetch");
    }

    // 4. An absurd (multi-day) hint is capped at the 24-hour ceiling.
    #[tokio::test]
    async fn absurd_retry_after_is_capped_at_the_cooldown_ceiling() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |_| {
            Err(rate_limited_failure(Some(u64::MAX / 2)))
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Still cooling down just before the cap.
        advance(&clock, MAX_COOLDOWN_MS as i64 - 60_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "capped cooldown still holds");

        // Past the cap the provider is eligible again — never a multi-day
        // lockout from one absurd header.
        advance(&clock, 2 * 60_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "cooldown expired at the cap");
    }

    // 12. Grok's cadence and the server cooldown are separate gates; the
    // next eligible fetch is the later of the two.
    #[tokio::test]
    async fn grok_next_eligible_fetch_is_the_later_of_cadence_and_cooldown() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |_| {
            Err(rate_limited_failure(Some(10 * 60_000)))
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Cooldown still active (cadence window open too): skip, no fetch,
        // no cadence-cache resurrection of a success.
        advance(&clock, 5 * 60_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Cooldown expired, cadence window (15 min from the attempt) still
        // open: the cached last result is served without a fetch.
        advance(&clock, 5 * 60_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(core.snapshot().providers[0].status, "error");

        // Past both gates: back on the wire.
        advance(&clock, 5 * 60_000 + 1_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    // 13. last-good data is retained while cooling down, still stamped at
    // its original fetch time, and never reads as healthy.
    #[tokio::test]
    async fn cooling_down_provider_retains_last_good() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("openai-codex", "OpenAI / Codex", 33.0)),
            _ => Err(rate_limited_failure(Some(30 * 60_000))),
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await; // success seeds last good
        let good_checked_at = core.snapshot().providers[0].checked_at.clone();
        let good_limits = core.snapshot().providers[0].limits.clone();

        core.run_cycle().await; // rate limited
        advance(&clock, 60_000);
        core.run_cycle().await; // cooldown skip
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.status, "error");
        assert_eq!(retained.limits, good_limits, "last-good values survive the skip");
        assert_eq!(
            retained.checked_at, good_checked_at,
            "retained checkedAt stays at the original fetch time"
        );
        assert_eq!(
            retained.error.as_deref(),
            Some("Refresh failed: rate limited (unexpected_response)")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2, "skip did not refetch");
    }

    // 14. a cooldown skip creates no quota observation and no history
    // revision movement.
    #[tokio::test]
    async fn cooldown_skip_creates_no_history_observation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("openai-codex", "OpenAI / Codex", 25.0)),
            _ => Err(rate_limited_failure(Some(30 * 60_000))),
        });
        // Seed the clock at real wall time so the history store's future-skew
        // window matches the runtime clock.
        let (clock, getter_clock) = {
            let clock = Arc::new(Mutex::new(Utc::now()));
            let getter = clock.clone();
            (clock, getter)
        };
        let history_path = std::env::temp_dir().join(format!(
            "runtime-cooldown-history-test-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        ));
        let _ = std::fs::remove_file(&history_path);
        let store = Arc::new(QuotaHistoryStore::open_with_clock(
            history_path.clone(),
            Box::new(|| chrono::Utc::now().timestamp_millis()),
        ));
        let core = Arc::new(
            RuntimeCore::with_injections(
                vec![spec],
                5,
                Box::new(|| 0),
                Box::new(move || *getter_clock.lock().unwrap()),
            )
            .with_history_store(Some(store.clone())),
        );

        core.run_cycle().await; // ok — one real observation
        let after_success = store.history().len();
        let revision_after_success = store.revision();
        assert_eq!(after_success, 1, "the one successful refresh sampled once");
        assert!(revision_after_success >= 1);

        core.run_cycle().await; // rate limited — error entries never sample
        assert_eq!(store.history().len(), after_success);
        assert_eq!(store.revision(), revision_after_success);

        advance(&clock, 60_000);
        core.run_cycle().await; // cooldown skip — no fake observation
        assert_eq!(store.history().len(), after_success);
        assert_eq!(store.revision(), revision_after_success);

        drop(core);
        drop(store);
        let _ = std::fs::remove_file(history_path);
    }

    // 18. one provider cooling down never blocks the other providers.
    #[tokio::test]
    async fn one_provider_cooling_down_does_not_block_others() {
        let codex_calls = Arc::new(AtomicUsize::new(0));
        let zai_calls = Arc::new(AtomicUsize::new(0));
        let specs = vec![
            counting_spec(ProviderKind::Codex, codex_calls.clone(), |_| {
                Err(rate_limited_failure(Some(30 * 60_000)))
            }),
            counting_spec(ProviderKind::Zai, zai_calls.clone(), |nth| {
                Ok(ok_usage("zai", "Z.ai", nth as f64))
            }),
        ];
        let (core, clock) = manual_clock_core(specs);
        core.run_cycle().await;
        assert_eq!(codex_calls.load(Ordering::SeqCst), 1);
        assert_eq!(zai_calls.load(Ordering::SeqCst), 1);

        advance(&clock, 60_000);
        core.run_cycle().await;
        assert_eq!(
            codex_calls.load(Ordering::SeqCst),
            1,
            "cooling provider skipped"
        );
        assert_eq!(
            zai_calls.load(Ordering::SeqCst),
            2,
            "healthy provider refreshes normally"
        );
        assert_eq!(core.snapshot().providers[1].status, "ok");
    }

    // 19. cooldown expiry restores eligibility.
    #[tokio::test]
    async fn cooldown_expiry_restores_eligibility() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |_| {
            Err(rate_limited_failure(Some(10 * 60_000)))
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        advance(&clock, 10 * 60_000 + 1_000);
        core.run_cycle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "provider is on the wire again after expiry"
        );
    }

    // ---- wake / reconnect triggers ----

    // Wake detection: scheduling jitter is tolerated, a real timer freeze
    // (suspend) is caught.
    #[test]
    fn suspend_detection_tolerates_jitter_but_catches_freezes() {
        assert!(!suspend_detected(30_000, 30_000), "on-time chunk");
        assert!(!suspend_detected(31_800, 30_000), "scheduling jitter");
        assert!(
            !suspend_detected(30_000 + WAKE_DRIFT_TOLERANCE_MS, 30_000),
            "exactly at the tolerance is not a suspend"
        );
        assert!(
            suspend_detected(30_000 + WAKE_DRIFT_TOLERANCE_MS + 1, 30_000),
            "beyond the tolerance is a suspend"
        );
        assert!(
            suspend_detected(2 * 60 * 60 * 1000, SCHEDULER_CHUNK_MS),
            "a two-hour wall overshoot of one chunk is a suspend"
        );
    }

    // 16. a resume burst mid-cycle coalesces into at most one follow-up,
    // and repeat events inside the dedupe window after the chain never
    // start another cycle.
    #[tokio::test]
    async fn resume_burst_results_in_at_most_one_follow_up_cycle() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let gate_fetch = gate.clone();
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                let gate = gate_fetch.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let _permit = gate.acquire_owned().await.unwrap();
                    Ok(ok_usage("openai-codex", "OpenAI / Codex", 42.0)) as Result<_, ProviderFailure>
                }) as FetchFuture
            }),
        };
        let (core, clock) = manual_clock_core(vec![spec]);
        assert!(core.try_begin_cycle(), "scheduled cycle starts");
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });

        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // A wake burst during the cycle: every event coalesces (false), the
        // single pending follow-up is set once.
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Wake));
        advance(&clock, 1_000);
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Wake));
        advance(&clock, 1_000);
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Wake));

        gate.add_permits(2);
        let _ = chain.await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "exactly one follow-up ran");

        // More wake events inside the dedupe window after the chain: no
        // new cycle may start.
        advance(&clock, 1_000);
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Wake));
        // The coalescing invariants hold.
        let snapshot = core.snapshot();
        assert!(!snapshot.cycle_in_flight);
        assert_eq!(snapshot.seq, 2);
    }

    // 17. a reconnect burst behaves like a wake burst: one follow-up at
    // most, and wake/reconnect dedupe independently (one trigger kind
    // never consumes the other's burst window).
    #[tokio::test]
    async fn reconnect_burst_results_in_at_most_one_follow_up_cycle() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let gate_fetch = gate.clone();
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Zai,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                let gate = gate_fetch.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let _permit = gate.acquire_owned().await.unwrap();
                    Ok(ok_usage("zai", "Z.ai", 42.0)) as Result<_, ProviderFailure>
                }) as FetchFuture
            }),
        };
        let (core, clock) = manual_clock_core(vec![spec]);
        // A wake trigger starts the cycle (simulating a resume that found
        // the network still down).
        assert!(core.try_begin_auto_cycle(AutoTrigger::Wake));
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // The reconnect lands mid-cycle (network returned): it coalesces
        // into the follow-up even though wake just fired — separate windows.
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Reconnect));
        // A reconnect burst right behind it: deduped, still one follow-up.
        advance(&clock, 1_000);
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Reconnect));
        advance(&clock, 1_000);
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Reconnect));

        gate.add_permits(2);
        let _ = chain.await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "exactly one follow-up ran");
        assert!(!core.snapshot().cycle_in_flight);
    }

    // 15. manual, resume, and reconnect requests hitting a live cycle all
    // converge into the same single follow-up — and a manual refresh is
    // never throttled by the trigger dedupe.
    #[tokio::test]
    async fn manual_and_auto_requests_share_one_coalesced_chain() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let gate_fetch = gate.clone();
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::OpenCodeGo,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                let gate = gate_fetch.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let _permit = gate.acquire_owned().await.unwrap();
                    Ok(ok_usage("opencode-go", "OpenCode Go", 42.0)) as Result<_, ProviderFailure>
                }) as FetchFuture
            }),
        };
        let (core, _clock) = manual_clock_core(vec![spec]);
        assert!(core.try_begin_cycle(), "manual refresh starts the cycle");
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Manual and automatic sources all coalesce into the one pending
        // follow-up; none may start a parallel cycle.
        assert!(!core.try_begin_cycle(), "manual mid-cycle coalesces");
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Wake));
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Reconnect));
        {
            let inner = core.inner.lock().unwrap();
            assert!(inner.rerun_pending, "exactly one pending follow-up");
        }

        gate.add_permits(2);
        let _ = chain.await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Immediately after the chain, a manual refresh still runs (never
        // deduped) while an automatic repeat inside the burst window is
        // dropped outright — the manual cycle covers it.
        assert!(core.try_begin_cycle(), "manual refresh is never deduped");
        let manual_core = core.clone();
        let manual_chain = tokio::spawn(async move { manual_core.run_chain(|_| {}).await });
        while calls.load(Ordering::SeqCst) == 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Wake));
        gate.add_permits(1);
        let _ = manual_chain.await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    // Automatic triggers become eligible again once the dedupe window has
    // passed (clock-driven, not a permanent latch).
    #[test]
    fn auto_trigger_dedupe_expires_with_time() {
        let (core, clock) = manual_clock_core(vec![grok_spec()]);
        assert!(core.try_begin_auto_cycle(AutoTrigger::Wake));
        core.finish_cycle(); // close the accepted trigger's cycle
        // A repeat inside the window is dropped even on an idle runtime.
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Wake));
        // Past the window the trigger is accepted again.
        advance(&clock, TRIGGER_DEDUPE_MS + 1);
        assert!(core.try_begin_auto_cycle(AutoTrigger::Wake));
    }

    // ---------- reset wake (scheduler deadline) ----------

    fn usage_with_reset(
        id: &str,
        name: &str,
        label: &str,
        reset_at: Option<&str>,
    ) -> ProviderUsageDto {
        let mut usage = ok_usage(id, name, 10.0);
        usage.limits = vec![UsageLimitDto {
            label: label.to_string(),
            used_percent: 10.0,
            reset_at: reset_at.map(str::to_string),
        }];
        usage
    }

    /// A reset stamp `ms_from_now` from the fixed test clock (negative is
    /// in the past), in the same RFC-3339 shape `limits_from` produces.
    fn reset_at_ms(ms_from_now: i64) -> String {
        (fixed_now() + chrono::Duration::milliseconds(ms_from_now))
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    // 1. no reset bound at all: the scheduler keeps its normal deadline.
    #[test]
    fn no_reset_at_leaves_the_normal_polling_deadline_alone() {
        let usage = ok_usage("openai-codex", "OpenAI / Codex", 10.0);
        assert_eq!(earliest_reset_wake_at(&[usage], fixed_now()), None);
        assert_eq!(earliest_reset_wake_at(&[], fixed_now()), None);
    }

    // 2. a known future reset pulls the next wake forward to exactly
    //    reset_at + guard — observed through the real ingestion path, so
    //    the plausibility rule gates the stamp before the scheduler sees it.
    #[tokio::test]
    async fn known_reset_advances_the_wake_to_reset_plus_guard() {
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(|| {
                Box::pin(async {
                    Ok(usage_with_reset(
                        "openai-codex",
                        "OpenAI / Codex",
                        "5 hours",
                        Some(&reset_at_ms(5 * 60_000)),
                    ))
                }) as FetchFuture
            }),
        };
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        let expected = fixed_now()
            + chrono::Duration::milliseconds(5 * 60_000 + RESET_WAKE_GUARD_MS);
        assert_eq!(core.earliest_reset_wake_at(), Some(expected));
        // Once the guard has elapsed the stamp is spent: it can never
        // schedule another wake, so no repeated-wakeup loop is possible.
        advance(&clock, 5 * 60_000 + RESET_WAKE_GUARD_MS + 1);
        assert_eq!(core.earliest_reset_wake_at(), None);
    }

    // The pure deadline combination: a reset wake strictly sooner than the
    // interval pulls the effective deadline forward; a tie or a later reset
    // leaves the normal tick (one cycle covers both).
    #[test]
    fn a_sooner_reset_wake_pulls_the_deadline_forward() {
        let interval = tokio::time::Instant::now() + Duration::from_secs(300);
        let reset = tokio::time::Instant::now() + Duration::from_secs(60);
        let (deadline, outcome) = combined_scheduler_deadline(interval, Some(reset));
        assert_eq!(deadline, reset);
        assert!(matches!(outcome, SchedulerOutcome::ResetWake));
    }

    #[test]
    fn the_normal_poll_wins_when_sooner_or_tied_or_absent() {
        let interval = tokio::time::Instant::now() + Duration::from_secs(60);
        let (deadline, outcome) = combined_scheduler_deadline(
            interval,
            Some(interval + Duration::from_secs(240)),
        );
        assert_eq!(deadline, interval);
        assert!(matches!(outcome, SchedulerOutcome::Tick));
        // Effectively simultaneous: one useful cycle — the normal tick.
        let (deadline, outcome) = combined_scheduler_deadline(interval, Some(interval));
        assert_eq!(deadline, interval);
        assert!(matches!(outcome, SchedulerOutcome::Tick));
        let (deadline, outcome) = combined_scheduler_deadline(interval, None);
        assert_eq!(deadline, interval);
        assert!(matches!(outcome, SchedulerOutcome::Tick));
    }

    // 4. multiple windows across providers: the earliest eligible reset wins.
    #[test]
    fn the_earliest_eligible_reset_wins_across_providers_and_windows() {
        let codex = usage_with_reset(
            "openai-codex",
            "OpenAI / Codex",
            "5 hours",
            Some(&reset_at_ms(120 * 60_000)),
        );
        let mut zai = usage_with_reset("zai", "Z.ai", "Weekly", Some(&reset_at_ms(30 * 60_000)));
        zai.limits.push(UsageLimitDto {
            label: "5 hours".to_string(),
            used_percent: 20.0,
            reset_at: Some(reset_at_ms(60 * 60_000)),
        });
        let wake = earliest_reset_wake_at(&[codex, zai], fixed_now())
            .expect("an eligible reset must schedule a wake");
        assert_eq!(
            wake,
            fixed_now() + chrono::Duration::milliseconds(30 * 60_000 + RESET_WAKE_GUARD_MS)
        );
    }

    // 5. a past reset never schedules a wake — including one inside the
    //    5-minute clock-skew tolerance, which plausibility alone still
    //    accepts. This is the no-wakeup-loop pin for consumed stamps.
    #[test]
    fn a_past_reset_never_schedules_a_wake() {
        let old = usage_with_reset(
            "openai-codex",
            "OpenAI / Codex",
            "5 hours",
            Some(&reset_at_ms(-60 * 60_000)),
        );
        assert_eq!(earliest_reset_wake_at(&[old], fixed_now()), None);
        let spent = usage_with_reset(
            "openai-codex",
            "OpenAI / Codex",
            "5 hours",
            Some(&reset_at_ms(-2 * 60_000)),
        );
        assert_eq!(earliest_reset_wake_at(&[spent], fixed_now()), None);
    }

    // 6. implausible stamps are ignored by the same rule ingestion uses —
    //    the scheduler never grows a competing validation.
    #[test]
    fn implausible_resets_are_ignored_by_the_existing_plausibility_rule() {
        let beyond = usage_with_reset(
            "zai",
            "Z.ai",
            "Weekly",
            Some(&reset_at_ms(20 * 24 * 60 * 60_000)),
        );
        assert_eq!(
            earliest_reset_wake_at(&[beyond], fixed_now()),
            None,
            "20 days out is past the weekly horizon"
        );
        let unparseable = usage_with_reset("zai", "Z.ai", "Weekly", Some("soon"));
        assert_eq!(earliest_reset_wake_at(&[unparseable], fixed_now()), None);
    }

    // 7. the reset wake routes through the same single-owner cycle
    //    machinery as the other automatic triggers, and its burst window
    //    expires with time like theirs.
    #[tokio::test]
    async fn reset_wake_routes_through_the_shared_cycle_machinery() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| {
            Ok(ok_usage("openai-codex", "OpenAI / Codex", nth as f64))
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        assert!(
            core.try_begin_auto_cycle(AutoTrigger::ResetWake),
            "the reset wake owns a cycle"
        );
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });
        let _ = chain.await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(core.snapshot().seq, 1);
        // A repeat inside the burst window is deduped like every trigger.
        assert!(!core.try_begin_auto_cycle(AutoTrigger::ResetWake));
        advance(&clock, TRIGGER_DEDUPE_MS + 1);
        assert!(core.try_begin_auto_cycle(AutoTrigger::ResetWake));
        core.finish_cycle();
    }

    // 8. a reset wake landing mid-cycle coalesces into the one pending
    //    follow-up next to the other automatic triggers — never a parallel
    //    cycle, and its repeats stay inside one burst window.
    #[tokio::test]
    async fn reset_wake_mid_cycle_coalesces_into_one_follow_up() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let gate_fetch = gate.clone();
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                let gate = gate_fetch.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let _permit = gate.acquire_owned().await.unwrap();
                    Ok(ok_usage("openai-codex", "OpenAI / Codex", 42.0)) as Result<_, ProviderFailure>
                }) as FetchFuture
            }),
        };
        let (core, clock) = manual_clock_core(vec![spec]);
        assert!(core.try_begin_cycle(), "the scheduled cycle starts");
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // The reset wake lands mid-cycle: it coalesces into the follow-up.
        assert!(!core.try_begin_auto_cycle(AutoTrigger::ResetWake));
        // A reset-wake burst right behind it: deduped, still one follow-up.
        advance(&clock, 1_000);
        assert!(!core.try_begin_auto_cycle(AutoTrigger::ResetWake));
        // Other trigger kinds share the same single follow-up slot.
        assert!(!core.try_begin_auto_cycle(AutoTrigger::Wake));

        gate.add_permits(2);
        let _ = chain.await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "exactly one follow-up ran");
        assert_eq!(core.snapshot().seq, 2);
        assert!(!core.snapshot().cycle_in_flight);
    }

    // 9. a reset wake never bypasses a server-directed cooldown: the
    //    cooling-down provider is skipped by the shared machinery, like for
    //    every other trigger.
    #[tokio::test]
    async fn reset_wake_does_not_bypass_an_active_cooldown() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(usage_with_reset(
                "openai-codex",
                "OpenAI / Codex",
                "5 hours",
                Some(&reset_at_ms(30 * 60_000)),
            )),
            _ => Err(rate_limited_failure(Some(30 * 60_000))),
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await; // ok — the reset is learned
        core.run_cycle().await; // 429 + Retry-After → cooldown
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // The reset wake fires while the cooldown is still active.
        advance(&clock, 60_000);
        assert!(core.try_begin_auto_cycle(AutoTrigger::ResetWake));
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });
        let _ = chain.await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the cooling-down provider must not be refetched"
        );
        let provider = &core.snapshot().providers[0];
        assert_eq!(provider.health, ProviderHealth::Cooldown);
        assert_eq!(provider.status, "error");
    }

    // 10. the reset wake honors the provider cadence floor: Grok inside its
    //     15-minute window is served from the cadence cache, not refetched.
    #[tokio::test]
    async fn reset_wake_honors_the_grok_cadence_floor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |_| {
            Ok(grok_usage(40.0, "7a2d5abe…"))
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await; // one real fetch, cached
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        advance(&clock, 60_000); // well inside the 15-minute cadence window
        assert!(core.try_begin_auto_cycle(AutoTrigger::ResetWake));
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });
        let _ = chain.await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the cadence floor must serve the cache, not refetch"
        );
        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Live);
    }

    // ---- normalized health contract (v0.6) ----

    fn failure(code: &str, http_status: Option<u16>) -> ProviderFailure {
        ProviderFailure {
            code: code.to_string(),
            message: "refused".to_string(),
            http_status,
            transient: Some(false),
            retry_after_ms: None,
            identity: None,
            transport_timeout: false,
        }
    }

    // 1. a fresh successful provider projects Live, and the legacy status
    // stays "ok" (derived, never independent).
    #[tokio::test]
    async fn fresh_success_projects_live() {
        let core = test_core(vec![grok_spec()]);
        core.run_cycle().await;
        let provider = &core.snapshot().providers[0];
        assert_eq!(provider.health, ProviderHealth::Live);
        assert_eq!(provider.status, "ok");
        assert_eq!(provider.error_category, None);
        assert_eq!(provider.error_http_status, None);
    }

    // 2. a success past freshness policy (Antigravity's source verdict)
    // projects Stale — shown, but never live.
    #[test]
    fn stale_verdict_projects_stale_never_live() {
        let stale = normalize_antigravity(
            Ok(AntigravityUsage {
                limits: vec![antigravity::AntigravityLimitWindow {
                    label: "AI credits".to_string(),
                    used_percent: 50.0,
                    reset_at: None,
                }],
                source_updated_at: Some("2026-09-29T11:00:00.000Z".to_string()),
                data_freshness: DataSourceFreshness::Stale,
                fallback_failure: None,
            }),
            fixed_now().timestamp_millis(),
        )
        .unwrap();
        assert_eq!(stale.health, ProviderHealth::Stale);
        assert_eq!(stale.status, "stale");
        assert_ne!(stale.health, ProviderHealth::Live);
        // A persisted-style stale snapshot (same shape a future hydration
        // would emit) survives the wire with the same values.
        let wire = serde_json::to_value(&stale).unwrap();
        assert_eq!(wire["health"], "stale");
        assert_eq!(wire["status"], "stale");
    }

    // 3. a failed refresh with last-good projects Error + retained data,
    // with the failure's normalized metadata.
    #[tokio::test]
    async fn failure_with_last_good_is_error_with_metadata() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("openai-codex", "OpenAI / Codex", 33.0)),
            _ => Err(failure("network", None)),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await; // success seeds last good
        core.run_cycle().await; // failure retains it
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.health, ProviderHealth::Error);
        assert_eq!(retained.status, "error", "legacy wire value is unchanged");
        assert_eq!(retained.limits.len(), 1, "last-good values are retained");
        assert_eq!(retained.error_category.as_deref(), Some("network"));
        assert_eq!(retained.error_http_status, None);
    }

    // 4. a failed refresh without last-good is Error or Unavailable by the
    // explicit code contract: transport/HTTP/schema failures are Error;
    // absent local source/auth state is Unavailable.
    #[tokio::test]
    async fn failure_without_last_good_is_error_or_unavailable_by_code() {
        let transport = failure("network", None);
        assert!(!failure_is_source_absent(&transport.code));
        let http = failure("unexpected_response", Some(503));
        assert!(!failure_is_source_absent(&http.code));
        let entitled = failure("not_entitled", None);
        assert!(!failure_is_source_absent(&entitled.code));
        for code in [
            "credential_missing",
            "credential_expired",
            "credential_ambiguous",
            "opencodex_config_unreadable",
            "auth_expired",
            "auth_invalid",
            "auth_unreadable",
            "auth_file_missing",
            "auth_failed",
            "not_logged_in",
            "cache_missing",
            "no_account",
            "quota_missing",
            "codex_not_installed",
            "opencode_not_installed",
            "zcode_not_installed",
        ] {
            assert!(failure_is_source_absent(code), "{code} is source-absent");
        }
        // The bare projections follow the classification.
        let network_bare = {
            let core = test_core(vec![counting_spec(ProviderKind::Codex, Arc::new(AtomicUsize::new(0)), move |_| Err(failure("network", Some(503))))]);
            core.run_cycle().await;
            core.snapshot().providers[0].clone()
        };
        assert_eq!(network_bare.health, ProviderHealth::Error);
        assert_eq!(network_bare.error_category.as_deref(), Some("network"));
        assert_eq!(network_bare.error_http_status, Some(503));
        let auth_bare = {
            let core = test_core(vec![counting_spec(
                ProviderKind::Codex,
                Arc::new(AtomicUsize::new(0)),
                move |_| Err(failure("credential_missing", None)),
            )]);
            core.run_cycle().await;
            core.snapshot().providers[0].clone()
        };
        assert_eq!(auth_bare.health, ProviderHealth::Unavailable);
        assert_eq!(auth_bare.status, "error", "legacy wire value is unchanged");
        assert_eq!(auth_bare.error_category.as_deref(), Some("credential_missing"));
    }

    // 5./6. an active Retry-After cooldown projects Cooldown — bare and
    // with retained last-good — distinct from Error and from Stale.
    #[tokio::test]
    async fn cooldown_projects_cooldown_bare_and_with_last_good() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("openai-codex", "OpenAI / Codex", 33.0)),
            _ => Err(rate_limited_failure(Some(30 * 60_000))),
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await; // success seeds last good
        core.run_cycle().await; // rate limited, cooldown applied
        advance(&clock, 60_000);
        core.run_cycle().await; // cooldown skip with last-good
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.health, ProviderHealth::Cooldown);
        assert_eq!(retained.status, "error", "legacy wire value is unchanged");
        assert_eq!(retained.limits.len(), 1, "last-good stays on display");
        assert_eq!(retained.error_category.as_deref(), Some("unexpected_response"));
        assert_eq!(retained.error_http_status, Some(429));
        // Cooldown is never Live and never ordinary Stale.
        assert_ne!(retained.health, ProviderHealth::Live);
        assert_ne!(retained.health, ProviderHealth::Stale);
        assert_ne!(retained.health, ProviderHealth::Error);

        // A bare cooldown (no last-good) is still Cooldown.
        let calls_bare = Arc::new(AtomicUsize::new(0));
        let bare_spec = counting_spec(ProviderKind::Zai, calls_bare.clone(), |_| {
            Err(rate_limited_failure(Some(30 * 60_000)))
        });
        let (bare_core, bare_clock) = manual_clock_core(vec![bare_spec]);
        bare_core.run_cycle().await; // rate limited, cooldown applied
        advance(&bare_clock, 60_000);
        bare_core.run_cycle().await; // skip without last-good
        let bare = &bare_core.snapshot().providers[0];
        assert_eq!(bare.health, ProviderHealth::Cooldown);
        assert!(bare.limits.is_empty());
    }

    // 7. cooldown expiry + success projects Live again.
    #[tokio::test]
    async fn cooldown_expiry_plus_success_is_live() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Err(rate_limited_failure(Some(10 * 60_000))),
            _ => Ok(ok_usage("openai-codex", "OpenAI / Codex", 7.0)),
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await; // rate limited
        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Error);
        advance(&clock, 10 * 60_000 + 1_000);
        core.run_cycle().await; // cooldown expired, fetch succeeds
        let provider = &core.snapshot().providers[0];
        assert_eq!(calls.load(Ordering::SeqCst), 2, "provider is back on the wire");
        assert_eq!(provider.health, ProviderHealth::Live);
        assert_eq!(provider.error_category, None, "recovery clears metadata");
        assert_eq!(provider.error_http_status, None);
    }

    // 9. an auth/source-absent failure without last-good projects
    // Unavailable — while the same failure with retained last-good stays
    // Error (retained data is still shown; Error-with-last-good is the
    // contract, Unavailable needs absent state).
    #[tokio::test]
    async fn unavailable_auth_failure_with_retained_last_good_stays_error() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("openai-codex", "OpenAI / Codex", 33.0)),
            _ => Err(permanent_failure()), // auth_expired
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        core.run_cycle().await;
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.health, ProviderHealth::Error);
        assert_eq!(retained.error_category.as_deref(), Some("auth_expired"));
    }

    // 8. a stale projection never reads as live through any runtime path:
    // the derived legacy status is never "ok" and a failure over stale
    // last-good lands in the failure family (Error), never Live, never
    // back to plain Stale. A persisted-style stale snapshot is exactly the
    // shape a future hydration would feed in.
    #[tokio::test]
    async fn stale_projection_never_reads_live() {
        let stale = normalize_antigravity(
            Ok(AntigravityUsage {
                limits: vec![antigravity::AntigravityLimitWindow {
                    label: "AI credits".to_string(),
                    used_percent: 50.0,
                    reset_at: None,
                }],
                source_updated_at: Some("2026-09-29T11:00:00.000Z".to_string()),
                data_freshness: DataSourceFreshness::Stale,
                fallback_failure: None,
            }),
            fixed_now().timestamp_millis(),
        )
        .unwrap();
        assert_eq!(stale.health, ProviderHealth::Stale);
        let fail_spec = ProviderSpec {
            kind: ProviderKind::Antigravity,
            fetch: Arc::new(|| {
                Box::pin(async { Err(failure("network", None)) }) as FetchFuture
            }),
        };
        let core = Arc::new(RuntimeCore::with_injections(
            vec![fail_spec],
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        ));
        core.inner
            .lock()
            .unwrap()
            .last_good
            .insert(ProviderKind::Antigravity.id(), stale);
        core.run_cycle().await;
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.health, ProviderHealth::Error);
        assert_ne!(retained.health, ProviderHealth::Live);
        assert_ne!(retained.health, ProviderHealth::Stale);
    }

    // 12./13. account-safe state: retained last-good keeps the account it
    // was fetched for (never re-attributed), and a later success for a
    // different account replaces the retained state wholesale.
    #[tokio::test]
    async fn account_switch_does_not_reuse_wrong_state() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::OpenCodeGo, calls.clone(), |nth| {
            let mut dto = ok_usage("opencode-go", "OpenCode Go", 20.0 + nth as f64);
            dto.account = build_opencode_attribution(if nth <= 2 { Some("3456") } else { Some("9999") });
            if nth == 3 {
                dto.account = None; // failed fetch below; success at 4
                return Err(failure("network", None));
            }
            Ok(dto)
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await; // account A (key:3456) success
        core.run_cycle().await; // account A again
        core.run_cycle().await; // account B fetch fails — retains A's state
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.health, ProviderHealth::Error);
        assert_eq!(
            retained.account.as_ref().and_then(|a| a.identity.as_deref()),
            Some("key:3456"),
            "retained data keeps its own account attribution"
        );
        core.run_cycle().await; // account B success replaces last-good
        let fresh = &core.snapshot().providers[0];
        assert_eq!(fresh.health, ProviderHealth::Live);
        assert_eq!(
            fresh.account.as_ref().and_then(|a| a.identity.as_deref()),
            Some("key:9999"),
            "the new account's data replaces the old account's state"
        );
        assert_ne!(
            fresh.account.as_ref().and_then(|a| a.identity.as_deref()),
            retained.account.as_ref().and_then(|a| a.identity.as_deref()),
            "stale account A state never attaches to account B"
        );
    }

    // 14./15. recovery to Live from both failure families is the runtime's
    // only path back to eligible data (Error leg pinned below; cooldown leg
    // in cooldown_expiry_plus_success_is_live). The failing attempt is
    // deterministic (no retry, not source-absent) so the first cycle truly
    // projects Error.
    #[tokio::test]
    async fn provider_recovery_from_error_is_live() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Zai, calls.clone(), |nth| match nth {
            1 => Err(failure("unexpected", None)),
            _ => Ok(ok_usage("zai", "Z.ai", 11.0)),
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "deterministic failure never retries");
        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Error);
        core.run_cycle().await;
        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Live);
    }

    // 16. freshness stays independent from health: a retained-error entry
    // keeps its source-snapshot freshness (here: fresh), and health stays
    // Error — neither field collapses into the other.
    #[tokio::test]
    async fn freshness_is_independent_from_health() {
        let fresh_payload = normalize_antigravity(
            Ok(AntigravityUsage {
                limits: vec![antigravity::AntigravityLimitWindow {
                    label: "AI credits".to_string(),
                    used_percent: 50.0,
                    reset_at: None,
                }],
                source_updated_at: Some("2026-09-29T11:00:00.000Z".to_string()),
                data_freshness: DataSourceFreshness::Fresh,
                fallback_failure: None,
            }),
            fixed_now().timestamp_millis(),
        )
        .unwrap();
        assert_eq!(fresh_payload.health, ProviderHealth::Live);
        assert_eq!(fresh_payload.data_freshness, Some("fresh"));
        let stale_payload = normalize_antigravity(
            Ok(AntigravityUsage {
                limits: vec![antigravity::AntigravityLimitWindow {
                    label: "AI credits".to_string(),
                    used_percent: 60.0,
                    reset_at: None,
                }],
                source_updated_at: Some("2026-09-29T10:00:00.000Z".to_string()),
                data_freshness: DataSourceFreshness::Stale,
                fallback_failure: None,
            }),
            fixed_now().timestamp_millis(),
        )
        .unwrap();
        assert_eq!(stale_payload.health, ProviderHealth::Stale);
        assert_eq!(stale_payload.data_freshness, Some("stale"));
        // Live + Fresh and Stale + Stale pair as the contract expects, and
        // the freshness signal never duplicates the health value.
        assert_ne!(fresh_payload.health.as_str(), fresh_payload.data_freshness.unwrap());
        // A retained failure keeps the freshness of the data it retains
        // while health moves to Error: health=Error + freshness=fresh is
        // meaningful and must survive.
        let spec = ProviderSpec {
            kind: ProviderKind::Antigravity,
            fetch: Arc::new(move || {
                let payload = fresh_payload.clone();
                Box::pin(async move { Ok(payload) }) as FetchFuture
            }),
        };
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        let fail_spec = ProviderSpec {
            kind: ProviderKind::Antigravity,
            fetch: Arc::new(|| {
                Box::pin(async { Err(failure("network", None)) }) as FetchFuture
            }),
        };
        let seed = core.snapshot().providers[0].clone();
        let core = Arc::new(RuntimeCore::with_injections(
            vec![fail_spec],
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        ));
        core.inner
            .lock()
            .unwrap()
            .last_good
            .insert(ProviderKind::Antigravity.id(), seed);
        core.run_cycle().await;
        let retained = &core.snapshot().providers[0];
        assert_eq!(retained.health, ProviderHealth::Error);
        assert_eq!(
            retained.data_freshness,
            Some("fresh"),
            "the retained snapshot's own freshness is preserved"
        );
    }

    // 17. the wire shape is pinned: health is always present, error
    // metadata appears only on failure states, snake_case never leaks.
    #[tokio::test]
    async fn wire_pins_health_and_error_metadata_fields() {
        let live_core = test_core(vec![grok_spec()]);
        live_core.run_cycle().await;
        let live_wire = serde_json::to_value(&live_core.snapshot()).unwrap();
        let live = &live_wire["providers"][0];
        assert_eq!(live["health"], "live");
        assert_eq!(live["status"], "ok");
        assert!(live.get("errorCategory").is_none(), "success carries no metadata");
        assert!(live.get("errorHttpStatus").is_none());

        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |_| {
            Err(failure("network", Some(502)))
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;
        let error_wire = serde_json::to_value(&core.snapshot()).unwrap();
        let errored = &error_wire["providers"][0];
        assert_eq!(errored["health"], "error");
        assert_eq!(errored["status"], "error");
        assert_eq!(errored["errorCategory"], "network");
        assert_eq!(errored["errorHttpStatus"], 502);
        let wire_text = error_wire.to_string();
        assert!(!wire_text.contains("error_category"), "snake_case must never leak");
        assert!(!wire_text.contains("error_http_status"));
    }

    // 18. the provider registry order is unchanged by the contract (the
    // dedicated registry test pins it in full; this guards the contract
    // tests against accidental reordering).
    #[test]
    fn registry_ids_are_unchanged_by_the_health_contract() {
        let ids: Vec<&str> = production_specs().iter().map(|s| s.kind.id()).collect();
        assert_eq!(ids, ["openai-codex", "zai", "opencode-go", "antigravity", "grok"]);
    }

    #[test]
    fn codex_surfaces_safe_plan_type() {
        let limits = vec![crate::codex::CodexLimitWindow {
            label: "5-hour".to_string(),
            used_percent: 42.0,
            reset_at: Some("2026-09-30T14:00:00Z".to_string()),
        }];
        let usage = crate::codex::CodexUsage {
            limits,
            plan_type: Some(" Team ".to_string()),
            account: None,
            reset_credits: None,
        };
        let dto = normalize_codex(Ok(usage), fixed_now().timestamp_millis())
            .expect("normalized codex usage");
        assert_eq!(dto.plan_type.as_deref(), Some("team"));

        let serialized = serde_json::to_string(&dto).expect("serialized dto");
        assert!(serialized.contains(r#""planType":"team""#), "wire: {serialized}");

        let empty_plan = crate::codex::CodexUsage {
            limits: vec![],
            plan_type: Some("   ".to_string()),
            account: None,
            reset_credits: None,
        };
        let dto_empty = normalize_codex(Ok(empty_plan), fixed_now().timestamp_millis())
            .expect("normalized codex usage");
        assert_eq!(dto_empty.plan_type, None);
        let wire_empty = serde_json::to_string(&dto_empty).expect("serialized dto");
        assert!(!wire_empty.contains("planType"), "wire: {wire_empty}");
    }

    // ---------- v0.8 Lane B: runtime trust ----------
    //
    // Two mechanisms, both upstream of every history consumer:
    // 1. the reset plausibility gate at the normalization chokepoint
    //    (`limits_from`) — the pure label table itself is pinned in
    //    `reset_plausibility.rs`; here the integration consequences;
    // 2. the suspicious-drop hold/confirm state machine at recording time —
    //    bounded (one confirmation fetch per candidate, global cap, lazy
    //    TTL) and coalesced (one follow-up cycle per chain at most).

    fn laneb_clock() -> Arc<Mutex<DateTime<Utc>>> {
        Arc::new(Mutex::new(fixed_now()))
    }

    /// A history store on a temp path sharing the runtime's manual clock, so
    /// retention windows and hold timestamps evaluate deterministically.
    fn laneb_store(
        tag: &str,
        clock: &Arc<Mutex<DateTime<Utc>>>,
    ) -> (Arc<QuotaHistoryStore>, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "runtime-laneb-{tag}-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        ));
        let _ = std::fs::remove_file(&path);
        let getter = clock.clone();
        let store = Arc::new(QuotaHistoryStore::open_with_clock(
            path.clone(),
            Box::new(move || getter.lock().unwrap().timestamp_millis()),
        ));
        (store, path)
    }

    fn laneb_core(
        specs: Vec<ProviderSpec>,
        store: Arc<QuotaHistoryStore>,
        clock: &Arc<Mutex<DateTime<Utc>>>,
    ) -> Arc<RuntimeCore> {
        let getter = clock.clone();
        Arc::new(
            RuntimeCore::with_injections(
                specs,
                5,
                Box::new(|| 0),
                Box::new(move || *getter.lock().unwrap()),
            )
            .with_history_store(Some(store)),
        )
    }

    /// A Live one-window usage DTO with explicit label/reset control
    /// (`ok_usage` pins "Weekly"/None; the trust tests vary both).
    fn ok_usage_window(
        id: &str,
        name: &str,
        label: &str,
        percent: f64,
        reset_at: Option<&str>,
    ) -> ProviderUsageDto {
        ProviderUsageDto {
            id: id.to_string(),
            name: name.to_string(),
            status: "ok",
            health: ProviderHealth::Live,
            checked_at: fixed_now().to_rfc3339_opts(SecondsFormat::Millis, true),
            limits: vec![UsageLimitDto {
                label: label.to_string(),
                used_percent: percent,
                reset_at: reset_at.map(str::to_string),
            }],
            account: None,
            plan_type: None,
            error: None,
            error_category: None,
            error_http_status: None,
            source_updated_at: None,
            data_freshness: None,
            reset_credits: None,
            zcode_reset_cards: None,
            zcode_plans: None,
            fallback_failure: None,
        }
    }

    fn multi_window_usage(id: &str, name: &str, windows: &[(&str, f64)]) -> ProviderUsageDto {
        ProviderUsageDto {
            limits: windows
                .iter()
                .map(|(label, percent)| UsageLimitDto {
                    label: label.to_string(),
                    used_percent: *percent,
                    reset_at: None,
                })
                .collect(),
            ..ok_usage(id, name, 0.0)
        }
    }

    fn seeded_observation(
        provider_id: &str,
        window_label: &str,
        used_percent: f64,
        observed_at: DateTime<Utc>,
    ) -> QuotaObservation {
        QuotaObservation {
            provider_id: provider_id.to_string(),
            window_label: window_label.to_string(),
            used_percent,
            observed_at: observed_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            reset_at: None,
            account: None,
        }
    }

    fn week_ahead_reset() -> String {
        (fixed_now() + chrono::Duration::days(7))
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    // ---- reset plausibility gate: integration consequences ----

    // The gate is per window at the single chokepoint: one window's
    // implausible bound is dropped without touching its siblings.
    #[test]
    fn limits_from_drops_implausible_bounds_without_touching_siblings() {
        let now_ms = fixed_now().timestamp_millis();
        let far_reset = week_ahead_reset();
        let six_hours_out = (fixed_now() + chrono::Duration::hours(6))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let limits = vec![
            ("5 hours".to_string(), 40.0, Some(far_reset.clone())),
            ("Weekly".to_string(), 50.0, Some(far_reset)),
            ("hourly".to_string(), 60.0, Some(six_hours_out)),
            ("monthly".to_string(), 70.0, None),
        ];
        let out = limits_from(&limits, now_ms);
        assert_eq!(
            out[0].reset_at, None,
            "a 5-hour window claiming a reset 7 days out is rejected"
        );
        assert!(
            out[1].reset_at.is_some(),
            "the Weekly sibling keeps its plausible bound (isolation)"
        );
        assert_eq!(
            out[2].reset_at, None,
            "an hourly window claiming 6 hours out is rejected (3 h horizon)"
        );
        assert_eq!(out[3].reset_at, None, "absent stays absent");
        assert_eq!(out[0].used_percent, 40.0, "the window keeps its data");
    }

    // Full degradation pass: normalize drops only the bound, the observation
    // records with the window's data intact and no reset anchor.
    #[test]
    fn implausible_bound_degrades_through_normalize_and_record() {
        let usage = crate::codex::CodexUsage {
            limits: vec![crate::codex::CodexLimitWindow {
                label: "5 hours".to_string(),
                used_percent: 42.0,
                reset_at: Some(week_ahead_reset()),
            }],
            plan_type: None,
            account: None,
            reset_credits: None,
        };
        let mut dto = normalize_codex(Ok(usage), fixed_now().timestamp_millis())
            .expect("codex success must normalize");
        assert_eq!(dto.health, ProviderHealth::Live, "the window stays live");
        assert_eq!(dto.limits[0].used_percent, 42.0, "the data survives");
        assert_eq!(dto.limits[0].reset_at, None, "only the bound is dropped");

        // Record exactly what the runtime would: same eligibility rules.
        dto.checked_at = fixed_now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let clock = laneb_clock();
        let (store, path) = laneb_store("degrade-record", &clock);
        store.record(crate::history::observations_from_usages(&[dto]));
        let history = store.history();
        assert_eq!(history.len(), 1, "the observation records");
        assert_eq!(history[0].used_percent, 42.0);
        assert_eq!(history[0].reset_at, None, "no bound reached history");
        let _ = std::fs::remove_file(path);
    }

    // A parseable-but-too-far-future bound is treated exactly like the
    // unparseable family it joins: no anchor, honest degradation.
    #[test]
    fn unparseable_and_implausible_bounds_degrade_identically() {
        let now_ms = fixed_now().timestamp_millis();
        let limits = vec![
            ("Weekly".to_string(), 40.0, Some("soon".to_string())),
            (
                "Weekly".to_string(),
                50.0,
                Some((fixed_now() + chrono::Duration::days(40)).to_rfc3339_opts(SecondsFormat::Millis, true)),
            ),
        ];
        let out = limits_from(&limits, now_ms);
        assert_eq!(out[0].reset_at, None, "unparseable is not plausible");
        assert_eq!(out[1].reset_at, None, "40 days out is not plausible");
    }

    // ---- suspicious-drop confirmation: state machine ----

    #[tokio::test]
    async fn normal_progression_records_every_cycle_without_holds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage_window("openai-codex", "OpenAI / Codex", "Weekly", 10.0, None)),
            2 => Ok(ok_usage_window("openai-codex", "OpenAI / Codex", "Weekly", 45.0, None)),
            _ => Ok(ok_usage_window("openai-codex", "OpenAI / Codex", "Weekly", 70.0, None)),
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("normal-progression", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        for _ in 0..3 {
            core.run_cycle().await;
            advance(&clock, 60_000);
        }
        let history = store.history();
        assert_eq!(history.len(), 3, "rising values record every cycle");
        assert_eq!(history[2].used_percent, 70.0);
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // An announced reset (forward resetAt move > 1 min) is a legitimate new
    // cycle by definition: the drop records immediately, never held.
    #[tokio::test]
    async fn announced_reset_exempts_a_drop_from_confirmation() {
        let week_ahead = week_ahead_reset();
        let two_weeks_ahead = (fixed_now() + chrono::Duration::days(14))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| match nth {
            1 => Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                50.0,
                Some(&week_ahead),
            )),
            _ => Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                12.0,
                Some(&two_weeks_ahead),
            )),
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("announced-reset", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await;

        let history = store.history();
        assert_eq!(history.len(), 2, "the reset drop records immediately");
        assert_eq!(history[1].used_percent, 12.0);
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // A suspicious drop is withheld: no history write, no revision
    // movement — while the snapshot still shows the provider's live answer.
    #[tokio::test]
    async fn suspicious_drop_is_held_and_never_written_before_confirmation() {
        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| match nth {
            1 => Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                50.0,
                Some(&week_ahead),
            )),
            _ => Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                20.0,
                Some(&week_ahead),
            )),
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("held-drop", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        let revision_after_first = store.revision();
        advance(&clock, 60_000);
        core.run_cycle().await;

        assert_eq!(store.history().len(), 1, "nothing written before confirmation");
        assert_eq!(
            store.revision(),
            revision_after_first,
            "the history revision moves only on accepted writes"
        );
        assert_eq!(
            core.inner.lock().unwrap().pending_confirmations.len(),
            1,
            "exactly one hold, keyed by the logical window"
        );
        let live = &core.snapshot().providers[0];
        assert_eq!(live.health, ProviderHealth::Live);
        assert_eq!(
            live.limits[0].used_percent, 20.0,
            "the snapshot still shows current reality (only the history write is gated)"
        );
        let _ = std::fs::remove_file(path);
    }

    // Confirmation: the second low sample confirms the drop; the held
    // candidate and the confirming sample land time-ordered — exactly once.
    #[tokio::test]
    async fn confirmed_drop_writes_candidate_and_confirming_sample_time_ordered() {
        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            let percent = match nth {
                1 => 50.0,
                2 => 20.0,
                _ => 18.0,
            };
            Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                percent,
                Some(&week_ahead),
            ))
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("confirmed-drop", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        let revision_after_first = store.revision();
        advance(&clock, 60_000);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await;

        let history = store.history();
        let values: Vec<f64> = history.iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![50.0, 20.0, 18.0], "boundary and confirming sample, time-ordered");
        assert_eq!(
            history.iter().filter(|o| o.used_percent == 20.0).count(),
            1,
            "the held candidate is written exactly once (no double record)"
        );
        assert!(store.revision() > revision_after_first);
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // Rejection: the provider bounces back within jitter of the baseline —
    // the candidate is discarded and the refuting sample records normally.
    #[tokio::test]
    async fn refuted_drop_discards_the_candidate_and_records_the_refuting_sample() {
        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            let percent = match nth {
                1 => 50.0,
                2 => 20.0,
                _ => 49.0,
            };
            Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                percent,
                Some(&week_ahead),
            ))
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("refuted-drop", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await;

        let history = store.history();
        let values: Vec<f64> = history.iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![50.0, 49.0], "glitch discarded, refuting sample kept");
        assert!(!values.contains(&20.0), "the held candidate never reached history");
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // Defer: a sample inside the jitter band (inconclusive) writes nothing,
    // requests nothing, and keeps the pending — the next regular sample
    // resolves it.
    #[tokio::test]
    async fn inconclusive_sample_defers_without_writing_and_the_pending_survives() {
        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            let percent = match nth {
                1 => 50.0,
                2 => 20.0,
                3 => 47.0, // between 50−5 and 50−2: neither confirm nor refute
                _ => 18.0,
            };
            Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                percent,
                Some(&week_ahead),
            ))
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("deferred-sample", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await; // hold 20
        advance(&clock, 60_000);
        core.run_cycle().await; // 47: inconclusive

        assert_eq!(
            store.history().len(),
            1,
            "an inconclusive sample writes nothing while unresolved"
        );
        assert_eq!(
            core.inner.lock().unwrap().pending_confirmations.len(),
            1,
            "the pending survives an inconclusive sample"
        );

        advance(&clock, 60_000);
        core.run_cycle().await; // 18: confirms
        let values: Vec<f64> = store.history().iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![50.0, 20.0, 18.0]);
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // TTL expiry: the held candidate is discarded lazily (never written) and
    // the window re-classifies from scratch on the next regular cycle.
    #[tokio::test]
    async fn pending_confirmation_expires_after_ttl_and_reclassifies() {
        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            let percent = match nth {
                1 => 50.0,
                2 => 20.0,
                _ => 55.0,
            };
            Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                percent,
                Some(&week_ahead),
            ))
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("ttl-expiry", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await; // hold 20
        advance(&clock, 16 * 60_000); // past the 15-minute TTL
        core.run_cycle().await; // 55: pending expired, classifies as normal

        let values: Vec<f64> = store.history().iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![50.0, 55.0], "expired candidate never written; next sample records normally");
        assert!(!values.contains(&20.0));
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // Cached echo: a follow-up candidate identical to the held candidate —
    // same stamp, value, and bound, exactly what serving the provider's
    // cadence cache produces (the cached DTO keeps its original checkedAt,
    // while a real fetch re-stamps it in apply_success) — is not a second
    // observation. It defers: nothing is written, the pending survives, and
    // a genuinely fresh sample still confirms. Exercised at the
    // admit_observations level because a full-cycle fetch always re-stamps
    // checked_at, which would mask the echo.
    #[test]
    fn cached_echo_of_the_held_sample_never_confirms_the_drop() {
        let clock = laneb_clock();
        let (store, path) = laneb_store("cached-echo", &clock);
        let core = laneb_core(vec![], store.clone(), &clock);
        let at = |offset_s: i64| fixed_now() + chrono::Duration::seconds(offset_s);

        // The baseline records normally (no hold, no request).
        let (accepted, requested) = core.admit_observations(
            &[],
            vec![seeded_observation("openai-codex", "Weekly", 50.0, at(0))],
            at(0).timestamp_millis(),
        );
        assert!(!requested);
        assert_eq!(accepted.len(), 1);
        store.record(accepted);

        // The glitch is held: withheld from history, one follow-up requested.
        let (accepted, requested) = core.admit_observations(
            &store.history(),
            vec![seeded_observation("openai-codex", "Weekly", 20.0, at(30))],
            at(30).timestamp_millis(),
        );
        assert!(accepted.is_empty() && requested, "the drop is held");
        assert_eq!(store.history().len(), 1);

        // The echo: the identical observation again (same stamp — the
        // cadence cache re-serves the held candidate's own response). Not a
        // second observation: defer, write nothing, request nothing.
        let (accepted, requested) = core.admit_observations(
            &store.history(),
            vec![seeded_observation("openai-codex", "Weekly", 20.0, at(30))],
            at(60).timestamp_millis(),
        );
        assert!(accepted.is_empty() && !requested, "the echo defers");
        assert_eq!(
            store.history().len(),
            1,
            "the echo must not write the candidate"
        );
        assert_eq!(
            core.inner.lock().unwrap().pending_confirmations.len(),
            1,
            "the pending survives its own echo"
        );

        // A genuinely fresh sample confirms: the held candidate and the
        // confirming sample land time-ordered, exactly once.
        let (accepted, _requested) = core.admit_observations(
            &store.history(),
            vec![seeded_observation("openai-codex", "Weekly", 18.0, at(90))],
            at(90).timestamp_millis(),
        );
        let values: Vec<f64> = accepted.iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![20.0, 18.0], "candidate then confirming sample");
        store.record(accepted);
        let values: Vec<f64> = store.history().iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![50.0, 20.0, 18.0]);
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // The confirmation follow-up must observe fresh upstream state: the
    // follow-up cycle that carries a confirmation request runs with the
    // Grok cadence-cache bypass set, and the bypass clears with the chain.
    // Without this, the rerun lands inside Grok's 15-minute cache window
    // and "confirms" the drop with the held candidate's own echo.
    #[test]
    fn confirmation_follow_up_consumes_into_a_one_cycle_grok_cache_bypass() {
        let core = test_core(vec![]);
        assert!(core.try_begin_cycle());
        core.request_confirmation_follow_up();
        assert!(core.finish_cycle(), "the follow-up runs");
        assert!(core.inner.lock().unwrap().grok_cache_bypass);
        assert!(!core.finish_cycle(), "the chain closes");
        assert!(
            !core.inner.lock().unwrap().grok_cache_bypass,
            "the bypass clears with the chain"
        );
    }

    #[test]
    fn a_generic_follow_up_does_not_bypass_the_grok_cadence_cache() {
        let core = test_core(vec![]);
        assert!(core.try_begin_cycle());
        // A mid-cycle request without a confirmation hold (wake/tick path).
        assert!(!core.try_begin_cycle());
        assert!(core.finish_cycle());
        assert!(!core.inner.lock().unwrap().grok_cache_bypass);
    }

    // Job level: with the bypass set, Grok fetches even inside its cadence
    // window; without it, the cached result is served as before.
    #[tokio::test]
    async fn grok_confirmation_follow_up_bypasses_the_cadence_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |nth| {
            Ok(ok_usage("grok", "Grok (xAI)", nth as f64))
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await; // fresh fetch (1%); cache is hot
        core.run_cycle().await; // served from cache
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        core.inner.lock().unwrap().grok_cache_bypass = true;
        core.run_cycle().await; // bypass: fetches (2%)
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            core.snapshot().providers[0].limits[0].used_percent, 2.0,
            "the bypass cycle observes fresh upstream state"
        );
    }

    // Confirmation fetch failure: a failed cycle produces no sample, so the
    // pending survives untouched and no history write happens; the next
    // successful cycle resolves the hold.
    #[tokio::test]
    async fn failed_confirmation_cycle_keeps_pending_without_history_write() {
        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            match nth {
                1 => Ok(ok_usage_window(
                    "openai-codex",
                    "OpenAI / Codex",
                    "Weekly",
                    50.0,
                    Some(&week_ahead),
                )),
                2 => Ok(ok_usage_window(
                    "openai-codex",
                    "OpenAI / Codex",
                    "Weekly",
                    20.0,
                    Some(&week_ahead),
                )),
                // The confirmation fetch fails through its one bounded retry.
                3 | 4 => Err(transient_failure("network")),
                _ => Ok(ok_usage_window(
                    "openai-codex",
                    "OpenAI / Codex",
                    "Weekly",
                    18.0,
                    Some(&week_ahead),
                )),
            }
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("failed-confirm", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await; // hold 20
        let revision_after_hold = store.revision();
        advance(&clock, 60_000);
        core.run_cycle().await; // network failure (one bounded retry burns)

        assert_eq!(calls.load(Ordering::SeqCst), 4, "the failure burned its single retry");
        assert_eq!(store.history().len(), 1, "no history write on a failed cycle");
        assert_eq!(store.revision(), revision_after_hold);
        assert_eq!(
            core.inner.lock().unwrap().pending_confirmations.len(),
            1,
            "the pending survives a failed confirmation cycle"
        );

        advance(&clock, 60_000);
        core.run_cycle().await; // 18: confirms
        let values: Vec<f64> = store.history().iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![50.0, 20.0, 18.0]);
        let _ = std::fs::remove_file(path);
    }

    // Cooldown-blocked confirmation: the rerun is a normal cycle, so a
    // provider in cooldown is skipped entirely — no fetch, pending survives,
    // and the hold resolves when the cooldown expires.
    #[tokio::test]
    async fn cooldown_blocked_confirmation_waits_without_refetching() {
        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            match nth {
                1 => Ok(ok_usage_window(
                    "openai-codex",
                    "OpenAI / Codex",
                    "Weekly",
                    50.0,
                    Some(&week_ahead),
                )),
                2 => Ok(ok_usage_window(
                    "openai-codex",
                    "OpenAI / Codex",
                    "Weekly",
                    20.0,
                    Some(&week_ahead),
                )),
                // Rate-limited with a 5-minute Retry-After: shorter than the
                // pending TTL, so the hold can still resolve after the
                // cooldown expires.
                3 => Err(rate_limited_failure(Some(5 * 60_000))),
                _ => Ok(ok_usage_window(
                    "openai-codex",
                    "OpenAI / Codex",
                    "Weekly",
                    18.0,
                    Some(&week_ahead),
                )),
            }
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("cooldown-confirm", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await; // hold 20
        advance(&clock, 60_000);
        core.run_cycle().await; // 429 + Retry-After: cooldown, no fast retry

        assert_eq!(calls.load(Ordering::SeqCst), 3);
        advance(&clock, 60_000);
        core.run_cycle().await; // cooldown skip
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "the cooldown skip fetched nothing (no storm)"
        );
        assert_eq!(store.history().len(), 1);
        assert_eq!(
            core.inner.lock().unwrap().pending_confirmations.len(),
            1,
            "the pending waits for the cooldown, it does not re-fetch"
        );

        advance(&clock, 5 * 60_000); // past the cooldown, inside the TTL
        core.run_cycle().await; // 18: confirms
        let values: Vec<f64> = store.history().iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![50.0, 20.0, 18.0]);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        let _ = std::fs::remove_file(path);
    }

    // Coalescing pin: several suspicious windows during ONE in-flight cycle
    // request exactly one confirmation follow-up through the existing cycle
    // machinery — two cycles total, never one extra cycle per hold.
    #[tokio::test]
    async fn multiple_holds_in_one_cycle_coalesce_into_exactly_one_follow_up() {
        let calls = Arc::new(AtomicUsize::new(0));
        let clock = laneb_clock();
        let follow_up_clock = clock.clone();
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            // The follow-up observes genuinely fresh state: the fetch stamps
            // a later time (the manual clock advances with it) and the values
            // moved (still ≤ baseline − 5, so they confirm). Re-serving the
            // identical response at the identical stamp would be a
            // provider-cache echo, which the confirmation flow defers
            // instead of confirming with.
            if nth >= 2 {
                *follow_up_clock.lock().unwrap() =
                    fixed_now() + chrono::Duration::minutes(1);
            }
            if nth == 1 {
                Ok(multi_window_usage(
                    "openai-codex",
                    "OpenAI / Codex",
                    &[("Weekly", 20.0), ("Daily", 25.0)],
                ))
            } else {
                Ok(multi_window_usage(
                    "openai-codex",
                    "OpenAI / Codex",
                    &[("Weekly", 19.0), ("Daily", 24.0)],
                ))
            }
        });
        let (store, path) = laneb_store("no-storm", &clock);
        store.record(vec![
            seeded_observation("openai-codex", "Weekly", 50.0, fixed_now() - chrono::Duration::minutes(5)),
            seeded_observation("openai-codex", "Daily", 60.0, fixed_now() - chrono::Duration::minutes(5)),
        ]);
        let core = laneb_core(vec![spec], store.clone(), &clock);

        // Run through the real chain so the confirmation request lands on
        // the coalescing machinery exactly as production holds do.
        assert!(core.try_begin_cycle(), "the chain owns its cycle");
        core.run_chain(|_| {}).await;

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "one cycle + exactly one coalesced confirmation follow-up for two holds"
        );
        assert_eq!(core.snapshot().seq, 2, "the chain ran two cycles, not three");
        let history = store.history();
        assert_eq!(history.len(), 6, "both windows confirmed during the follow-up");
        let weekly: Vec<f64> = history
            .iter()
            .filter(|o| o.window_label == "Weekly")
            .map(|o| o.used_percent)
            .collect();
        assert_eq!(weekly, vec![50.0, 20.0, 19.0]);
        let daily: Vec<f64> = history
            .iter()
            .filter(|o| o.window_label == "Daily")
            .map(|o| o.used_percent)
            .collect();
        assert_eq!(daily, vec![60.0, 25.0, 24.0]);
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // Global cap: suspicious candidates arriving while
    // PENDING_CONFIRMATIONS_MAX pendings exist are discarded — never held,
    // never written.
    #[tokio::test]
    async fn global_cap_discards_suspicious_candidates_beyond_eight_pendings() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            // Nine suspicious windows; past the cap only eight may be held.
            1 => Ok(multi_window_usage(
                "openai-codex",
                "OpenAI / Codex",
                &[
                    ("w1", 20.0),
                    ("w2", 21.0),
                    ("w3", 22.0),
                    ("w4", 23.0),
                    ("w5", 24.0),
                    ("w6", 25.0),
                    ("w7", 26.0),
                    ("w8", 27.0),
                    ("w9", 28.0),
                ],
            )),
            // The confirmation cycle samples only the eight held windows.
            _ => Ok(multi_window_usage(
                "openai-codex",
                "OpenAI / Codex",
                &[
                    ("w1", 20.0),
                    ("w2", 21.0),
                    ("w3", 22.0),
                    ("w4", 23.0),
                    ("w5", 24.0),
                    ("w6", 25.0),
                    ("w7", 26.0),
                    ("w8", 27.0),
                ],
            )),
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("global-cap", &clock);
        let baselines: Vec<QuotaObservation> = (1..=9)
            .map(|i| {
                seeded_observation(
                    "openai-codex",
                    &format!("w{i}"),
                    50.0 + i as f64,
                    fixed_now() - chrono::Duration::minutes(5),
                )
            })
            .collect();
        store.record(baselines);
        let core = laneb_core(vec![spec], store.clone(), &clock);

        core.run_cycle().await;
        assert_eq!(
            core.inner.lock().unwrap().pending_confirmations.len(),
            PENDING_CONFIRMATIONS_MAX,
            "the cap bounds simultaneous holds"
        );
        let ninth = store
            .history()
            .iter()
            .filter(|o| o.window_label == "w9")
            .count();
        assert_eq!(ninth, 1, "the ninth candidate was discarded, never written");

        advance(&clock, 60_000);
        core.run_cycle().await; // the eight held windows confirm
        assert_eq!(store.history().len(), 9 + 8 * 2, "nine baselines + eight confirmed pairs");
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    // Clear interaction: dropping the pendings (Local Data paths) forgets
    // the hold — the next suspicious sample re-classifies from scratch, and
    // confirmation only happens against a freshly held candidate.
    #[tokio::test]
    async fn clearing_pending_confirmations_lets_the_window_reclassify() {
        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            let percent = match nth {
                1 => 50.0,
                _ => 18.0,
            };
            Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                percent,
                Some(&week_ahead),
            ))
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("clear-pendings", &clock);
        let core = laneb_core(vec![spec], store.clone(), &clock);
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await; // hold 18
        assert_eq!(core.inner.lock().unwrap().pending_confirmations.len(), 1);

        core.clear_pending_confirmations();
        assert!(core
            .inner
            .lock()
            .unwrap()
            .pending_confirmations
            .is_empty());
        assert_eq!(store.history().len(), 1, "clearing drops the gated candidate");

        advance(&clock, 60_000);
        core.run_cycle().await; // 18 again: suspicious against the baseline → held anew
        assert_eq!(
            core.inner.lock().unwrap().pending_confirmations.len(),
            1,
            "the persistent drop re-classifies instead of confirming against a dropped hold"
        );
        assert_eq!(store.history().len(), 1);

        advance(&clock, 60_000);
        core.run_cycle().await; // confirms
        let values: Vec<f64> = store.history().iter().map(|o| o.used_percent).collect();
        assert_eq!(values, vec![50.0, 18.0, 18.0]);
        let _ = std::fs::remove_file(path);
    }

    // Notifications consume snapshot DTOs and fire upward crossings only: a
    // held (or rejected) drop evaluates to nothing.
    #[tokio::test]
    async fn held_candidate_produces_no_notification() {
        let delivered: Arc<Mutex<Vec<crate::notifications::QuotaNotification>>> =
            Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let lane_path = std::env::temp_dir().join(format!(
            "runtime-laneb-notifications-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        ));
        let _ = std::fs::remove_file(&lane_path);
        let lane = Arc::new(crate::notifications::NotificationLane::open(
            lane_path.clone(),
            Box::new(move |notes| sink.lock().unwrap().extend_from_slice(notes)),
        ));
        lane.set_enabled(true);

        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |nth| {
            let percent = match nth {
                1 => 50.0,
                _ => 20.0,
            };
            Ok(ok_usage_window(
                "openai-codex",
                "OpenAI / Codex",
                "Weekly",
                percent,
                Some(&week_ahead),
            ))
        });
        let clock = laneb_clock();
        let (store, path) = laneb_store("notification-hold", &clock);
        let getter_clock = clock.clone();
        let core = Arc::new(
            RuntimeCore::with_injections(
                vec![spec],
                5,
                Box::new(|| 0),
                Box::new(move || *getter_clock.lock().unwrap()),
            )
            .with_history_store(Some(store))
            .with_notification_lane(Some(lane)),
        );
        core.run_cycle().await;
        advance(&clock, 60_000);
        core.run_cycle().await; // hold 20

        assert!(
            delivered.lock().unwrap().is_empty(),
            "a held candidate must not produce a notification"
        );
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(lane_path);
    }

    // -------------------------------------------------------------------
    // fault-injection matrix (hardening/runtime-fault-injection)
    //
    // Deterministic fault injection against the isolation model: one bad
    // provider or one badly timed refresh must never corrupt sibling
    // providers or runtime state. Scenario numbers reference the fault
    // matrix; the pinned invariants are called out inline.
    // -------------------------------------------------------------------

    // 20. a concurrent storm of manual and automatic requests against one
    //     live cycle. Invariants pinned: never more than one active refresh
    //     cycle; a requested coalesced rerun is not silently lost (the
    //     single follow-up slot absorbs every request and is consumed by
    //     exactly one real cycle).
    #[tokio::test]
    async fn a_concurrent_request_storm_never_starts_a_second_cycle_or_loses_the_follow_up() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let gate_fetch = gate.clone();
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                let gate = gate_fetch.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let _permit = gate.acquire_owned().await.unwrap();
                    Ok(ok_usage("openai-codex", "OpenAI / Codex", 42.0))
                        as Result<_, ProviderFailure>
                }) as FetchFuture
            }),
        };
        let core = test_core(vec![spec]);
        assert!(
            core.try_begin_cycle(),
            "the storm's first victim owns the cycle"
        );
        let chain_core = core.clone();
        let chain = tokio::spawn(async move { chain_core.run_chain(|_| {}).await });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // 120 concurrent requests while the fetch is parked: none may start
        // a second active cycle.
        let mut handles = Vec::new();
        for _ in 0..30 {
            let core_manual = core.clone();
            handles.push(tokio::spawn(async move { core_manual.try_begin_cycle() }));
            let core_wake = core.clone();
            handles.push(tokio::spawn(async move {
                core_wake.try_begin_auto_cycle(AutoTrigger::Wake)
            }));
            let core_reconnect = core.clone();
            handles.push(tokio::spawn(async move {
                core_reconnect.try_begin_auto_cycle(AutoTrigger::Reconnect)
            }));
            let core_reset = core.clone();
            handles.push(tokio::spawn(async move {
                core_reset.try_begin_auto_cycle(AutoTrigger::ResetWake)
            }));
        }
        for handle in handles {
            assert!(
                !handle.await.unwrap(),
                "no storm request may own a second active cycle"
            );
        }

        // Release the parked fetch and the follow-up's fetch.
        gate.add_permits(2);
        let _ = chain.await;
        // Every request collapsed into the single pending follow-up: two
        // cycles, two fetches — the rerun was never silently lost.
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "exactly one coalesced follow-up must run for the whole storm"
        );
        assert!(
            !core.finish_cycle(),
            "the chain is closed after the follow-up"
        );
        let snapshot = core.snapshot();
        assert!(!snapshot.cycle_in_flight);
        assert_eq!(snapshot.seq, 2);
    }

    // 21 + 23. a provider whose job completes late (a slow/hung upstream the
    //     backend timeout will eventually cut off). Invariants pinned: the
    //     late job's cycle commits exactly once as its own generation, a
    //     request arriving meanwhile coalesces instead of racing, and the
    //     newest cycle's state is what survives — a late completion can
    //     never overwrite newer state (state commits once per cycle under
    //     one lock, after every job has joined).
    #[tokio::test]
    async fn a_late_provider_completion_commits_once_and_cannot_overwrite_newer_state() {
        let codex_calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let gate_fetch = gate.clone();
        let calls_fetch = codex_calls.clone();
        let codex_spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                let gate = gate_fetch.clone();
                Box::pin(async move {
                    let nth = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if nth == 1 {
                        // Cycle 1's job parks here; the runtime cannot
                        // commit a partial cycle around it.
                        let _permit = gate.acquire_owned().await.unwrap();
                        Ok(ok_usage("openai-codex", "OpenAI / Codex", 11.0))
                    } else {
                        Ok(ok_usage("openai-codex", "OpenAI / Codex", 22.0))
                    }
                }) as FetchFuture
            }),
        };
        let zai_calls = Arc::new(AtomicUsize::new(0));
        let zai_spec = counting_spec(ProviderKind::Zai, zai_calls.clone(), |nth| {
            Ok(ok_usage("zai", "Z.ai", 100.0 + nth as f64))
        });
        let core = test_core(vec![codex_spec, zai_spec]);

        assert!(core.try_begin_cycle(), "the scheduled cycle starts");
        let snapshots = Arc::new(Mutex::new(Vec::<RuntimeSnapshot>::new()));
        let sink = snapshots.clone();
        let chain_core = core.clone();
        let chain = tokio::spawn(async move {
            chain_core
                .run_chain(move |event| {
                    if let RuntimeEvent::Snapshot(snapshot) = event {
                        sink.lock().unwrap().push(snapshot);
                    }
                })
                .await
        });
        while codex_calls.load(Ordering::SeqCst) == 0 || zai_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Requests landing while the late job is still parked coalesce into
        // the one pending follow-up — they never start a competing cycle.
        assert!(!core.try_begin_cycle(), "manual mid-cycle coalesces");
        assert!(
            !core.try_begin_auto_cycle(AutoTrigger::ResetWake),
            "a reset wake mid-cycle coalesces"
        );

        gate.add_permits(1);
        let _ = chain.await;

        // The late job's cycle committed exactly once (both providers), then
        // the coalesced follow-up ran: two cycles, two snapshots, seq in
        // commit order.
        assert_eq!(codex_calls.load(Ordering::SeqCst), 2);
        assert_eq!(zai_calls.load(Ordering::SeqCst), 2);
        let all = snapshots.lock().unwrap().clone();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].seq, 1, "the late job's cycle is generation 1");
        assert_eq!(all[0].providers[0].limits[0].used_percent, 11.0);
        assert_eq!(all[0].providers[1].limits[0].used_percent, 101.0);
        assert_eq!(all[1].seq, 2, "the follow-up committed after it");
        assert_eq!(all[1].providers[0].limits[0].used_percent, 22.0);
        assert_eq!(all[1].providers[1].limits[0].used_percent, 102.0);
        assert!(!all[1].cycle_in_flight);
        // Nothing landed after the newest commit: the final state is the
        // newest cycle's, and no late result can resurrect older values.
        assert_eq!(core.snapshot().seq, 2);
        assert_eq!(core.snapshot().providers[0].limits[0].used_percent, 22.0);
    }

    // 11 + 12. several providers fail in the same cycle with different fault
    //     classes while a sibling succeeds. Invariants pinned: each entry
    //     carries exactly its own failure category, every provider follows
    //     its own retry/cooldown policy without cross-contamination, and the
    //     healthy sibling still publishes (the cycle stays usable).
    #[tokio::test]
    async fn multiple_providers_failing_in_the_same_cycle_stay_isolated() {
        let codex_calls = Arc::new(AtomicUsize::new(0));
        let zai_calls = Arc::new(AtomicUsize::new(0));
        let opencode_calls = Arc::new(AtomicUsize::new(0));
        let grok_calls = Arc::new(AtomicUsize::new(0));
        let specs = vec![
            counting_spec(ProviderKind::Codex, codex_calls.clone(), |_| {
                Err(transient_failure("network"))
            }),
            counting_spec(ProviderKind::Zai, zai_calls.clone(), |_| {
                Err(permanent_failure())
            }),
            counting_spec(ProviderKind::OpenCodeGo, opencode_calls.clone(), |_| {
                Err(server_error_failure(Some(10 * 60_000)))
            }),
            counting_spec(ProviderKind::Grok, grok_calls.clone(), |nth| {
                Ok(ok_usage("grok", "Grok (xAI)", 10.0 + nth as f64))
            }),
        ];
        let (core, clock) = manual_clock_core(specs);
        core.run_cycle().await;

        let providers = &core.snapshot().providers;
        assert_eq!(providers.len(), 4, "every registered provider surfaces");
        let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["openai-codex", "zai", "opencode-go", "grok"]);

        // Codex: transport failure, retried once, then the ordinary error.
        assert_eq!(providers[0].status, "error");
        assert_eq!(providers[0].health, ProviderHealth::Error);
        assert_eq!(providers[0].error_category.as_deref(), Some("network"));
        assert!(providers[0].limits.is_empty());
        // Zai: deterministic auth failure — no retry, source-absent health.
        assert_eq!(providers[1].health, ProviderHealth::Unavailable);
        assert_eq!(providers[1].error_category.as_deref(), Some("auth_expired"));
        // OpenCode Go: 503 + Retry-After — no fast retry, cooldown recorded.
        assert_eq!(providers[2].health, ProviderHealth::Error);
        assert_eq!(
            providers[2].error_category.as_deref(),
            Some("unexpected_response")
        );
        assert_eq!(providers[2].error_http_status, Some(503));
        // Grok: the healthy sibling publishes normally in the same cycle.
        assert_eq!(providers[3].health, ProviderHealth::Live);
        assert_eq!(providers[3].limits[0].used_percent, 11.0);

        // Each provider followed its own retry policy — no cross-wiring.
        assert_eq!(
            codex_calls.load(Ordering::SeqCst),
            2,
            "transport: one retry"
        );
        assert_eq!(zai_calls.load(Ordering::SeqCst), 1, "auth: never retried");
        assert_eq!(
            opencode_calls.load(Ordering::SeqCst),
            1,
            "Retry-After: no fast retry"
        );
        assert_eq!(grok_calls.load(Ordering::SeqCst), 1);
        assert!(
            core.snapshot().cycle_succeeded,
            "one healthy provider keeps the cycle usable"
        );

        // The cooldown is per-provider: the next cycle skips only OpenCode
        // Go while the siblings refresh (Grok via its cadence cache).
        advance(&clock, 60_000);
        core.run_cycle().await;
        assert_eq!(
            opencode_calls.load(Ordering::SeqCst),
            1,
            "the cooldown skip performs no fetch"
        );
        let providers = &core.snapshot().providers;
        assert_eq!(providers[2].health, ProviderHealth::Cooldown);
        assert_eq!(
            providers[2].error_category.as_deref(),
            Some("unexpected_response")
        );
        assert_eq!(codex_calls.load(Ordering::SeqCst), 4);
        assert_eq!(zai_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            grok_calls.load(Ordering::SeqCst),
            1,
            "the cadence cache serves grok inside its window"
        );
        assert_eq!(providers[3].health, ProviderHealth::Live);
        assert!(core.snapshot().cycle_succeeded);
        // Diagnostics see the cooldown on the failing provider only.
        let diagnostics = core.diagnostic_source(fixed_now());
        let opencode = diagnostics
            .providers
            .iter()
            .find(|p| p.usage.id == "opencode-go")
            .unwrap();
        assert!(opencode.cooldown_until.is_some());
        let grok = diagnostics
            .providers
            .iter()
            .find(|p| p.usage.id == "grok")
            .unwrap();
        assert!(grok.cooldown_until.is_none());
    }

    // 1 + 13. panic containment edges: a non-string panic payload from an
    //     async fetch, and a fetch closure that panics synchronously before
    //     any future exists. Both become the ordinary structured failure
    //     ("unexpected" category, no retry burn, no cooldown) while the
    //     healthy sibling publishes. Invariants: a panic is a bounded
    //     provider failure; diagnostics retain the safe category with no
    //     cooldown evidence and no invented data.

    /// A fetch future that panics with a non-string payload — neither `&str`
    /// nor `String` — must be contained with the same withheld-payload
    /// sentence as every other payload shape.
    #[allow(unreachable_code)] // panic_any types as () although it never returns
    async fn non_string_panic_fetch() -> Result<ProviderUsageDto, ProviderFailure> {
        std::panic::panic_any(vec![7_i32, 7, 7]);
        unreachable!("panic_any must not return")
    }

    #[tokio::test]
    async fn panic_containment_covers_non_string_payloads_and_synchronous_closure_panics() {
        let codex_calls = Arc::new(AtomicUsize::new(0));
        let calls_fetch = codex_calls.clone();
        let codex_spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    non_string_panic_fetch().await
                }) as FetchFuture
            }),
        };
        let zai_calls = Arc::new(AtomicUsize::new(0));
        let zai_calls_fetch = zai_calls.clone();
        let zai_spec = ProviderSpec {
            kind: ProviderKind::Zai,
            fetch: Arc::new(move || -> FetchFuture {
                // The panic fires when the closure itself is invoked —
                // before any future exists to poll.
                zai_calls_fetch.fetch_add(1, Ordering::SeqCst);
                panic!("sync closure boom")
            }),
        };
        let grok_calls = Arc::new(AtomicUsize::new(0));
        let grok_spec = counting_spec(ProviderKind::Grok, grok_calls.clone(), |_| {
            Ok(ok_usage("grok", "Grok (xAI)", 10.0))
        });
        let core = test_core(vec![codex_spec, zai_spec, grok_spec]);
        core.run_cycle().await;

        let providers = &core.snapshot().providers;
        assert_eq!(providers.len(), 3, "panicked providers stay in the cycle");
        assert_eq!(providers[0].status, "error");
        assert_eq!(providers[0].health, ProviderHealth::Error);
        assert_eq!(
            providers[0].error_category.as_deref(),
            Some("unexpected"),
            "a non-string payload still lands in the shared category"
        );
        // Both payload shapes — a non-string payload and a sync &str panic —
        // surface the same fixed containment sentence: no payload text ever
        // rides the snapshot message.
        let codex_error = providers[0].error.as_deref().unwrap_or_default();
        assert!(
            codex_error.contains("provider fetch panicked (payload withheld)"),
            "got: {codex_error}"
        );
        assert!(
            providers[0].limits.is_empty(),
            "no quota data can be invented"
        );
        assert_eq!(
            providers[1].status, "error",
            "a synchronous closure panic is contained too"
        );
        assert_eq!(providers[1].health, ProviderHealth::Error);
        assert_eq!(providers[1].error_category.as_deref(), Some("unexpected"));
        let sync_error = providers[1].error.as_deref().unwrap_or_default();
        assert!(
            sync_error.contains("provider fetch panicked (payload withheld)")
                && !sync_error.contains("boom"),
            "got: {sync_error}"
        );
        // The healthy sibling publishes in the same cycle.
        assert_eq!(providers[2].health, ProviderHealth::Live);
        assert!(core.snapshot().cycle_succeeded);

        // A panic is a local defect: no retry burn for either shape, and no
        // cooldown recorded anywhere.
        assert_eq!(codex_calls.load(Ordering::SeqCst), 1);
        assert_eq!(zai_calls.load(Ordering::SeqCst), 1);
        let diagnostics = core.diagnostic_source(fixed_now());
        for provider in &diagnostics.providers {
            assert!(
                provider.cooldown_until.is_none(),
                "a panic must never produce cooldown evidence"
            );
        }
        let codex_source = &diagnostics.providers[0];
        assert_eq!(
            codex_source.last_error.as_ref().map(|e| e.code.clone()),
            Some("unexpected".to_string())
        );
        assert_eq!(codex_source.last_error.as_ref().unwrap().http_status, None);
    }

    // Panic containment edge (companion to `1 + 13`): a `String`-shaped
    // payload — what `panic!("... {}", value)` produces — must be withheld
    // exactly like `&str` and non-string payloads. This is the payload shape
    // the pre-fix containment actually read and forwarded.

    /// A fetch future that panics with a `String` payload.
    #[allow(unreachable_code)] // panic_any types as () although it never returns
    async fn string_panic_fetch() -> Result<ProviderUsageDto, ProviderFailure> {
        std::panic::panic_any(format!("boom string {}", 42));
        unreachable!("panic_any must not return")
    }

    #[tokio::test]
    async fn panic_containment_withholds_string_payloads() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    string_panic_fetch().await
                }) as FetchFuture
            }),
        };
        let core = test_core(vec![spec]);
        core.run_cycle().await;

        let provider = &core.snapshot().providers[0];
        assert_eq!(provider.status, "error");
        assert_eq!(provider.health, ProviderHealth::Error);
        assert_eq!(
            provider.error_category.as_deref(),
            Some("unexpected"),
            "a String payload still lands in the shared category"
        );
        let error = provider.error.as_deref().unwrap_or_default();
        assert!(
            error.contains("provider fetch panicked (payload withheld)"),
            "got: {error}"
        );
        assert!(
            !error.contains("boom"),
            "String payload text leaked: {error}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry burn for a panic");
    }

    // 9 (Grok). a structurally successful refresh that reports zero windows.
    //     Invariants pinned: the entry reads unknown (never 0%, never an
    //     error), the retained last-good windows stay visible, the cycle is
    //     not claimed successful, and the zero-window result never becomes
    //     last good.
    #[tokio::test]
    async fn grok_zero_window_success_surfaces_unknown_and_keeps_last_good() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Grok, calls.clone(), |nth| match nth {
            1 => Ok(ok_usage("grok", "Grok (xAI)", 33.0)),
            2 => {
                // Upstream answered but reported nothing usable.
                let mut empty = ok_usage("grok", "Grok (xAI)", 0.0);
                empty.limits = Vec::new();
                Ok(empty)
            }
            _ => Ok(ok_usage("grok", "Grok (xAI)", 40.0)),
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await;
        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Live);
        let updated_at_after_success = core.snapshot().last_updated_at.clone();

        advance(&clock, GROK_MIN_POLL_INTERVAL_MS + 60_000);
        core.run_cycle().await;
        let zero = &core.snapshot().providers[0];
        assert_eq!(zero.status, "unknown");
        assert_eq!(zero.health, ProviderHealth::Unknown);
        assert_eq!(zero.error, None, "an unknown verdict is not a failure");
        // The last good windows are retained (not retired) and shown.
        assert_eq!(zero.limits.len(), 1);
        assert_eq!(zero.limits[0].used_percent, 33.0);
        // An all-unknown cycle is not a successful data refresh.
        assert!(!core.snapshot().cycle_succeeded);
        assert_eq!(
            core.snapshot().last_updated_at,
            updated_at_after_success,
            "unknown data does not move last_updated_at"
        );

        advance(&clock, GROK_MIN_POLL_INTERVAL_MS + 60_000);
        core.run_cycle().await;
        let recovered = &core.snapshot().providers[0];
        assert_eq!(recovered.health, ProviderHealth::Live);
        assert_eq!(
            recovered.limits[0].used_percent, 40.0,
            "the zero-window result never became last good"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(core.snapshot().cycle_succeeded);
    }

    // 7 + 9 (non-Grok). an HTTP-2xx-shaped success with no usable windows,
    //     through the real Codex normalizer. Invariants pinned: the entry
    //     publishes as unknown — not live, not an error — the cycle is not
    //     claimed successful, and diagnostics record a structural success
    //     rather than a fabricated failure.
    #[tokio::test]
    async fn empty_window_success_publishes_unknown_and_never_claims_cycle_success() {
        let payload = normalize_codex(
            Ok(CodexUsage {
                limits: Vec::new(),
                plan_type: None,
                account: None,
                reset_credits: None,
            }),
            fixed_now().timestamp_millis(),
        )
        .expect("an empty success normalizes to the unknown health");
        assert_eq!(payload.health, ProviderHealth::Unknown);

        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), move |_| {
            Ok(payload.clone())
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;

        let entry = &core.snapshot().providers[0];
        assert_eq!(entry.status, "unknown");
        assert_eq!(entry.health, ProviderHealth::Unknown);
        assert_eq!(
            entry.error, None,
            "a 2xx response with no usable windows is not an error"
        );
        assert!(entry.limits.is_empty());
        assert!(
            !core.snapshot().cycle_succeeded,
            "an unknown-only cycle must not claim usable data"
        );
        assert_eq!(core.snapshot().last_updated_at, None);
        // Diagnostics see the structural success, not a fabricated failure.
        let source = &core.diagnostic_source(fixed_now()).providers[0];
        assert!(source.last_success_at.is_some());
        assert!(source.last_error.is_none());
    }

    // 15. the Antigravity cache fallback itself fails (live source down AND
    //     no usable cache). Invariants pinned: the adapter's Err flows
    //     through the ordinary failure projection — source-absent health,
    //     safe category in diagnostics, no cached windows invented, and no
    //     cooldown deferring the next attempt. (14 — the fallback succeeding
    //     after the live source fails — is pinned by
    //     `antigravity_fallback_cause_is_diagnostic_evidence_only`.)
    #[tokio::test]
    async fn antigravity_fallback_failure_projects_without_inventing_cached_data() {
        let outcome = normalize_antigravity(
            Err(antigravity::AntigravityError {
                code: "cache_missing".to_string(),
                message: "no local Antigravity cache was found".to_string(),
            }),
            fixed_now().timestamp_millis(),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Antigravity, calls.clone(), move |_| {
            outcome.clone()
        });
        let core = test_core(vec![spec]);
        core.run_cycle().await;

        let entry = &core.snapshot().providers[0];
        assert_eq!(entry.status, "error");
        assert_eq!(
            entry.health,
            ProviderHealth::Unavailable,
            "a missing cache is source-absent, not a transport error"
        );
        assert_eq!(entry.error_category.as_deref(), Some("cache_missing"));
        assert_eq!(entry.error_http_status, None);
        assert!(entry.limits.is_empty(), "no cached windows may be invented");
        assert!(!core.snapshot().cycle_succeeded);
        // Diagnostics carry the safe category; no cooldown attaches to a
        // fallback failure, so the next cycle goes back on the wire.
        let source = &core.diagnostic_source(fixed_now()).providers[0];
        assert_eq!(
            source.last_error.as_ref().map(|e| e.code.clone()),
            Some("cache_missing".to_string())
        );
        assert!(source.cooldown_until.is_none());
        core.run_cycle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "no cooldown defers the provider after a fallback failure"
        );
    }

    // 22. the cooldown expires just before a scheduled reset wake. Invariants
    //     pinned: the boundary produces exactly one real fetch, the fresh
    //     observation replaces the consumed wake stamp, the old wake instant
    //     never double-fires, and the new stamp is spent once its own guard
    //     elapses — no wake loop is possible from either stamp.
    #[tokio::test]
    async fn cooldown_expiry_at_a_reset_wake_boundary_is_one_cycle_and_the_stamp_is_spent() {
        let clock = Arc::new(Mutex::new(fixed_now()));
        let getter = clock.clone();
        let fetch_clock = clock.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let clock = fetch_clock.clone();
                let calls = calls_fetch.clone();
                Box::pin(async move {
                    let nth = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let now = *clock.lock().unwrap();
                    let reset_at = (now + chrono::Duration::minutes(12))
                        .to_rfc3339_opts(SecondsFormat::Millis, true);
                    match nth {
                        1 => Ok(usage_with_reset(
                            "openai-codex",
                            "OpenAI / Codex",
                            "5 hours",
                            Some(&reset_at),
                        )),
                        2 => Err(rate_limited_failure(Some(10 * 60_000))),
                        _ => Ok(usage_with_reset(
                            "openai-codex",
                            "OpenAI / Codex",
                            "5 hours",
                            Some(&reset_at),
                        )),
                    }
                }) as FetchFuture
            }),
        };
        let core = Arc::new(RuntimeCore::with_injections(
            vec![spec],
            5,
            Box::new(|| 0),
            Box::new(move || *getter.lock().unwrap()),
        ));

        // T0: success announcing a reset 12 minutes out.
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let first_wake =
            fixed_now() + chrono::Duration::milliseconds(12 * 60_000 + RESET_WAKE_GUARD_MS);
        assert_eq!(core.earliest_reset_wake_at(), Some(first_wake));

        // T0+1min: rate limited with a 10-minute hint — cooldown until
        // T0+11min, expiring just BEFORE the wake at T0+12m20s.
        advance(&clock, 60_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // The retained entry still carries the announced reset: the wake
        // stays scheduled through the failure.
        assert_eq!(core.earliest_reset_wake_at(), Some(first_wake));

        // T0+11min30s — past the cooldown, before the wake: the provider
        // goes back on the wire exactly once, and the fresh observation
        // REPLACES the wake stamp. The old instant is consumed by this
        // cycle; it can never fire a second one.
        advance(&clock, 10 * 60_000 + 30_000);
        core.run_cycle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "cooldown expiry restores the wire exactly once at the boundary"
        );
        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Live);
        let second_wake = fixed_now() + chrono::Duration::milliseconds(23 * 60_000 + 50_000);
        assert_eq!(
            core.earliest_reset_wake_at(),
            Some(second_wake),
            "the fresh stamp replaces the consumed wake"
        );
        // The old wake instant passes without a scheduling opportunity.
        advance(&clock, 60_000 + RESET_WAKE_GUARD_MS + 1);
        assert_eq!(core.earliest_reset_wake_at(), Some(second_wake));
        // The new stamp is spent once its own guard elapses: no spin-loop
        // from either stamp.
        advance(&clock, 11 * 60_000 + 30_000 + 1);
        assert_eq!(core.earliest_reset_wake_at(), None);
    }

    // 18 (complement of `reset_wake_does_not_bypass_an_active_cooldown`).
    //     A reset stamp whose wake fires while the provider is cooling down
    //     is consumed by the skipped cycle: it never schedules a second wake
    //     while the cooldown holds, and eligibility returns at cooldown
    //     expiry without any wake.
    #[tokio::test]
    async fn a_reset_wake_stamp_is_spent_once_fired_even_while_cooling_down() {
        let calls = Arc::new(AtomicUsize::new(0));
        let spec = counting_spec(ProviderKind::Codex, calls.clone(), |nth| match nth {
            1 => Ok(usage_with_reset(
                "openai-codex",
                "OpenAI / Codex",
                "5 hours",
                Some(&reset_at_ms(12 * 60_000)),
            )),
            _ => Err(rate_limited_failure(Some(30 * 60_000))),
        });
        let (core, clock) = manual_clock_core(vec![spec]);
        core.run_cycle().await; // ok — the reset is learned
        core.run_cycle().await; // 429 + Retry-After → cooldown until T0+31min
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Just before the wake instant the stamp is still live.
        advance(&clock, 12 * 60_000 + RESET_WAKE_GUARD_MS - 1);
        assert!(core.earliest_reset_wake_at().is_some());
        // At the wake instant (guard elapsed) the stamp is spent — the
        // scheduler cannot re-arm it, no matter how long the cooldown runs.
        advance(&clock, 2);
        assert_eq!(core.earliest_reset_wake_at(), None);
        // The wake cycle runs: the cooldown gates it, no fetch happens.
        core.run_cycle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the cooling-down provider must not be refetched by the wake"
        );
        assert_eq!(
            core.snapshot().providers[0].health,
            ProviderHealth::Cooldown
        );
        // Eligibility returns at cooldown expiry without any wake stamp.
        advance(&clock, 19 * 60_000 + 1);
        core.run_cycle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "cooldown expiry restores the wire"
        );
        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Error);
    }

    // 19. multiple reset-wake opportunities collapse into the earliest one;
    //     once that stamp is consumed, the next window's wake cascades in —
    //     the scheduler targets exactly one wake at a time and never loses
    //     the remaining opportunity.
    #[test]
    fn reset_wake_opportunities_collapse_to_the_earliest_then_cascade() {
        let first = usage_with_reset(
            "openai-codex",
            "OpenAI / Codex",
            "5 hours",
            Some(&reset_at_ms(10 * 60_000)),
        );
        let second = usage_with_reset("zai", "Z.ai", "Weekly", Some(&reset_at_ms(11 * 60_000)));
        // Both live: the earliest wins — one wake covers the collapse.
        assert_eq!(
            earliest_reset_wake_at(&[first.clone(), second.clone()], fixed_now()),
            Some(fixed_now() + chrono::Duration::milliseconds(10 * 60_000 + RESET_WAKE_GUARD_MS))
        );
        // After the first wake fired, its stamp is spent; the second window
        // still schedules its own wake.
        let after_first =
            fixed_now() + chrono::Duration::milliseconds(10 * 60_000 + RESET_WAKE_GUARD_MS + 1);
        assert_eq!(
            earliest_reset_wake_at(&[first, second], after_first),
            Some(fixed_now() + chrono::Duration::milliseconds(11 * 60_000 + RESET_WAKE_GUARD_MS))
        );
    }

    // 20 (panic interaction). a coalesced rerun requested mid-cycle survives
    //     a cycle body that panics later. Invariants pinned: the request is
    //     consumed by a real cycle (never silently lost), the panicked cycle
    //     commits no state and does not wedge the in-flight flag, and the
    //     runtime keeps collecting afterwards.
    #[tokio::test]
    async fn a_coalesced_rerun_survives_a_panicking_cycle_body() {
        let lane_path = std::env::temp_dir().join(format!(
            "runtime-fault-rerun-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        ));
        let _ = std::fs::remove_file(&lane_path);
        let lane = Arc::new(crate::notifications::NotificationLane::open(
            lane_path.clone(),
            Box::new(|_: &[crate::notifications::QuotaNotification]| {
                panic!("native delivery exploded");
            }),
        ));
        lane.set_enabled(true);

        let week_ahead = week_ahead_reset();
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let gate_fetch = gate.clone();
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let calls = calls_fetch.clone();
                let gate = gate_fetch.clone();
                let week = week_ahead.clone();
                Box::pin(async move {
                    let nth = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let _permit = gate.acquire_owned().await.unwrap();
                    let percent = if nth == 1 { 50.0 } else { 85.0 };
                    Ok(ok_usage_window(
                        "openai-codex",
                        "OpenAI / Codex",
                        "Weekly",
                        percent,
                        Some(&week),
                    ))
                }) as FetchFuture
            }),
        };
        let clock = Arc::new(Mutex::new(fixed_now()));
        let getter_clock = clock.clone();
        let core = Arc::new(
            RuntimeCore::with_injections(
                vec![spec],
                5,
                Box::new(|| 0),
                Box::new(move || *getter_clock.lock().unwrap()),
            )
            .with_notification_lane(Some(lane)),
        );

        assert!(core.try_begin_cycle());
        let snapshots = Arc::new(Mutex::new(Vec::<RuntimeSnapshot>::new()));
        let sink = snapshots.clone();
        let chain_core = core.clone();
        let chain = tokio::spawn(async move {
            chain_core
                .run_chain(move |event| {
                    if let RuntimeEvent::Snapshot(snapshot) = event {
                        sink.lock().unwrap().push(snapshot);
                    }
                })
                .await
        });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // The manual request lands mid-cycle: coalesced, not lost.
        assert!(!core.try_begin_cycle(), "the request coalesces");
        gate.add_permits(1); // cycle 1 completes: 50% baseline, no crossing

        // The follow-up cycle is proof the rerun was scheduled, not dropped.
        while calls.load(Ordering::SeqCst) == 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        gate.add_permits(1); // cycle 2: 85% crosses — the delivery sink panics
        let _ = chain.await;

        // The rerun ran (both cycles fetched) even though its own cycle body
        // panicked mid-flight: the request was consumed by a real cycle.
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let all = snapshots.lock().unwrap().clone();
        assert_eq!(
            all.len(),
            2,
            "a snapshot per chain iteration, including the panicked cycle"
        );
        assert!(!core.snapshot().cycle_in_flight, "no wedged flag");
        assert_eq!(
            core.snapshot().seq,
            1,
            "the panicked cycle committed no state"
        );
        // The runtime keeps collecting: a fresh cycle completes normally.
        advance(&clock, 60_000);
        core.run_cycle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Live);
        assert_eq!(core.snapshot().seq, 2);
        let _ = std::fs::remove_file(&lane_path);
    }
}
