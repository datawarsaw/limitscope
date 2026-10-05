//! Grok (xAI subscription) provider backend.
//!
//! Reads an existing xAI OAuth credential read-only from the user's own
//! harness stores (canonical Grok CLI `~/.grok/auth.json`, then the OpenCode
//! auth store, then the OpenCodex auth store — see
//! `docs/grok-provider-discovery.md` §1/§3/§6) and queries the subscription
//! credit pool (`GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`).
//!
//! Security boundary (discovery §3.4/§8): the token stays in this process.
//! No OAuth refresh, no refresh-token rotation, no writes to any store, no
//! browser cookies, no inference calls, and no auth material beyond
//! normalized windows and a masked account id ever crosses to the WebView or
//! the logs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, NaiveDateTime, TimeZone, TimeDelta, Utc};
use serde::Serialize;
use serde_json::Value;

const CREDITS_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
// The compatibility header value the official Grok CLI plane expects
// (discovery §4.1, corroborated by the installed harness's own probe).
const GROK_CLIENT_VERSION: &str = "0.2.93";
const USER_AGENT: &str = concat!("rate-limits/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// The device-flow client id of the official Grok CLI (discovery §1/§3.3). The
// canonical store keys its entries by `<issuer>::<client-id>`; when that key
// exists it is the store's own primary selection, so it is preferred over any
// other entry before falling back to the store's file order.
const OFFICIAL_GROK_CLI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_OIDC_KEY_PREFIX: &str = "https://auth.x.ai::";
const XAI_LEGACY_KEY: &str = "https://accounts.x.ai/sign-in";

// A token within this margin of its locally stored expiry is treated as
// expired without any network traffic. Mirrors the owning harnesses, which
// require `expires >= now + 60s` and write a 2-minute skew into their stores.
const EXPIRY_GUARD: TimeDelta = TimeDelta::seconds(60);

// ---------- data returned to the WebView (camelCase on the wire) ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokLimitWindow {
    pub label: String,
    pub used_percent: f64,
    /// Provider-reported ISO-8601 reset, preserved as sent. `None` when the
    /// upstream timestamp is missing or malformed.
    pub reset_at: Option<String>,
}

/// Masked, non-sensitive account attribution (discovery §6/§7): the stable
/// account id (`credential.accountId` / JWT `sub` / store `user_id`), reduced
/// to its first 8 characters before it crosses the wire, plus which store the
/// credential came from. Never a token or a full identifier.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokAccount {
    pub id: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokUsage {
    pub limits: Vec<GrokLimitWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<GrokAccount>,
}

/// Structured error surfaced to the frontend as
/// `{ code, message, httpStatus?, transient?, retryAfterMs? }`, mirroring the
/// other provider backends. Messages are safe to display: they never embed
/// token material or raw response bodies.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transient: Option<bool>,
    /// Server-pacing hint from a 429 `Retry-After` header (milliseconds).
    /// The frontend honors it conservatively: whenever it is present, the
    /// fast in-call retry is skipped and the next cadence poll (minutes away)
    /// serves as the retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Internal account attribution for the runtime's last-good guard: the
    /// masked identity of the credential this refresh actually attempted.
    /// Never serialized — the WebView receives the same identity through the
    /// usage DTO's `account` field.
    #[serde(skip)]
    pub identity_hint: Option<String>,
    /// Internal: the transport failure was a timeout (the request budget
    /// expired). Never serialized; the runtime's bounded-retry rule treats a
    /// transport timeout as terminal for the current cycle — the same marker
    /// `ProviderError` carries for the other live backends.
    #[serde(skip)]
    pub transport_timeout: bool,
}

impl GrokError {
    /// Deterministic failure (default): the frontend never retries it.
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            http_status: None,
            transient: Some(false),
            retry_after_ms: None,
            identity_hint: None,
            transport_timeout: false,
        }
    }

    /// Transient failure without an HTTP status (transport problems).
    fn transient(code: &str, message: impl Into<String>) -> Self {
        Self {
            transient: Some(true),
            ..Self::new(code, message)
        }
    }

    /// A non-success HTTP response with the Grok error contract:
    /// 429 → `rate_limited` (transient, `Retry-After` preserved),
    /// 401/403 → `auth_failed`, 5xx → `unexpected_response` (transient),
    /// everything else → `unexpected_response` (deterministic).
    fn http_failure(status: reqwest::StatusCode, message: impl Into<String>) -> Self {
        let code = match status {
            reqwest::StatusCode::TOO_MANY_REQUESTS => "rate_limited",
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => "auth_failed",
            _ => "unexpected_response",
        };
        Self {
            http_status: Some(status.as_u16()),
            transient: Some(
                status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error(),
            ),
            ..Self::new(code, message)
        }
    }

    /// A non-success HTTP response with the server's `Retry-After` hint
    /// attached when one is present and parseable. Only the statuses the
    /// bounded-retry family honors (429/5xx) carry the hint.
    fn http_failure_with_retry_after(
        status: reqwest::StatusCode,
        message: impl Into<String>,
        retry_after_header: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        let retry_after_ms = if status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
        {
            retry_after_header.and_then(|value| {
                crate::provider_error::retry_after_header_ms(value, now)
            })
        } else {
            None
        };
        Self {
            retry_after_ms,
            ..Self::http_failure(status, message)
        }
    }

    fn with_retry_after_ms(self, ms: Option<u64>) -> Self {
        Self { retry_after_ms: ms, ..self }
    }

    /// Marks (or clears) the internal transport-timeout marker. Only the
    /// transport mapping in `send`/`read_body` sets it.
    fn with_transport_timeout(mut self, transport_timeout: bool) -> Self {
        self.transport_timeout = transport_timeout;
        self
    }

    /// Attaches the masked credential identity of the account this refresh
    /// attempted, so the runtime can tell a failure of account A apart from
    /// a failure that arrived while account B's credential is stored (the
    /// same pattern `ProviderError` carries for the other backends).
    fn with_identity_hint(mut self, identity_hint: Option<String>) -> Self {
        self.identity_hint = identity_hint;
        self
    }
}

fn credential_missing() -> GrokError {
    GrokError::new(
        "credential_missing",
        "No xAI OAuth credential found. Sign in to xAI with the Grok CLI, OpenCode, or OpenCodex first.",
    )
}

fn credential_expired(source: &str, hint: &str) -> GrokError {
    let account_note = if hint.is_empty() {
        String::new()
    } else {
        format!(" (account {hint})")
    };
    GrokError::new(
        "credential_expired",
        format!(
            "The cached xAI credential in the {source} store{account_note} has expired. \
             Re-authenticate xAI in the owning harness; LimitScope never refreshes OAuth tokens."
        ),
    )
}

fn credential_ambiguous() -> GrokError {
    GrokError::new(
        "credential_ambiguous",
        "The OpenCodex xAI store holds several accounts without a resolvable active selection. \
         Pick the active account in OpenCodex; LimitScope will not guess one.",
    )
}

fn auth_rejected(status: reqwest::StatusCode) -> GrokError {
    GrokError::http_failure(
        status,
        "Grok rejected the credential. Re-authenticate xAI in the owning harness, then refresh again.",
    )
}

// ---------- small parsing helpers ----------

/// Parses an expiry value as stored by the different harnesses: an epoch
/// number (seconds or milliseconds) or an ISO-8601 string. The canonical
/// store's `expires_at` may be either (the harnesses parse it with JS `Date`).
fn parse_expiry_value(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        if !number.is_finite() || number <= 0.0 {
            return None;
        }
        // Anything above 1e11 cannot be whole seconds in a plausible era.
        let millis = if number >= 100_000_000_000.0 {
            number as i64
        } else {
            (number * 1000.0) as i64
        };
        return Utc.timestamp_millis_opt(millis).single();
    }
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// Conservative local expiry verdict: an absent or unparseable expiry is
/// expired (no network traffic is ever spent on an unverifiable token), and a
/// token inside the guard margin counts as expired too.
fn is_expired(expires: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match expires {
        Some(expires) => now + EXPIRY_GUARD >= expires,
        None => true,
    }
}

/// The `sub` claim of the access token's JWT payload — the stable account
/// identifier the stores already carry (discovery §6). Only the claim is
/// decoded; the token itself never leaves this module.
fn jwt_sub(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    // Tolerate padded input; JWT segments are unpadded by spec.
    let payload = payload.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("sub")?
        .as_str()
        .map(str::trim)
        .filter(|sub| !sub.is_empty())
        .map(str::to_string)
}

/// First 8 characters plus an ellipsis — the masked shape the discovery
/// contract renders (§7). A non-sensitive display id, not an identity proof.
fn mask_account_id(id: &str) -> String {
    let id = id.trim();
    let mut masked: String = id.chars().take(8).collect();
    if id.chars().count() > 8 {
        masked.push('…');
    }
    masked
}

/// The identity string the success attribution (`build_grok_attribution` in
/// the runtime) and every post-resolution failure build from the credential
/// actually attempted, so the runtime's last-good guard compares like-for-like.
/// Keys on the store-agnostic masked account id: the same account seen
/// through a different store is the same identity.
fn credential_identity(credential: &GrokCredential) -> Option<String> {
    credential
        .account_id
        .as_deref()
        .map(|id| format!("xai:{}", mask_account_id(id)))
}

fn finite_number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        return number.is_finite().then_some(number);
    }
    // The harness probe also accepts numeric strings; parity keeps a drifted
    // string-typed figure usable instead of discarding the window.
    value.as_str()?.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

/// The reset timestamp is passed through exactly as the provider sent it; a
/// value that does not parse as ISO-8601 is dropped (the UI renders the
/// window without a reset line) rather than guessed at.
fn validated_reset(raw: Option<&Value>) -> Option<String> {
    let text = raw?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(text).ok().map(|_| text.to_string())
}

fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

// ---------- credential resolution (read-only, no network) ----------

// Manual impl so the token can never reach logs through Debug formatting.
struct SecretToken(String);

impl std::fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SecretToken").field(&"<redacted>").finish()
    }
}

struct GrokCredential {
    token: SecretToken,
    /// Raw account id (`credential.accountId` / `user_id` / JWT `sub`), used
    /// for the `x-userid` header and masked before crossing the wire.
    account_id: Option<String>,
    source: &'static str,
}

/// What one store scan concluded. `Expired` is remembered across the
/// precedence chain (the highest-precedence expired credential wins the final
/// error), `Ambiguous` is terminal — a store that holds several accounts
/// without a resolvable active selection must not be guessed at.
#[derive(Debug)]
enum StoreScan {
    Selected(GrokCredential),
    Expired { source: &'static str, account: Option<String> },
    Ambiguous,
    NoEntry,
}

impl std::fmt::Debug for GrokCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrokCredential")
            .field("token", &"<redacted>")
            .field("account_id", &self.account_id.as_deref().map(mask_account_id))
            .field("source", &self.source)
            .finish()
    }
}

/// Canonical Grok CLI store (`~/.grok/auth.json`, `GROK_HOME` overridable).
/// Entries are top-level keys `<issuer>::<client-id>` (or the legacy sign-in
/// key) carrying `key` (access token), `refresh_token`, `expires_at`,
/// `auth_mode: Oidc|ApiKey`, `user_id`, `email`. API-key-mode entries are a
/// different credential world and are skipped (discovery §3.1/§10). The
/// official CLI's own client-id key is the store's primary selection; without
/// it, the first usable entry in file order is selected (the read order the
/// installed fork's read-only detector uses). One entry is selected and its
/// expiry gates it — a selection never falls through to another entry.
fn parse_grok_cli_store(raw: &str, now: DateTime<Utc>) -> StoreScan {
    let value: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return StoreScan::NoEntry,
    };
    let Some(map) = value.as_object() else {
        return StoreScan::NoEntry;
    };
    let mut candidates: Vec<(&String, &Value)> = map
        .iter()
        .filter(|(key, _)| {
            key.as_str().starts_with(XAI_OIDC_KEY_PREFIX) || key.as_str() == XAI_LEGACY_KEY
        })
        .collect();
    // Official client-id key first, everything else in file order (stable).
    candidates.sort_by_key(|(key, _)| {
        key.as_str() != format!("{XAI_OIDC_KEY_PREFIX}{OFFICIAL_GROK_CLI_CLIENT_ID}")
    });

    let Some((_, entry)) = candidates.into_iter().find(|(_, entry)| {
        let is_api_key = entry.get("auth_mode").and_then(Value::as_str) == Some("ApiKey");
        let has_access = entry
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|key| !key.is_empty());
        let has_refresh = entry
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|token| !token.is_empty());
        let oidc_mode = entry.get("auth_mode").and_then(Value::as_str) == Some("Oidc");
        !is_api_key && has_access && (has_refresh || oidc_mode)
    }) else {
        return StoreScan::NoEntry;
    };

    let access = entry
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let account_id = entry
        .get("user_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| jwt_sub(&access));
    if is_expired(parse_expiry_value(entry.get("expires_at")), now) {
        return StoreScan::Expired { source: "Grok CLI", account: account_id };
    }
    StoreScan::Selected(GrokCredential {
        token: SecretToken(access),
        account_id,
        source: "grok-cli",
    })
}

/// OpenCode auth store (`auth.json` → flat `xai` entry
/// `{type:"oauth", access, refresh, expires}`), single account.
fn parse_opencode_store(raw: &str, now: DateTime<Utc>) -> StoreScan {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return StoreScan::NoEntry;
    };
    let Some(entry) = value.get("xai") else {
        return StoreScan::NoEntry;
    };
    let Some(entry) = entry.as_object() else {
        return StoreScan::NoEntry;
    };
    // `type: "api"` is an API-key entry — a different credential world.
    if entry.get("type").and_then(Value::as_str) != Some("oauth") {
        return StoreScan::NoEntry;
    }
    let Some(access) = entry
        .get("access")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|access| !access.is_empty())
    else {
        return StoreScan::NoEntry;
    };
    let access = access.to_string();
    let account_id = jwt_sub(&access);
    if is_expired(parse_expiry_value(entry.get("expires")), now) {
        return StoreScan::Expired { source: "OpenCode", account: account_id };
    }
    StoreScan::Selected(GrokCredential {
        token: SecretToken(access),
        account_id,
        source: "opencode",
    })
}

/// OpenCodex auth store (`auth.json` → `xai` entry with `accounts[]`,
/// `activeAccountId`, `selectionRevision`). Genuine multi-account store: the
/// store's own active selection is authoritative. An expired active account
/// never falls through to a different account; several accounts without a
/// resolvable selection are terminal ambiguity (discovery §6).
fn parse_opencodex_store(raw: &str, now: DateTime<Utc>) -> StoreScan {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return StoreScan::NoEntry;
    };
    let Some(entry) = value.get("xai").and_then(Value::as_object) else {
        return StoreScan::NoEntry;
    };
    let Some(accounts) = entry.get("accounts").and_then(Value::as_array) else {
        return StoreScan::NoEntry;
    };
    let usable: Vec<&Value> = accounts
        .iter()
        .filter(|account| {
            account
                .get("credential")
                .and_then(|credential| credential.get("access"))
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|access| !access.is_empty())
        })
        .collect();
    if usable.is_empty() {
        return StoreScan::NoEntry;
    }
    let active_id = entry
        .get("activeAccountId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let selected = match active_id {
        Some(active_id) => usable
            .iter()
            .find(|account| {
                account.get("id").and_then(Value::as_str) == Some(active_id)
            })
            .copied(),
        None => None,
    };
    let Some(selected) = selected.or_else(|| {
        // No active selection (or it points nowhere): a single account is
        // unambiguous; several are a guess — refuse.
        if usable.len() == 1 {
            Some(usable[0])
        } else {
            None
        }
    }) else {
        return StoreScan::Ambiguous;
    };

    let credential = selected.get("credential").expect("filtered above");
    let access = credential
        .get("access")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let account_id = credential
        .get("accountId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| jwt_sub(&access));
    if is_expired(parse_expiry_value(credential.get("expires")), now) {
        return StoreScan::Expired { source: "OpenCodex", account: account_id };
    }
    StoreScan::Selected(GrokCredential {
        token: SecretToken(access),
        account_id,
        source: "opencodex",
    })
}

/// The three store files in documented precedence (discovery §1/§3.2).
#[derive(Debug, Default)]
struct StorePaths {
    grok_cli_auth: PathBuf,
    opencode_auth: PathBuf,
    opencodex_auth: PathBuf,
}

fn grok_cli_auth_path(grok_home: Option<&str>, home: &Path) -> PathBuf {
    match grok_home.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("auth.json"),
        None => home.join(".grok").join("auth.json"),
    }
}

fn opencode_auth_path(xdg_data_home: Option<&str>, home: &Path) -> PathBuf {
    // Same XDG resolution OpenCode itself uses (mirrors opencode_go.rs).
    if let Some(xdg) = xdg_data_home.map(str::trim).filter(|dir| !dir.is_empty()) {
        let dir = PathBuf::from(xdg).join("opencode");
        if dir.exists() {
            return dir.join("auth.json");
        }
    }
    home.join(".local").join("share").join("opencode").join("auth.json")
}

fn opencodex_auth_path(opencodex_home: Option<&str>, home: &Path) -> PathBuf {
    match opencodex_home.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("auth.json"),
        None => home.join(".opencodex").join("auth.json"),
    }
}

fn store_paths_from_env() -> Result<StorePaths, GrokError> {
    let home = std::env::home_dir().ok_or_else(|| {
        GrokError::new("unexpected", "Could not locate the user home directory.")
    })?;
    Ok(StorePaths {
        grok_cli_auth: grok_cli_auth_path(
            std::env::var("GROK_HOME").ok().as_deref(),
            &home,
        ),
        opencode_auth: opencode_auth_path(
            std::env::var("XDG_DATA_HOME").ok().as_deref(),
            &home,
        ),
        opencodex_auth: opencodex_auth_path(
            std::env::var("OPENCODEX_HOME").ok().as_deref(),
            &home,
        ),
    })
}

fn read_store(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// Walks the store precedence chain. A store that is absent, unreadable, or
/// holds no usable xAI OAuth entry is skipped; the first store with a usable
/// selected credential wins; an expired selection is remembered (the
/// highest-precedence one reports the final error) and the walk continues —
/// never into a different account of the same store.
fn resolve_credential_in(paths: &StorePaths, now: DateTime<Utc>) -> Result<GrokCredential, GrokError> {
    let mut expired: Option<StoreScan> = None;
    for (path, parser) in [
        (&paths.grok_cli_auth, parse_grok_cli_store as fn(&str, DateTime<Utc>) -> StoreScan),
        (&paths.opencode_auth, parse_opencode_store),
        (&paths.opencodex_auth, parse_opencodex_store),
    ] {
        let scan = match read_store(path) {
            Some(raw) => parser(&raw, now),
            None => StoreScan::NoEntry,
        };
        match scan {
            StoreScan::Selected(credential) => return Ok(credential),
            StoreScan::Expired { .. } => {
                if expired.is_none() {
                    expired = Some(scan);
                }
            }
            StoreScan::Ambiguous => return Err(credential_ambiguous()),
            StoreScan::NoEntry => continue,
        }
    }
    match expired {
        Some(StoreScan::Expired { source, account }) => {
            let hint = account.as_deref().map(mask_account_id).unwrap_or_default();
            // The expired credential was the one selected for the attempt:
            // its identity is known even though no request was made, so a
            // last-good snapshot from a different account must not survive it.
            let identity = account
                .as_deref()
                .map(|id| format!("xai:{}", mask_account_id(id)));
            Err(credential_expired(source, &hint).with_identity_hint(identity))
        }
        _ => Err(credential_missing()),
    }
}

fn resolve_credential(now: DateTime<Utc>) -> Result<GrokCredential, GrokError> {
    let paths = store_paths_from_env()?;
    resolve_credential_in(&paths, now)
}

// ---------- upstream response parsing ----------

/// Outcome of parsing the weekly-credits payload. `NoWindow` is the
/// discovery-permitted zero-window success (percent absent ⇒ unknown usage ⇒
/// zero windows, never 0% — §5/§10); `SchemaDrift` is everything the schema
/// does not explicitly permit (non-JSON body, missing `config`, wrong-typed
/// percent).
#[derive(Debug)]
enum CreditsOutcome {
    Windows(Vec<GrokLimitWindow>),
    NoWindow,
    SchemaDrift(GrokError),
}

fn credits_schema_drift(reason: &str) -> GrokError {
    GrokError::new(
        "unexpected_response",
        format!("Grok billing response format changed; {reason}."),
    )
}

/// Parses `GET /v1/billing?format=credits` (discovery §4.2/§5): one primary
/// weekly credit-pool window (`config.creditUsagePercent` against
/// `currentPeriod.type == "USAGE_PERIOD_TYPE_WEEKLY"`, reset from
/// `currentPeriod.end`, fallback `billingPeriodEnd`), plus an on-demand
/// window appended only when `onDemandCap.val > 0`. `productUsage` rows are
/// shares of the same pool and never become windows.
fn parse_credits(body: &[u8]) -> CreditsOutcome {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return CreditsOutcome::SchemaDrift(credits_schema_drift("the body was not usable JSON")),
    };
    let Some(config) = parsed.get("config").and_then(Value::as_object) else {
        return CreditsOutcome::SchemaDrift(credits_schema_drift("no billing config found"));
    };
    let percent = match config.get("creditUsagePercent") {
        None => return CreditsOutcome::NoWindow,
        Some(raw) => match finite_number(Some(raw)) {
            Some(percent) => percent,
            None => {
                return CreditsOutcome::SchemaDrift(credits_schema_drift(
                    "the credit usage figure is not a number",
                ))
            }
        },
    };
    let period_type = config
        .get("currentPeriod")
        .and_then(|period| period.get("type"))
        .and_then(Value::as_str);
    if period_type != Some("USAGE_PERIOD_TYPE_WEEKLY") {
        // Not the weekly-credit schema the window is defined by; the harness
        // hard-requires this exact type. Weekly window unavailable.
        return CreditsOutcome::NoWindow;
    }
    let period = config.get("currentPeriod");
    let reset_at = validated_reset(period.and_then(|period| period.get("end")))
        .or_else(|| validated_reset(config.get("billingPeriodEnd")));

    let mut limits = vec![GrokLimitWindow {
        label: "Weekly credits".to_string(),
        used_percent: clamp_percent(percent),
        reset_at: reset_at.clone(),
    }];
    // On-demand sub-cap: a real independent quota only when the cap is
    // present and positive; a missing/zero cap emits nothing (discovery §5).
    let cap = finite_number(
        config
            .get("onDemandCap")
            .and_then(|cap| cap.get("val")),
    );
    let used = finite_number(
        config
            .get("onDemandUsed")
            .and_then(|used| used.get("val")),
    );
    if let Some(cap) = cap.filter(|cap| *cap > 0.0) {
        if let Some(used) = used {
            limits.push(GrokLimitWindow {
                label: "On-demand".to_string(),
                used_percent: clamp_percent(used / cap * 100.0),
                reset_at,
            });
        }
    }
    CreditsOutcome::Windows(limits)
}

/// Outcome of parsing the legacy monthly-dollar payload
/// (`GET /v1/billing`, discovery §4.3) — fallback only, never combined with
/// the weekly-credit schema.
#[derive(Debug)]
enum LegacyOutcome {
    Window(GrokLimitWindow),
    NoWindow,
    /// The legacy body drifted; its error never surfaces — the primary
    /// weekly outcome governs the result (see `assemble_usage`).
    SchemaDrift,
}

fn parse_legacy(body: &[u8]) -> LegacyOutcome {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return LegacyOutcome::SchemaDrift,
    };
    let Some(config) = parsed.get("config").and_then(Value::as_object) else {
        return LegacyOutcome::SchemaDrift;
    };
    let Some(limit_cents) = finite_number(config.get("monthlyLimit").and_then(|v| v.get("val")))
    else {
        return LegacyOutcome::NoWindow;
    };
    let Some(used_cents) = finite_number(config.get("used").and_then(|v| v.get("val"))) else {
        return LegacyOutcome::NoWindow;
    };
    if limit_cents <= 0.0 {
        return LegacyOutcome::NoWindow;
    }
    LegacyOutcome::Window(GrokLimitWindow {
        label: "Monthly credits".to_string(),
        used_percent: clamp_percent(used_cents / limit_cents * 100.0),
        reset_at: validated_reset(config.get("billingPeriodEnd")),
    })
}

/// Combines the weekly-credits outcome with the legacy fallback outcome.
///
/// Rules (discovery §4.3/§5/§10): the weekly window wins whenever it exists;
/// the legacy dollar pool is only consulted as a conservative fallback and
/// the two schemas are never merged into fabricated totals. A structurally
/// valid weekly payload without a usable percent resolves to a zero-window
/// success (the one no-window success the schema permits); a drifted weekly
/// payload surfaces as `unexpected_response` once the fallback also fails to
/// produce a window.
fn assemble_usage(
    weekly: CreditsOutcome,
    legacy: Result<LegacyOutcome, GrokError>,
    account: Option<GrokAccount>,
) -> Result<GrokUsage, GrokError> {
    let drift = match &weekly {
        CreditsOutcome::SchemaDrift(error) => Some(error.clone()),
        _ => None,
    };
    match weekly {
        CreditsOutcome::Windows(windows) if !windows.is_empty() => {
            Ok(GrokUsage { limits: windows, account })
        }
        _ => match legacy {
            Ok(LegacyOutcome::Window(window)) => Ok(GrokUsage {
                limits: vec![window],
                account,
            }),
            // The fallback failed, but the primary endpoint answered with a
            // structurally valid schema and simply no percent: the permitted
            // zero-window success stands (status "unknown" upstream of here).
            _ => match drift {
                Some(error) => Err(error),
                None => Ok(GrokUsage { limits: vec![], account }),
            },
        },
    }
}

// ---------- HTTP ----------

fn http_client() -> Result<&'static reqwest::Client, GrokError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            // A redirect would carry the Authorization header to whatever host
            // the redirect names. The harness probe hard-fails redirects too
            // (`redirect: "error"`); a 3xx surfaces as a plain HTTP failure.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(client) => Ok(client),
        Err(error) => Err(GrokError::new(
            "unexpected",
            format!("Could not initialize HTTP client: {error}"),
        )),
    }
}

/// The exact header set the installed harness's quota probe sends
/// (discovery §4.1). `x-userid` echoes the account id (stored account id,
/// JWT `sub` fallback) and is omitted only when neither exists.
fn build_credits_request(
    client: &reqwest::Client,
    token: &str,
    user_id: Option<&str>,
) -> reqwest::RequestBuilder {
    let request = client
        .get(CREDITS_URL)
        .bearer_auth(token)
        .header("accept", "application/json")
        .header("x-xai-token-auth", "xai-grok-cli")
        .header("x-authenticateresponse", "authenticate-response")
        .header("x-grok-client-version", GROK_CLIENT_VERSION);
    match user_id {
        Some(user_id) => request.header("x-userid", user_id),
        None => request,
    }
}

/// Legacy fallback request: bearer + `Accept` only (discovery §4.3).
fn build_billing_request(client: &reqwest::Client, token: &str) -> reqwest::RequestBuilder {
    client
        .get(BILLING_URL)
        .bearer_auth(token)
        .header("accept", "application/json")
}

/// `Retry-After` in milliseconds via the one shared parser (delta-seconds
/// or HTTP-date, malformed ignored); the runtime applies the safety cap.
fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let now = Utc::now();
    crate::provider_error::retry_after_header(headers)
        .and_then(|value| crate::provider_error::retry_after_header_ms(value, now))
}

async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, GrokError> {
    let response = request.send().await.map_err(|error| {
        GrokError::transient(
            "network_error",
            format!("Could not reach the Grok billing endpoint: {error}"),
        )
        .with_transport_timeout(error.is_timeout())
    })?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(auth_rejected(status));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after_ms = retry_after_ms(response.headers());
        return Err(
            GrokError::http_failure(
                status,
                "Grok billing endpoint is rate limiting requests; try again later.",
            )
            .with_retry_after_ms(retry_after_ms),
        );
    }
    Ok(response)
}

async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, GrokError> {
    let status = response.status();
    let retry_after = crate::provider_error::retry_after_header(response.headers()).map(String::from);
    let body = response.bytes().await.map_err(|error| {
        GrokError::transient(
            "network_error",
            format!("Grok billing response was cut short: {error}"),
        )
        .with_transport_timeout(error.is_timeout())
    })?;
    if !status.is_success() {
        return Err(GrokError::http_failure_with_retry_after(
            status,
            format!("Grok billing endpoint returned HTTP {status}."),
            retry_after.as_deref(),
            Utc::now(),
        ));
    }
    Ok(body.to_vec())
}

// ---------- fetch + command ----------

async fn fetch_grok_usage() -> Result<GrokUsage, GrokError> {
    // Credential resolution never touches the network; an expired or missing
    // credential fails here without a single request (discovery §10).
    let credential = resolve_credential(Utc::now())?;
    // The attempted account is known as soon as the credential is resolved;
    // every failure after this point carries its masked identity so the
    // runtime can never retain one account's last-good data for a different
    // account (the same guard pattern the OpenCode Go backend stamps).
    let identity = credential_identity(&credential);
    fetch_usage_with_credential(&credential)
        .await
        .map_err(|mut error| {
            // Final-boundary value scrub (`secret_scrub`): the token is alive
            // for this whole fetch, so any exact occurrence of it in a
            // failure message is removed before the error leaves the adapter.
            error.message =
                crate::secret_scrub::scrub_text(&error.message, &[credential.token.0.as_str()]);
            error.with_identity_hint(identity)
        })
}

/// The usage fetch for an already-resolved credential. Cadence semantics are
/// the runtime's concern (the 15-minute gate above this call); nothing here
/// caches or refreshes — an expired or rejected token stays failed until the
/// owning harness re-authenticates.
async fn fetch_usage_with_credential(credential: &GrokCredential) -> Result<GrokUsage, GrokError> {
    let client = http_client()?;
    let user_id = credential
        .account_id
        .clone()
        .or_else(|| jwt_sub(&credential.token.0));
    let account = credential.account_id.as_deref().map(|id| GrokAccount {
        id: mask_account_id(id),
        source: credential.source.to_string(),
    });

    let weekly = {
        let response = send(build_credits_request(
            client,
            &credential.token.0,
            user_id.as_deref(),
        ))
        .await?;
        parse_credits(&read_body(response).await?)
    };
    let weekly = match weekly {
        CreditsOutcome::Windows(windows) if !windows.is_empty() => {
            return Ok(GrokUsage {
                limits: windows,
                account,
            });
        }
        other => other,
    };
    // Weekly window unavailable (absent percent, wrong period type, or
    // schema drift): the legacy monthly dollar pool is the conservative
    // fallback — never combined with the weekly schema. A transport failure
    // of the fallback itself never masks the primary outcome.
    let legacy = async {
        let response = send(build_billing_request(client, &credential.token.0)).await?;
        Ok(parse_legacy(&read_body(response).await?))
    }
    .await;
    assemble_usage(weekly, legacy, account)
}

#[tauri::command]
pub async fn get_grok_usage() -> Result<GrokUsage, GrokError> {
    fetch_grok_usage().await
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{SecondsFormat, TimeZone};

    fn now_utc() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 28, 18, 0, 0).unwrap()
    }

    fn valid_expiry() -> Value {
        serde_json::json!("2026-09-28T23:00:00Z")
    }

    fn expired_expiry() -> Value {
        serde_json::json!("2026-09-01T00:00:00Z")
    }

    // The transport-timeout marker is internal retry-rule input: the wire
    // contract stays exactly the documented fields, and the verdict stays
    // transient so the next cadence cycle remains the honest retry path.
    #[test]
    fn transport_timeout_marker_stays_off_the_wire_and_transient() {
        let marked = GrokError::transient("network_error", "timed out").with_transport_timeout(true);
        assert_eq!(marked.transient, Some(true));
        assert!(marked.transport_timeout);
        let wire = serde_json::to_string(&marked).unwrap();
        assert!(!wire.contains("transportTimeout"), "wire: {wire}");
        assert!(!wire.contains("transport_timeout"), "wire: {wire}");
        // Constructors default to unmarked.
        assert!(!GrokError::new("auth_failed", "rejected").transport_timeout);
        assert!(!GrokError::transient("network_error", "offline").transport_timeout);
    }

    // Shape observed live from GET /v1/billing?format=credits on 2026-09-28
    // (discovery §4.2).
    fn credits_body() -> Value {
        serde_json::json!({
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-09-26T13:12:49.368933+00:00",
                    "end": "2026-10-03T13:12:49.368933+00:00"
                },
                "creditUsagePercent": 54.0,
                "onDemandCap": { "val": 0 },
                "onDemandUsed": { "val": 0 },
                "productUsage": [{ "product": "GrokBuild", "usagePercent": 54.0 }],
                "isUnifiedBillingUser": true,
                "prepaidBalance": { "val": 0 },
                "topUpMethod": "TOP_UP_METHOD_SAVED_PAYMENT_METHOD",
                "billingPeriodStart": "2026-09-26T13:12:49.368933+00:00",
                "billingPeriodEnd": "2026-10-03T13:12:49.368933+00:00"
            }
        })
    }

    // ---------- JWT / masking / expiry helpers ----------

    fn jwt_with_sub(sub: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"sub":"{sub}"}}"#));
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn jwt_sub_reads_payload_claim() {
        assert_eq!(
            jwt_sub(&jwt_with_sub("7a2d5abe-user")),
            Some("7a2d5abe-user".to_string())
        );
        // Padded segments still decode.
        let padded = format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode("header"),
            URL_SAFE_NO_PAD.encode(r#"{"sub":"padded-sub"}"#)
        );
        assert_eq!(jwt_sub(&padded), Some("padded-sub".to_string()));
        assert_eq!(jwt_sub("garbage"), None);
        assert_eq!(jwt_sub("a.b.c"), None);
    }

    #[test]
    fn masking_keeps_only_the_first_eight_characters() {
        assert_eq!(mask_account_id("7a2d5abe12345678"), "7a2d5abe…");
        assert_eq!(mask_account_id("short"), "short");
        assert_eq!(mask_account_id("  padded-id-long  "), "padded-i…");
    }

    #[test]
    fn expiry_values_accept_epoch_numbers_and_iso_strings() {
        let ms = serde_json::json!(1_788_573_485_569_i64);
        assert_eq!(
            parse_expiry_value(Some(&ms)).unwrap().to_rfc3339_opts(SecondsFormat::Millis, true),
            "2026-09-05T01:58:05.569Z"
        );
        let seconds = serde_json::json!(1_788_573_485_i64);
        assert_eq!(
            parse_expiry_value(Some(&seconds)).unwrap().to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-09-05T01:58:05Z"
        );
        let iso = serde_json::json!("2026-09-28T19:30:28Z");
        assert_eq!(
            parse_expiry_value(Some(&iso)).unwrap().to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-09-28T19:30:28Z"
        );
        // Unparseable / absent / nonsense are all "unknown" (→ expired).
        assert_eq!(parse_expiry_value(Some(&serde_json::json!("soon-ish"))), None);
        assert_eq!(parse_expiry_value(Some(&serde_json::json!(null))), None);
        assert_eq!(parse_expiry_value(None), None);
    }

    #[test]
    fn expiry_verdict_is_conservative() {
        let valid = Utc.with_ymd_and_hms(2026, 9, 28, 19, 0, 0).unwrap();
        assert!(!is_expired(Some(valid), now_utc()));
        // Inside the 60s guard margin counts as expired.
        let borderline = Utc.with_ymd_and_hms(2026, 9, 28, 18, 0, 30).unwrap();
        assert!(is_expired(Some(borderline), now_utc()));
        let past = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        assert!(is_expired(Some(past), now_utc()));
        // Unknown expiry must never read as valid.
        assert!(is_expired(None, now_utc()));
    }

    // ---------- canonical Grok CLI store ----------

    #[test]
    fn grok_cli_store_selects_the_official_client_entry() {
        let raw = serde_json::json!({
            "https://auth.x.ai::some-other-client": {
                "auth_mode": "Oidc",
                "key": "other-access",
                "refresh_token": "other-refresh",
                "expires_at": "2026-09-28T23:00:00Z",
                "user_id": "other-user"
            },
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                "auth_mode": "Oidc",
                "key": "official-access",
                "refresh_token": "official-refresh",
                "expires_at": "2026-09-28T23:00:00Z",
                "user_id": "official-user"
            }
        })
        .to_string();
        match parse_grok_cli_store(&raw, now_utc()) {
            StoreScan::Selected(credential) => {
                assert_eq!(credential.token.0, "official-access");
                assert_eq!(credential.account_id.as_deref(), Some("official-user"));
                assert_eq!(credential.source, "grok-cli");
            }
            other => panic!("expected a selected credential, got {other:?}"),
        }
    }

    #[test]
    fn grok_cli_store_skips_api_key_entries_and_requires_oauth_markers() {
        let raw = serde_json::json!({
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                "auth_mode": "ApiKey",
                "key": "xai-api-key",
                "expires_at": "2026-09-28T23:00:00Z"
            },
            "https://auth.x.ai::second-client": {
                "key": "mystery-access",
                "expires_at": "2026-09-28T23:00:00Z"
            },
            "https://auth.x.ai::third-client": {
                "auth_mode": "Oidc",
                "key": "oidc-access",
                "refresh_token": "oidc-refresh",
                "expires_at": "2026-09-28T23:00:00Z",
                "user_id": "third-user"
            }
        })
        .to_string();
        match parse_grok_cli_store(&raw, now_utc()) {
            StoreScan::Selected(credential) => {
                // The ApiKey entry is skipped; the key-less OAuth-marker entry
                // (no refresh_token, no explicit mode) is skipped too.
                assert_eq!(credential.token.0, "oidc-access");
                assert_ne!(credential.token.0, "xai-api-key");
            }
            other => panic!("expected a selected credential, got {other:?}"),
        }
    }

    #[test]
    fn grok_cli_store_reports_expired_selection_without_falling_through() {
        // Two entries: the selected (official) one is expired, the other is
        // valid. The selection stands — no silent fall-through.
        let raw = serde_json::json!({
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                "auth_mode": "Oidc",
                "key": "expired-access",
                "refresh_token": "expired-refresh",
                "expires_at": "2026-09-01T00:00:00Z",
                "user_id": "official-user"
            },
            "https://auth.x.ai::other-client": {
                "auth_mode": "Oidc",
                "key": "valid-access",
                "refresh_token": "valid-refresh",
                "expires_at": "2026-09-28T23:00:00Z",
                "user_id": "other-user"
            }
        })
        .to_string();
        match parse_grok_cli_store(&raw, now_utc()) {
            StoreScan::Expired { source, account } => {
                assert_eq!(source, "Grok CLI");
                assert_eq!(account.as_deref(), Some("official-user"));
            }
            other => panic!("expected an expired selection, got {other:?}"),
        }
    }

    #[test]
    fn grok_cli_store_treats_unparseable_expiry_as_expired() {
        let raw = serde_json::json!({
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                "auth_mode": "Oidc",
                "key": "access",
                "refresh_token": "refresh",
                "expires_at": "whenever"
            }
        })
        .to_string();
        assert!(matches!(
            parse_grok_cli_store(&raw, now_utc()),
            StoreScan::Expired { .. }
        ));
    }

    #[test]
    fn grok_cli_store_yields_no_entry_for_missing_or_malformed_content() {
        assert!(matches!(
            parse_grok_cli_store("not json", now_utc()),
            StoreScan::NoEntry
        ));
        assert!(matches!(
            parse_grok_cli_store(serde_json::json!([]).to_string().as_str(), now_utc()),
            StoreScan::NoEntry
        ));
        assert!(matches!(
            parse_grok_cli_store(serde_json::json!({}).to_string().as_str(), now_utc()),
            StoreScan::NoEntry
        ));
        // Only legacy-foreign keys → no entry.
        let foreign = serde_json::json!({ "https://github.com::x": { "key": "k" } }).to_string();
        assert!(matches!(
            parse_grok_cli_store(&foreign, now_utc()),
            StoreScan::NoEntry
        ));
    }

    #[test]
    fn grok_cli_store_accepts_the_legacy_sign_in_key() {
        let raw = serde_json::json!({
            "https://accounts.x.ai/sign-in": {
                "auth_mode": "Oidc",
                "key": "legacy-access",
                "refresh_token": "legacy-refresh",
                "expires_at": "2026-09-28T23:00:00Z"
            }
        })
        .to_string();
        match parse_grok_cli_store(&raw, now_utc()) {
            StoreScan::Selected(credential) => assert_eq!(credential.token.0, "legacy-access"),
            other => panic!("expected a selected credential, got {other:?}"),
        }
    }

    // ---------- OpenCode store ----------

    #[test]
    fn opencode_store_reads_the_flat_oauth_entry() {
        let raw = serde_json::json!({
            "openai": { "type": "oauth", "access": "oa", "refresh": "or", "expires": 1 },
            "xai": {
                "type": "oauth",
                "access": &jwt_with_sub("7cb92c5a-user"),
                "refresh": "refresh",
                "expires": 1_788_573_485_569_i64
            }
        })
        .to_string();
        // 1788573485569 ms = 2026-09-05T01:58:05Z — before `now_utc`, so the
        // real store's token classifies as expired.
        match parse_opencode_store(&raw, now_utc()) {
            StoreScan::Expired { source, account } => {
                assert_eq!(source, "OpenCode");
                assert_eq!(account.as_deref(), Some("7cb92c5a-user"));
            }
            other => panic!("expected an expired credential, got {other:?}"),
        }

        let fresh = serde_json::json!({
            "xai": {
                "type": "oauth",
                "access": &jwt_with_sub("7cb92c5a-user"),
                "refresh": "refresh",
                "expires": "2026-09-28T23:00:00Z"
            }
        })
        .to_string();
        match parse_opencode_store(&fresh, now_utc()) {
            StoreScan::Selected(credential) => {
                assert_eq!(credential.source, "opencode");
                // No stored account id: identity comes from the JWT claim.
                assert_eq!(credential.account_id.as_deref(), Some("7cb92c5a-user"));
            }
            other => panic!("expected a selected credential, got {other:?}"),
        }
    }

    #[test]
    fn opencode_store_ignores_api_type_entries_and_malformed_shapes() {
        let api = serde_json::json!({ "xai": { "type": "api", "key": "xai-key" } }).to_string();
        assert!(matches!(parse_opencode_store(&api, now_utc()), StoreScan::NoEntry));
        let empty = serde_json::json!({}).to_string();
        assert!(matches!(parse_opencode_store(&empty, now_utc()), StoreScan::NoEntry));
        let malformed = serde_json::json!({ "xai": { "type": "oauth", "access": "   ", "expires": "2026-09-28T23:00:00Z" } }).to_string();
        assert!(matches!(parse_opencode_store(&malformed, now_utc()), StoreScan::NoEntry));
        assert!(matches!(parse_opencode_store("not json", now_utc()), StoreScan::NoEntry));
    }

    // ---------- OpenCodex store ----------

    fn opencodex_body(active: Value, accounts: Value) -> String {
        serde_json::json!({
            "xai": {
                "activeAccountId": active,
                "accounts": accounts,
                "selectionRevision": "revision-1"
            }
        })
        .to_string()
    }

    fn opencodex_account(id: &str, access: &str, expires: Value, account_id: &str) -> Value {
        serde_json::json!({
            "id": id,
            "credential": {
                "access": access,
                "refresh": "refresh",
                "expires": expires,
                "accountId": account_id,
                "source": "oauth"
            },
            "addedAt": "2026-09-28T08:00:00Z"
        })
    }

    #[test]
    fn opencodex_store_selects_the_active_account() {
        let raw = opencodex_body(
            serde_json::json!("384c9cd0-active"),
            serde_json::json!([
                opencodex_account("07745294-first", "first-access", valid_expiry(), "7cb92c5a-user"),
                opencodex_account("384c9cd0-active", "active-access", valid_expiry(), "7a2d5abe-user")
            ]),
        );
        match parse_opencodex_store(&raw, now_utc()) {
            StoreScan::Selected(credential) => {
                assert_eq!(credential.token.0, "active-access");
                assert_eq!(credential.account_id.as_deref(), Some("7a2d5abe-user"));
                assert_eq!(credential.source, "opencodex");
            }
            other => panic!("expected a selected credential, got {other:?}"),
        }
    }

    #[test]
    fn opencodex_store_expired_active_account_does_not_fall_through() {
        // The active account expired today; the other account is still valid.
        // The active selection stands and reports expired.
        let raw = opencodex_body(
            serde_json::json!("07745294-first"),
            serde_json::json!([
                opencodex_account("07745294-first", "expired-access", expired_expiry(), "7cb92c5a-user"),
                opencodex_account("384c9cd0-active", "valid-access", valid_expiry(), "7a2d5abe-user")
            ]),
        );
        match parse_opencodex_store(&raw, now_utc()) {
            StoreScan::Expired { source, account } => {
                assert_eq!(source, "OpenCodex");
                assert_eq!(account.as_deref(), Some("7cb92c5a-user"));
            }
            other => panic!("expected an expired selection, got {other:?}"),
        }
    }

    #[test]
    fn opencodex_store_multiple_accounts_without_selection_is_ambiguous() {
        let no_active = opencodex_body(
            serde_json::json!(null),
            serde_json::json!([
                opencodex_account("a", "access-a", valid_expiry(), "sub-a"),
                opencodex_account("b", "access-b", valid_expiry(), "sub-b")
            ]),
        );
        assert!(matches!(
            parse_opencodex_store(&no_active, now_utc()),
            StoreScan::Ambiguous
        ));
        // A dangling activeAccountId with several accounts is equally ambiguous.
        let dangling = opencodex_body(
            serde_json::json!("missing-id"),
            serde_json::json!([
                opencodex_account("a", "access-a", valid_expiry(), "sub-a"),
                opencodex_account("b", "access-b", valid_expiry(), "sub-b")
            ]),
        );
        assert!(matches!(
            parse_opencodex_store(&dangling, now_utc()),
            StoreScan::Ambiguous
        ));
    }

    #[test]
    fn opencodex_store_single_account_without_selection_is_used() {
        let raw = opencodex_body(
            serde_json::json!(null),
            serde_json::json!([opencodex_account("only", "only-access", valid_expiry(), "only-sub")]),
        );
        match parse_opencodex_store(&raw, now_utc()) {
            StoreScan::Selected(credential) => assert_eq!(credential.token.0, "only-access"),
            other => panic!("expected a selected credential, got {other:?}"),
        }
    }

    #[test]
    fn opencodex_store_without_usable_entries_yields_no_entry() {
        assert!(matches!(
            parse_opencodex_store("not json", now_utc()),
            StoreScan::NoEntry
        ));
        let no_xai = serde_json::json!({ "kimi": {} }).to_string();
        assert!(matches!(
            parse_opencodex_store(&no_xai, now_utc()),
            StoreScan::NoEntry
        ));
        let empty_accounts = opencodex_body(serde_json::json!("x"), serde_json::json!([]));
        assert!(matches!(
            parse_opencodex_store(&empty_accounts, now_utc()),
            StoreScan::NoEntry
        ));
        let blank_access = opencodex_body(
            serde_json::json!("a"),
            serde_json::json!([opencodex_account("a", "   ", valid_expiry(), "sub-a")]),
        );
        assert!(matches!(
            parse_opencodex_store(&blank_access, now_utc()),
            StoreScan::NoEntry
        ));
    }

    // ---------- resolution precedence ----------

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rate-limits-grok-test-{}-{}",
            std::process::id(),
            name
        ));
        fs::write(&path, contents).expect("write temp store");
        path
    }

    fn paths(grok: Option<PathBuf>, opencode: Option<PathBuf>, opencodex: Option<PathBuf>) -> StorePaths {
        StorePaths {
            grok_cli_auth: grok.unwrap_or_else(|| PathBuf::from("/nonexistent/grok/auth.json")),
            opencode_auth: opencode.unwrap_or_else(|| PathBuf::from("/nonexistent/opencode/auth.json")),
            opencodex_auth: opencodex.unwrap_or_else(|| PathBuf::from("/nonexistent/opencodex/auth.json")),
        }
    }

    #[test]
    fn resolution_follows_the_documented_store_precedence() {
        // All three present: the canonical store wins.
        let grok = temp_file(
            "precedence-grok.json",
            &serde_json::json!({
                "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                    "auth_mode": "Oidc",
                    "key": "cli-access",
                    "refresh_token": "cli-refresh",
                    "expires_at": "2026-09-28T23:00:00Z"
                }
            })
            .to_string(),
        );
        let opencode = temp_file(
            "precedence-opencode.json",
            &serde_json::json!({
                "xai": { "type": "oauth", "access": "oc-access", "refresh": "r", "expires": "2026-09-28T23:00:00Z" }
            })
            .to_string(),
        );
        let opencodex = temp_file(
            "precedence-opencodex.json",
            &opencodex_body(
                serde_json::json!("a"),
                serde_json::json!([opencodex_account("a", "ocx-access", valid_expiry(), "sub-a")]),
            ),
        );
        let resolved = resolve_credential_in(
            &paths(Some(grok.clone()), Some(opencode.clone()), Some(opencodex.clone())),
            now_utc(),
        )
        .expect("credential");
        assert_eq!(resolved.token.0, "cli-access");
        assert_eq!(resolved.source, "grok-cli");

        // Canonical absent: OpenCode wins over OpenCodex.
        let resolved = resolve_credential_in(
            &paths(None, Some(opencode.clone()), Some(opencodex.clone())),
            now_utc(),
        )
        .expect("credential");
        assert_eq!(resolved.token.0, "oc-access");
        assert_eq!(resolved.source, "opencode");

        // Only OpenCodex present: it wins.
        let resolved = resolve_credential_in(&paths(None, None, Some(opencodex.clone())), now_utc())
            .expect("credential");
        assert_eq!(resolved.token.0, "ocx-access");
        assert_eq!(resolved.source, "opencodex");

        fs::remove_file(grok).ok();
        fs::remove_file(opencode).ok();
        fs::remove_file(opencodex).ok();
    }

    #[test]
    fn resolution_skips_stores_without_usable_entries_and_reports_expired_when_all_expire() {
        // Canonical store holds only an API-key entry → skipped entirely.
        let grok = temp_file(
            "skip-grok.json",
            &serde_json::json!({
                "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                    "auth_mode": "ApiKey",
                    "key": "xai-key"
                }
            })
            .to_string(),
        );
        let opencode = temp_file(
            "skip-opencode.json",
            &serde_json::json!({
                "xai": { "type": "oauth", "access": "expired-access", "refresh": "r", "expires": "2026-09-01T00:00:00Z" }
            })
            .to_string(),
        );
        let opencodex = temp_file(
            "skip-opencodex.json",
            &opencodex_body(
                serde_json::json!("a"),
                serde_json::json!([opencodex_account("a", "fresh-access", valid_expiry(), "sub-a")]),
            ),
        );
        // The expired OpenCode credential does not block the lower store.
        let resolved = resolve_credential_in(
            &paths(Some(grok.clone()), Some(opencode.clone()), Some(opencodex.clone())),
            now_utc(),
        )
        .expect("credential");
        assert_eq!(resolved.token.0, "fresh-access");
        assert_eq!(resolved.source, "opencodex");

        // With nothing usable below, the expired selection reports
        // credential_expired (not credential_missing).
        let error = resolve_credential_in(&paths(Some(grok.clone()), Some(opencode.clone()), None), now_utc())
            .expect_err("expired");
        assert_eq!(error.code, "credential_expired");
        assert_eq!(error.transient, Some(false));
        assert!(error.message.contains("OpenCode"), "message: {}", error.message);

        // No credential anywhere at all.
        let error = resolve_credential_in(&paths(None, None, None), now_utc()).expect_err("missing");
        assert_eq!(error.code, "credential_missing");

        fs::remove_file(grok).ok();
        fs::remove_file(opencode).ok();
        fs::remove_file(opencodex).ok();
    }

    #[test]
    fn resolution_surfaces_multi_account_ambiguity_instead_of_guessing() {
        let opencodex = temp_file(
            "ambiguous-opencodex.json",
            &opencodex_body(
                serde_json::json!(null),
                serde_json::json!([
                    opencodex_account("a", "access-a", valid_expiry(), "sub-a"),
                    opencodex_account("b", "access-b", valid_expiry(), "sub-b")
                ]),
            ),
        );
        let error = resolve_credential_in(&paths(None, None, Some(opencodex.clone())), now_utc())
            .expect_err("ambiguous");
        assert_eq!(error.code, "credential_ambiguous");
        fs::remove_file(opencodex).ok();
    }

    #[test]
    fn expired_credential_resolution_never_builds_a_request() {
        // Structural guarantee: resolution fails before any HTTP client or
        // request exists. The resolver is network-free by construction; this
        // pins the contract the live test relies on.
        let error = resolve_credential_in(&paths(None, None, None), now_utc()).unwrap_err();
        assert_eq!(error.code, "credential_missing");
        assert_eq!(error.http_status, None);
        assert_eq!(error.transient, Some(false));
    }

    // ---------- request shape ----------

    #[test]
    fn credits_request_carries_the_exact_discovery_headers() {
        let client = http_client().unwrap();
        let request = build_credits_request(client, "secret-token", Some("user-1"))
            .build()
            .unwrap();
        assert_eq!(request.url().as_str(), CREDITS_URL);
        let headers = request.headers();
        assert_eq!(headers.get("authorization").unwrap(), "Bearer secret-token");
        assert_eq!(headers.get("x-xai-token-auth").unwrap(), "xai-grok-cli");
        assert_eq!(
            headers.get("x-authenticateresponse").unwrap(),
            "authenticate-response"
        );
        assert_eq!(headers.get("x-grok-client-version").unwrap(), "0.2.93");
        assert_eq!(headers.get("x-userid").unwrap(), "user-1");
        assert_eq!(headers.get("accept").unwrap(), "application/json");
    }

    #[test]
    fn credits_request_omits_x_userid_when_no_identity_exists() {
        let client = http_client().unwrap();
        let request = build_credits_request(client, "secret-token", None)
            .build()
            .unwrap();
        assert!(request.headers().get("x-userid").is_none());
    }

    #[test]
    fn billing_request_sends_bearer_and_accept_only() {
        let client = http_client().unwrap();
        let request = build_billing_request(client, "secret-token").build().unwrap();
        assert_eq!(request.url().as_str(), BILLING_URL);
        let headers = request.headers();
        assert_eq!(headers.get("authorization").unwrap(), "Bearer secret-token");
        assert_eq!(headers.get("accept").unwrap(), "application/json");
        assert!(headers.get("x-userid").is_none());
        assert!(headers.get("x-grok-client-version").is_none());
    }

    // ---------- weekly payload parsing ----------

    #[test]
    fn credits_payload_parses_the_primary_weekly_window() {
        let limits = match parse_credits(credits_body().to_string().as_bytes()) {
            CreditsOutcome::Windows(limits) => limits,
            other => panic!("expected windows, got {other:?}"),
        };
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Weekly credits");
        assert_eq!(limits[0].used_percent, 54.0);
        assert_eq!(
            limits[0].reset_at.as_deref(),
            Some("2026-10-03T13:12:49.368933+00:00")
        );
    }

    #[test]
    fn credits_payload_reset_falls_back_to_billing_period_end() {
        let body = serde_json::json!({
            "config": {
                "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "nonsense" },
                "creditUsagePercent": 12.5,
                "billingPeriodEnd": "2026-10-03T13:12:49Z"
            }
        });
        let limits = match parse_credits(body.to_string().as_bytes()) {
            CreditsOutcome::Windows(limits) => limits,
            other => panic!("expected windows, got {other:?}"),
        };
        assert_eq!(limits[0].reset_at.as_deref(), Some("2026-10-03T13:12:49Z"));
    }

    #[test]
    fn credits_payload_without_period_type_or_percent_yields_no_window() {
        // Percent present, period type wrong → the weekly window is not
        // defined by this schema.
        let wrong_type = serde_json::json!({
            "config": {
                "currentPeriod": { "type": "USAGE_PERIOD_TYPE_MONTHLY", "end": "2026-10-03T00:00:00Z" },
                "creditUsagePercent": 54.0
            }
        });
        assert!(matches!(
            parse_credits(wrong_type.to_string().as_bytes()),
            CreditsOutcome::NoWindow
        ));
        // Percent absent ⇒ unknown usage ⇒ zero windows (never 0%).
        let no_percent = serde_json::json!({
            "config": {
                "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2026-10-03T00:00:00Z" }
            }
        });
        assert!(matches!(
            parse_credits(no_percent.to_string().as_bytes()),
            CreditsOutcome::NoWindow
        ));
    }

    #[test]
    fn credits_payload_with_wrong_typed_percent_is_schema_drift() {
        let body = serde_json::json!({
            "config": {
                "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2026-10-03T00:00:00Z" },
                "creditUsagePercent": "lots"
            }
        });
        match parse_credits(body.to_string().as_bytes()) {
            CreditsOutcome::SchemaDrift(error) => {
                assert_eq!(error.code, "unexpected_response");
                assert_eq!(error.transient, Some(false));
            }
            other => panic!("expected schema drift, got {other:?}"),
        }
    }

    #[test]
    fn credits_payload_accepts_numeric_strings_like_the_harness_probe() {
        let body = serde_json::json!({
            "config": {
                "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2026-10-03T00:00:00Z" },
                "creditUsagePercent": "54.5"
            }
        });
        match parse_credits(body.to_string().as_bytes()) {
            CreditsOutcome::Windows(limits) => assert_eq!(limits[0].used_percent, 54.5),
            other => panic!("expected windows, got {other:?}"),
        }
    }

    #[test]
    fn credits_payload_clamps_percent_and_ignores_product_usage() {
        let body = serde_json::json!({
            "config": {
                "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2026-10-03T00:00:00Z" },
                "creditUsagePercent": 154.0,
                "productUsage": [
                    { "product": "GrokBuild", "usagePercent": 54.0 },
                    { "product": "Other", "usagePercent": 90.0 }
                ]
            }
        });
        let limits = match parse_credits(body.to_string().as_bytes()) {
            CreditsOutcome::Windows(limits) => limits,
            other => panic!("expected windows, got {other:?}"),
        };
        // One window only: productUsage rows are shares of the same pool.
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].used_percent, 100.0);
    }

    #[test]
    fn on_demand_window_only_when_the_cap_is_positive() {
        let base = |cap: Value, used: Value| {
            serde_json::json!({
                "config": {
                    "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2026-10-03T00:00:00Z" },
                    "creditUsagePercent": 10.0,
                    "onDemandCap": cap,
                    "onDemandUsed": used
                }
            })
            .to_string()
        };
        // Live-account shape: zero cap → no second window.
        let limits = match parse_credits(base(serde_json::json!({ "val": 0 }), serde_json::json!({ "val": 0 })).as_bytes()) {
            CreditsOutcome::Windows(limits) => limits,
            other => panic!("expected windows, got {other:?}"),
        };
        assert_eq!(limits.len(), 1);

        let limits = match parse_credits(base(serde_json::json!({ "val": 4000 }), serde_json::json!({ "val": 1000 })).as_bytes()) {
            CreditsOutcome::Windows(limits) => limits,
            other => panic!("expected windows, got {other:?}"),
        };
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[1].label, "On-demand");
        assert_eq!(limits[1].used_percent, 25.0);
        assert_eq!(limits[1].reset_at, limits[0].reset_at);

        // Missing used value: no invented 0%.
        let body = serde_json::json!({
            "config": {
                "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2026-10-03T00:00:00Z" },
                "creditUsagePercent": 10.0,
                "onDemandCap": { "val": 4000 }
            }
        });
        let limits = match parse_credits(body.to_string().as_bytes()) {
            CreditsOutcome::Windows(limits) => limits,
            other => panic!("expected windows, got {other:?}"),
        };
        assert_eq!(limits.len(), 1);
    }

    #[test]
    fn credits_non_json_body_and_missing_config_are_schema_drift() {
        let html = b"<html><body>Sign in</body></html>";
        match parse_credits(html) {
            CreditsOutcome::SchemaDrift(error) => {
                assert_eq!(error.code, "unexpected_response");
                assert_eq!(error.transient, Some(false));
            }
            other => panic!("expected schema drift, got {other:?}"),
        }
        let empty = serde_json::json!({}).to_string();
        match parse_credits(empty.as_bytes()) {
            CreditsOutcome::SchemaDrift(_) => {}
            other => panic!("expected schema drift, got {other:?}"),
        }
        let config_not_object = serde_json::json!({ "config": "yes" }).to_string();
        assert!(matches!(
            parse_credits(config_not_object.as_bytes()),
            CreditsOutcome::SchemaDrift(_)
        ));
    }

    // ---------- legacy payload parsing ----------

    #[test]
    fn legacy_payload_parses_the_monthly_dollar_pool() {
        let body = serde_json::json!({
            "config": {
                "monthlyLimit": { "val": 15000 },
                "used": { "val": 7500 },
                "billingPeriodEnd": "2026-10-01T00:00:00Z"
            }
        });
        match parse_legacy(body.to_string().as_bytes()) {
            LegacyOutcome::Window(window) => {
                assert_eq!(window.label, "Monthly credits");
                assert_eq!(window.used_percent, 50.0);
                assert_eq!(window.reset_at.as_deref(), Some("2026-10-01T00:00:00Z"));
            }
            other => panic!("expected a window, got {other:?}"),
        }
    }

    #[test]
    fn legacy_payload_without_usable_amounts_yields_no_window() {
        let missing = serde_json::json!({ "config": { "billingPeriodEnd": "2026-10-01T00:00:00Z" } });
        assert!(matches!(
            parse_legacy(missing.to_string().as_bytes()),
            LegacyOutcome::NoWindow
        ));
        let zero_limit = serde_json::json!({
            "config": { "monthlyLimit": { "val": 0 }, "used": { "val": 0 } }
        });
        assert!(matches!(
            parse_legacy(zero_limit.to_string().as_bytes()),
            LegacyOutcome::NoWindow
        ));
        let no_config = serde_json::json!({}).to_string();
        assert!(matches!(
            parse_legacy(no_config.as_bytes()),
            LegacyOutcome::SchemaDrift
        ));
    }

    // ---------- assembly: weekly vs legacy vs zero-window success ----------

    fn weekly_windows() -> Vec<GrokLimitWindow> {
        vec![GrokLimitWindow {
            label: "Weekly credits".to_string(),
            used_percent: 54.0,
            reset_at: Some("2026-10-03T13:12:49Z".to_string()),
        }]
    }

    fn legacy_window() -> LegacyOutcome {
        LegacyOutcome::Window(GrokLimitWindow {
            label: "Monthly credits".to_string(),
            used_percent: 50.0,
            reset_at: None,
        })
    }

    #[test]
    fn weekly_windows_win_and_are_never_combined_with_legacy() {
        let usage = assemble_usage(
            CreditsOutcome::Windows(weekly_windows()),
            Ok(legacy_window()),
            None,
        )
        .unwrap();
        assert_eq!(usage.limits.len(), 1);
        assert_eq!(usage.limits[0].label, "Weekly credits");
    }

    #[test]
    fn legacy_window_is_the_fallback_when_weekly_is_unavailable() {
        for weekly in [
            CreditsOutcome::NoWindow,
            CreditsOutcome::Windows(vec![]),
        ] {
            let usage = assemble_usage(weekly, Ok(legacy_window()), None).unwrap();
            assert_eq!(usage.limits.len(), 1);
            assert_eq!(usage.limits[0].label, "Monthly credits");
        }
    }

    #[test]
    fn zero_window_success_only_for_the_permitted_no_percent_case() {
        // Valid schema, no percent: a successful empty result (status
        // "unknown" upstream of here) — never fabricated percentages.
        let usage = assemble_usage(CreditsOutcome::NoWindow, Ok(LegacyOutcome::NoWindow), None).unwrap();
        assert!(usage.limits.is_empty());
        let usage = assemble_usage(
            CreditsOutcome::NoWindow,
            Err(GrokError::transient("network_error", "fallback offline")),
            None,
        )
        .unwrap();
        assert!(usage.limits.is_empty());
        // Schema drift, on the other hand, surfaces as the error.
        let error = assemble_usage(
            CreditsOutcome::SchemaDrift(credits_schema_drift("drift")),
            Ok(LegacyOutcome::NoWindow),
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        let error = assemble_usage(
            CreditsOutcome::SchemaDrift(credits_schema_drift("drift")),
            Err(GrokError::transient("network_error", "fallback offline")),
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn legacy_success_rescues_a_drifted_weekly_payload() {
        let usage = assemble_usage(
            CreditsOutcome::SchemaDrift(credits_schema_drift("drift")),
            Ok(legacy_window()),
            None,
        )
        .unwrap();
        assert_eq!(usage.limits[0].label, "Monthly credits");
    }

    #[test]
    fn drifted_weekly_payload_with_failed_fallback_surfaces_the_primary_drift() {
        // The fallback's own transport failure must never mask the primary
        // endpoint's schema drift: the drifted weekly outcome governs.
        let error = assemble_usage(
            CreditsOutcome::SchemaDrift(credits_schema_drift("drift")),
            Err(GrokError::transient("network_error", "fallback offline")),
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error.message.contains("drift"), "message: {}", error.message);
        assert_eq!(error.transient, Some(false));
    }

    // ---------- error contract ----------

    #[test]
    fn error_metadata_matches_the_retry_contract() {
        assert_eq!(
            GrokError::transient("network_error", "offline").transient,
            Some(true)
        );
        assert_eq!(credential_missing().transient, Some(false));
        assert_eq!(credential_expired("OpenCodex", "7a2d5abe").transient, Some(false));
        assert_eq!(credential_ambiguous().transient, Some(false));
        assert_eq!(auth_rejected(reqwest::StatusCode::UNAUTHORIZED).code, "auth_failed");
        assert_eq!(auth_rejected(reqwest::StatusCode::FORBIDDEN).code, "auth_failed");
        assert_eq!(
            auth_rejected(reqwest::StatusCode::UNAUTHORIZED).transient,
            Some(false)
        );
        let rate_limited = GrokError::http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
        );
        assert_eq!(rate_limited.code, "rate_limited");
        assert_eq!(rate_limited.transient, Some(true));
        let server_error =
            GrokError::http_failure(reqwest::StatusCode::SERVICE_UNAVAILABLE, "down");
        assert_eq!(server_error.code, "unexpected_response");
        assert_eq!(server_error.transient, Some(true));
        assert_eq!(server_error.http_status, Some(503));
        let not_found = GrokError::http_failure(reqwest::StatusCode::NOT_FOUND, "gone");
        assert_eq!(not_found.transient, Some(false));
    }

    // The shared Retry-After parser rules, verified through the Grok call
    // shape: delta-seconds and HTTP-date are honored, malformed ignored, and
    // the runtime (not the parser) caps the wait.
    #[test]
    fn retry_after_is_parsed_via_the_shared_rules() {
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after_ms(&headers), None);
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("120"),
        );
        assert_eq!(retry_after_ms(&headers), Some(120_000));
        // An HTTP-date is a standard spelling and is honored (the old
        // delta-seconds-only guess is gone). Both sides recompute "now",
        // so allow a few milliseconds of skew between the two calls.
        let date_header = "Tue, 21 Oct 2036 07:28:00 GMT";
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static(date_header),
        );
        let parsed = retry_after_ms(&headers).expect("HTTP-date parses");
        let expected = crate::provider_error::retry_after_header_ms(date_header, Utc::now())
            .expect("reference parse");
        assert!(
            (parsed as i64 - expected as i64).abs() <= 5,
            "HTTP-date wait should match the shared parser: {parsed} vs {expected}"
        );
        // Malformed values are ignored, not guessed.
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("soon"),
        );
        assert_eq!(retry_after_ms(&headers), None);
        // Large values pass through uncapped here; the runtime's cooldown
        // cap (provider_error::MAX_COOLDOWN_MS) is the ceiling.
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("999999"),
        );
        assert_eq!(retry_after_ms(&headers), Some(999_999_000));
        assert_eq!(
            crate::provider_error::retry_after_header_ms("999999999", now),
            Some(999_999_999_000)
        );
    }

    // The 429/5xx constructor attaches the hint; other statuses never do.
    #[test]
    fn http_failure_attaches_the_retry_after_hint_on_honored_statuses() {
        let now = Utc::now();
        let limited = GrokError::http_failure_with_retry_after(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
            Some("120"),
            now,
        );
        assert_eq!(limited.retry_after_ms, Some(120_000));
        let down = GrokError::http_failure_with_retry_after(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "down",
            Some("30"),
            now,
        );
        assert_eq!(down.retry_after_ms, Some(30_000));
        let malformed = GrokError::http_failure_with_retry_after(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
            Some("bogus"),
            now,
        );
        assert_eq!(malformed.retry_after_ms, None);
        let not_found = GrokError::http_failure_with_retry_after(
            reqwest::StatusCode::NOT_FOUND,
            "gone",
            Some("120"),
            now,
        );
        assert_eq!(not_found.retry_after_ms, None);
    }

    #[test]
    fn four_twenty_nine_carries_the_retry_after_hint() {
        let error = GrokError::http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
        )
        .with_retry_after_ms(Some(120_000));
        assert_eq!(error.retry_after_ms, Some(120_000));
    }

    #[test]
    fn error_wire_format_is_camel_case_and_omits_absent_metadata() {
        let wire = serde_json::to_string(
            &GrokError::http_failure(reqwest::StatusCode::TOO_MANY_REQUESTS, "rate limited")
                .with_retry_after_ms(Some(60_000)),
        )
        .unwrap();
        assert!(wire.contains("\"code\":\"rate_limited\""), "wire: {wire}");
        assert!(wire.contains("\"httpStatus\":429"), "wire: {wire}");
        assert!(wire.contains("\"transient\":true"), "wire: {wire}");
        assert!(wire.contains("\"retryAfterMs\":60000"), "wire: {wire}");

        let wire = serde_json::to_string(&credential_missing()).unwrap();
        assert!(wire.contains("\"transient\":false"), "wire: {wire}");
        assert!(!wire.contains("httpStatus"), "wire: {wire}");
        assert!(!wire.contains("retryAfterMs"), "wire: {wire}");
    }

    // ---------- secret handling ----------

    #[test]
    fn debug_and_wire_output_never_contain_the_token() {
        let token = "super-secret-oauth-token-abc";
        let credential = GrokCredential {
            token: SecretToken(token.to_string()),
            account_id: Some("7a2d5abe1234567890".to_string()),
            source: "opencodex",
        };
        let debug = format!("{credential:?}");
        // The Debug rendering must actually happen — an empty rendering
        // would make the leak checks below vacuous.
        assert!(debug.contains("GrokCredential"), "debug: {debug}");
        assert!(!debug.contains(token), "debug leaked the token: {debug}");
        assert!(
            !debug.contains("7a2d5abe1234567890"),
            "debug leaked the raw account id: {debug}"
        );

        let usage = assemble_usage(
            CreditsOutcome::Windows(weekly_windows()),
            Ok(LegacyOutcome::NoWindow),
            Some(GrokAccount {
                id: mask_account_id("7a2d5abe1234567890"),
                source: "opencodex".to_string(),
            }),
        )
        .unwrap();
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(wire.contains("\"usedPercent\""), "wire: {wire}");
        assert!(wire.contains("\"resetAt\""), "wire: {wire}");
        assert!(wire.contains("\"account\""), "wire: {wire}");
        assert!(wire.contains("7a2d5abe…"), "wire: {wire}");
        assert!(!wire.contains(token), "wire leaked the token: {wire}");
        // Only the masked id crosses the wire.
        assert!(
            !wire.contains("7a2d5abe1234567890"),
            "wire leaked the raw account id: {wire}"
        );
    }

    #[test]
    fn account_source_names_the_owning_store() {
        for (source, expected) in [
            ("grok-cli", "grok-cli"),
            ("opencode", "opencode"),
            ("opencodex", "opencodex"),
        ] {
            let usage = GrokUsage {
                limits: vec![],
                account: Some(GrokAccount {
                    id: "abcd1234".to_string(),
                    source: source.to_string(),
                }),
            };
            let wire = serde_json::to_string(&usage).unwrap();
            assert!(wire.contains(expected), "wire: {wire}");
        }
    }

    // ---------- path overrides ----------

    #[test]
    fn store_paths_honor_the_documented_home_overrides() {
        let home = Path::new("/home/user");
        assert_eq!(
            grok_cli_auth_path(Some(" /custom/grok "), home),
            PathBuf::from("/custom/grok/auth.json")
        );
        assert_eq!(
            grok_cli_auth_path(None, home),
            home.join(".grok").join("auth.json")
        );
        assert_eq!(
            grok_cli_auth_path(Some("  "), home),
            home.join(".grok").join("auth.json")
        );
        assert_eq!(
            opencodex_auth_path(Some("/custom/ocx"), home),
            PathBuf::from("/custom/ocx/auth.json")
        );
        assert_eq!(
            opencodex_auth_path(None, home),
            home.join(".opencodex").join("auth.json")
        );
    }

    #[test]
    fn opencode_path_prefers_an_existing_xdg_dir() {
        let home = Path::new("/home/user");
        let custom = std::env::temp_dir().join(format!(
            "rate-limits-grok-xdg-{}",
            std::process::id()
        ));
        fs::create_dir_all(custom.join("opencode")).unwrap();
        assert_eq!(
            opencode_auth_path(custom.to_str(), home),
            custom.join("opencode").join("auth.json")
        );
        // Non-existent XDG dir falls back to the default location.
        assert_eq!(
            opencode_auth_path(Some("/nonexistent-xdg"), home),
            home.join(".local").join("share").join("opencode").join("auth.json")
        );
        fs::remove_dir_all(custom).ok();
    }

    // ---------- identity hardening (cross-account last-good guard) ----------

    /// The failure identity must match the identity the success attribution
    /// carries (`build_grok_attribution` keys on the store-agnostic masked id).
    #[test]
    fn credential_identity_matches_the_success_attribution_input() {
        let credential = GrokCredential {
            token: SecretToken("access".to_string()),
            account_id: Some("7a2d5abe1234567890".to_string()),
            source: "opencodex",
        };
        assert_eq!(
            credential_identity(&credential).as_deref(),
            Some("xai:7a2d5abe…")
        );
        // No account id: no identity anywhere — the runtime keeps its
        // conservative retention behavior.
        let anonymous = GrokCredential {
            token: SecretToken("access".to_string()),
            account_id: None,
            source: "grok-cli",
        };
        assert_eq!(credential_identity(&anonymous), None);
    }

    #[test]
    fn error_identity_hint_stays_off_the_wire() {
        let error = auth_rejected(reqwest::StatusCode::UNAUTHORIZED)
            .with_identity_hint(Some("xai:7a2d5abe…".to_string()));
        // Precondition: the hint must actually be retained on the error
        // before its absence from the wire can mean anything.
        assert_eq!(
            error.identity_hint.as_deref(),
            Some("xai:7a2d5abe…"),
            "identity hint was not retained"
        );
        let wire = serde_json::to_string(&error).unwrap();
        assert!(wire.contains("\"code\":\"auth_failed\""), "wire: {wire}");
        assert!(!wire.contains("identity"), "wire: {wire}");
        assert!(!wire.contains("7a2d5abe"), "wire: {wire}");
    }

    #[test]
    fn expired_credential_error_stamps_the_attempted_identity() {
        // The store holds one expired account: resolution selected it for the
        // attempt, so the failure names it — the runtime can then drop a
        // last-good snapshot that belongs to a different account.
        let grok = temp_file(
            "identity-expired.json",
            &serde_json::json!({
                "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                    "auth_mode": "Oidc",
                    "key": "cli-access",
                    "refresh_token": "cli-refresh",
                    "expires_at": expired_expiry(),
                    "user_id": "7a2d5abe1234567890"
                }
            })
            .to_string(),
        );
        let error = resolve_credential_in(&paths(Some(grok.clone()), None, None), now_utc())
            .expect_err("expired");
        assert_eq!(error.code, "credential_expired");
        assert_eq!(error.identity_hint.as_deref(), Some("xai:7a2d5abe…"));
        fs::remove_file(grok).ok();

        // Nothing could be attempted: no identity anywhere (ambiguous and
        // missing credentials stay unattributed on purpose).
        let error = resolve_credential_in(&paths(None, None, None), now_utc()).unwrap_err();
        assert_eq!(error.code, "credential_missing");
        assert_eq!(error.identity_hint, None);
    }

    // ---------- live gate (discovery §11) ----------

    /// Live verification against the real billing endpoint using the user's
    /// own cached credential: normal resolver, no refresh, no inference, and
    /// at least one parsed window. Requires an unexpired local xAI OAuth
    /// credential and network access; run with
    /// `cargo test --locked grok::tests::live_fetch_returns_windows -- --ignored --nocapture`.
    #[test]
    #[ignore = "live test: requires a valid local xAI OAuth credential and network access"]
    fn live_fetch_returns_windows() {
        let usage = tauri::async_runtime::block_on(fetch_grok_usage()).expect("live fetch");
        let account = usage
            .account
            .as_ref()
            .map(|account| format!("{} ({})", account.id, account.source))
            .unwrap_or_else(|| "<no account>".to_string());
        println!("account: {account}");
        for limit in &usage.limits {
            println!(
                "{}: used {:.1}% reset_at {:?}",
                limit.label,
                limit.used_percent,
                limit.reset_at.as_deref().unwrap_or("-")
            );
        }
        assert!(!usage.limits.is_empty(), "expected at least one parsed window");
    }
}
