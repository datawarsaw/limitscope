//! Google Antigravity / Gemini provider backend.
//!
//! Live-first, cache-fallback. The authoritative quota source is Google's own
//! Cloud Code Assist accounting endpoint — the same source the Antigravity IDE
//! and the reference quota monitors use: the stored account's refresh token is
//! exchanged for an access token (Google OAuth, `refresh_token` grant) and
//! `v1internal:retrieveUserQuotaSummary` is queried for the account's project.
//! The response carries one bucket per family per window (`5h` / `weekly`)
//! whose `remainingFraction` (0–1 remaining) maps to used percent exactly like
//! the cache mapping below. Only when the live fetch cannot produce windows
//! (offline, token rejected, endpoint/schema failure) does the backend fall
//! back to the quota cache written by the `@cortexkit/opencode-antigravity-auth`
//! OpenCode plugin (`~/.config/opencode/antigravity-accounts.json`) — which the
//! plugin stopped refreshing on this machine (last write 2026-09-18), the root
//! cause of the "Reset time passed" mismatch.
//!
//! No writes to any store, and no auth material (refresh tokens, access
//! tokens, session tokens, fingerprints, emails) ever leaves this module:
//! the WebView receives only normalized windows plus the freshness verdict,
//! with provider-derived label text display-sanitized (`display_label`)
//! before it reaches the wire. When the live fetch fails and cached windows
//! are used instead, the
//! result also carries the classified live-failure cause (`fallbackFailure`)
//! — a stable error code and an authored, display-safe message, never
//! provider payload text — so the runtime can explain why cached data was
//! used without changing the fallback's behavior or presentation.
//!
//! Cache shape observed on this machine 2026-09 (secret-bearing fields elided;
//! only the fields we consume are shown):
//!
//! ```text
//! {
//!   "version": 4,
//!   "activeIndex": 0,
//!   "accounts": [{
//!     "email": …, "refreshToken": …, "fingerprint": …, "enabled": true,
//!     "cachedQuota": {
//!       "gemini": {
//!         "remainingFraction": 0.90, "resetTime": "<ISO-8601>",
//!         "modelCount": 2,
//!         "windows": [
//!           { "window": "5h",     "remainingFraction": 0.90, "resetTime": "<ISO-8601>" },
//!           { "window": "weekly", "remainingFraction": 0.95, "resetTime": "<ISO-8601>" }
//!         ]
//!       },
//!       "non-gemini": { … same shape … }
//!     },
//!     "cachedQuotaUpdatedAt": 1789697612735
//!   }]
//! }
//! ```
//!
//! The top-level `remainingFraction`/`resetTime` of a family mirror whichever
//! window currently applies (observed: the 5h window for `gemini`, the weekly
//! window for `non-gemini`), so the `windows[]` entries are the authoritative
//! per-window data and the top-level pair is only used as a fallback when a
//! family carries no usable `windows[]`.
//!
//! Freshness (cache fallback): the account's `cachedQuotaUpdatedAt` (epoch
//! millis) is carried through as `sourceUpdatedAt` (exact RFC-3339,
//! millisecond precision) and classified into `dataFreshness` (`fresh` /
//! `stale`) against a 24-hour threshold. An unusable stamp — missing,
//! malformed, or implausibly ahead of the local clock — is indeterminate and
//! reads as `stale`. A successful live fetch stamps `sourceUpdatedAt` with the
//! fetch time and reads `fresh`. Staleness changes the verdict only — the
//! windows and their reset times are shown regardless, and no reset time is
//! ever re-invented.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

/// Written by the OpenCode Antigravity plugin, XDG-style even on Windows
/// (verified: `C:\Users\<user>\.config\opencode\antigravity-accounts.json`).
const CACHE_FILE: &str = "antigravity-accounts.json";

// ---------- live quota source (Google Cloud Code Assist accounting) ----------
//
// The OAuth client identifiers below are the public ones embedded in the
// Antigravity desktop client — the same constants every third-party
// Antigravity integration ships (they are configuration, not user secrets).
// The refresh token itself comes from the plugin cache and never leaves this
// process; nothing beyond normalized windows is returned to the WebView.

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const QUOTA_URL: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
/// The UA the Antigravity IDE sends; some accounts reject other clients for
/// quota accounting only (mirrored fallback: `antigravity/1.0`).
const QUOTA_USER_AGENT: &str =
    "antigravity/ide/2.5.5 (os_type=windows; arch=x86_64; antigravity-ide; auth_method=oauth)";
const QUOTA_FALLBACK_USER_AGENT: &str = "antigravity/1.0";
const LIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Reuse a cached access token until shortly before its documented expiry so
/// a refresh cycle never re-mints a token per poll.
const TOKEN_REUSE_MARGIN_SECS: i64 = 60;

// ---------- data returned to the WebView (camelCase on the wire) ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityLimitWindow {
    pub label: String,
    pub used_percent: f64,
    /// Provider-reported ISO-8601 timestamp, preserved exactly as cached.
    /// `None` when the cached timestamp is missing or malformed.
    pub reset_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityUsage {
    pub limits: Vec<AntigravityLimitWindow>,
    /// Exact snapshot time of the selected account's cache: the
    /// `cachedQuotaUpdatedAt` epoch-millis value rendered as RFC-3339 UTC
    /// with millisecond precision (a string-form timestamp is preserved
    /// verbatim). `None` when absent or malformed — never guessed.
    pub source_updated_at: Option<String>,
    /// Freshness of the cached snapshot: `stale` once it is strictly older
    /// than `STALE_AFTER_HOURS`, and also when the timestamp is unusable or
    /// implausibly ahead of the local clock.
    pub data_freshness: DataSourceFreshness,
    /// Present only when the live fetch failed and cached windows were used
    /// instead: the structured cause of that fresh live failure. Codes reuse
    /// the shared error vocabulary (`network`, `auth_invalid`,
    /// `unexpected_response`); messages are authored and display-safe —
    /// never a provider payload, credential, token, or filesystem path.
    /// Diagnostic evidence for why cached data was used; the fallback
    /// behavior, health, and presentation semantics are unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_failure: Option<AntigravityError>,
}

/// Generic freshness verdict for providers that surface a cached snapshot
/// instead of a live fetch; future cached providers can reuse the same shape
/// on the wire as `dataFreshness: "fresh" | "stale"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DataSourceFreshness {
    Fresh,
    Stale,
}

/// Structured error surfaced to the frontend as `{ code, message }`.
/// Messages are safe to display: they never embed account credentials,
/// file contents, or email addresses.
#[derive(Debug, Clone, Serialize)]
pub struct AntigravityError {
    pub code: String,
    pub message: String,
}

impl AntigravityError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

fn cache_missing() -> AntigravityError {
    AntigravityError::new(
        "cache_missing",
        "Antigravity account cache not found (antigravity-accounts.json). Open Antigravity or the OpenCode Antigravity plugin once so it caches quota.",
    )
}

fn no_account() -> AntigravityError {
    AntigravityError::new(
        "no_account",
        "No Antigravity account is stored in the local cache. Sign in to Antigravity first.",
    )
}

fn quota_missing() -> AntigravityError {
    AntigravityError::new(
        "quota_missing",
        "The Antigravity cache holds no quota snapshot for the active account yet. Open Antigravity once to populate it.",
    )
}

/// The cache's schema is no longer understood. Same verdict as the live
/// providers' `unexpected_response`, so the failure taxonomy stays uniform
/// across backends (no frontend branch keys on a cache-specific name).
fn schema_changed() -> AntigravityError {
    AntigravityError::new(
        "unexpected_response",
        "Antigravity quota cache format changed; no known model-family quotas were found.",
    )
}

// ---------- normalization ----------

/// Labels use only evidence-backed names: the `gemini` / `non-gemini` family
/// keys are verified in the local cache, and the `non-gemini` family — the
/// third-party models Antigravity also serves — displays as "Claude", the
/// family's headline models. The family key itself is the internal
/// identifier and stays `non-gemini`. Unknown future family keys are passed
/// through (display-sanitized) rather than guessed at.
fn family_label(family: &str) -> &str {
    match family {
        "gemini" => "Gemini",
        "non-gemini" => "Claude",
        other => other,
    }
}

/// Window keys verified in the cache: `5h` and `weekly`. The 5-hour window is
/// the primary quota, so it carries the plain family label; other keys are
/// suffixed onto the family label. Whatever the provider supplied, the
/// composed label passes through `display_label` — the one gate between
/// provider-derived text and the wire, shared by the live and cache paths.
fn window_label(family: &str, window: &str) -> String {
    let composed = match window {
        "5h" => family.to_string(),
        "weekly" => format!("{family} Weekly"),
        other => format!("{family} {other}"),
    };
    display_label(&composed)
}

/// Conservative display cap for a provider-derived label.
const MAX_LABEL_CHARS: usize = 40;

/// Display sanitization for any provider-derived text that ends up in a
/// usage-window label: ANSI escape sequences are stripped whole (so
/// `ESC [ 31 m` disappears instead of leaving a visible `[31m` residue),
/// control characters are removed, whitespace runs — newlines included —
/// collapse to single spaces with the ends trimmed, and the result is
/// capped at `MAX_LABEL_CHARS` characters. Ordinary text is unchanged byte
/// for byte — the authored family labels and every observed provider name
/// pass through identically, so only hostile or malformed input is altered.
/// Applied at the label composition points (`window_label` and the
/// top-level-pair fallback), which covers both the live and the cache path;
/// sanitizing the composed label rather than the pieces is what makes the
/// length cap hold for `family + " Weekly"`.
fn display_label(raw: &str) -> String {
    let visible: String = strip_ansi_escapes(raw)
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    visible
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_LABEL_CHARS)
        .collect()
}

/// Removes ANSI/ECMA-48 escape sequences — CSI (parameterized), OSC
/// (string-terminated), and the short `ESC` + byte forms — deterministically,
/// without regular expressions. An unterminated sequence consumes to the end
/// of the string, so a truncated escape cannot leak its tail into the label.
fn strip_ansi_escapes(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\u{1B}' {
            out.push(character);
            continue;
        }
        match chars.peek() {
            // CSI: `ESC [` then parameters/intermediates up to the final byte
            // in 0x40–0x7E (e.g. `ESC [ 31 m`).
            Some('[') => {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7E}').contains(&next) {
                        break;
                    }
                }
            }
            // OSC: `ESC ]` then a string ended by BEL or ST (`ESC \`).
            Some(']') => {
                chars.next();
                loop {
                    match chars.peek() {
                        Some('\u{07}') => {
                            chars.next();
                            break;
                        }
                        Some('\u{1B}') => {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        Some(_) => {
                            chars.next();
                        }
                        None => break,
                    }
                }
            }
            // nF sequences (e.g. the charset designation `ESC ( B`):
            // intermediate bytes 0x20–0x2F, then one final byte.
            Some(next) if ('\u{20}'..='\u{2F}').contains(next) => {
                while matches!(chars.peek(), Some(next) if ('\u{20}'..='\u{2F}').contains(next)) {
                    chars.next();
                }
                chars.next();
            }
            // Short private sequences (e.g. the cursor save `ESC 7`): one
            // final byte. A trailing bare `ESC` ends the string.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// `remainingFraction` (0–1 remaining) → used percent (0–100), clamped so a
/// glitched fraction can never render a nonsensical percentage.
fn fraction_to_percent(fraction: f64) -> f64 {
    ((1.0 - fraction) * 100.0).clamp(0.0, 100.0)
}

/// Only finite numbers qualify; strings, booleans, nulls and overflowing
/// values (e.g. `1e999`) are treated as absent so a broken family is skipped
/// instead of shown with invented data.
fn usable_fraction(value: Option<&Value>) -> Option<f64> {
    let fraction = value?.as_f64()?;
    if fraction.is_finite() {
        Some(fraction)
    } else {
        None
    }
}

/// The cached reset timestamp is passed through exactly as stored; a value
/// that does not parse as ISO-8601 is dropped (the window renders without a
/// reset line) rather than guessed at.
fn validated_reset(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    match DateTime::parse_from_rfc3339(raw) {
        Ok(_) => Some(raw.to_string()),
        Err(_) => None,
    }
}

// ---------- source freshness ----------

/// A cached snapshot strictly older than this is `stale`. The plugin
/// refreshes the cache on every Antigravity use, so a snapshot older than a
/// day means the account has not been used since and the 5-hour windows'
/// reset times are certainly outdated. Staleness only changes the verdict —
/// windows and their reset times are never hidden, altered, or re-invented.
const STALE_AFTER_HOURS: i64 = 24;

/// The plugin and this app read the same machine's clock, so a stamp may sit
/// slightly ahead of `now` — a write/read race, a small NTP correction — and
/// still describe a real snapshot age. Beyond this tolerance the stamp cannot
/// be a same-machine reading at all: the age it implies is unknowable, and
/// unknown age must never read as fresh.
const MAX_FUTURE_SKEW_MINS: i64 = 5;

fn data_freshness(
    source_updated_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> DataSourceFreshness {
    let Some(updated_at) = source_updated_at else {
        // Missing or malformed timestamp: freshness is indeterminate, which
        // must never read as fresh.
        return DataSourceFreshness::Stale;
    };
    // A modest future-dated stamp (same-machine clock skew) is taken at face
    // value; an implausible one is indeterminate, which must never read as
    // fresh.
    if updated_at - now > chrono::Duration::minutes(MAX_FUTURE_SKEW_MINS) {
        DataSourceFreshness::Stale
    } else if now - updated_at > chrono::Duration::hours(STALE_AFTER_HOURS) {
        DataSourceFreshness::Stale
    } else {
        DataSourceFreshness::Fresh
    }
}

/// The exact `cachedQuotaUpdatedAt` of the selected account, kept verbatim:
/// an epoch-milliseconds number is rendered as RFC-3339 UTC with millisecond
/// precision (no rounding), a string must parse as ISO-8601 and is preserved
/// as stored. Anything else — wrong type, empty, unparseable, or out of
/// range — is `None`, so the snapshot degrades to `stale` instead of
/// inventing a freshness verdict.
fn parse_source_updated_at(value: Option<&Value>) -> Option<(String, DateTime<Utc>)> {
    match value? {
        Value::Number(number) => {
            let at = DateTime::from_timestamp_millis(number.as_i64()?)?;
            Some((at.to_rfc3339_opts(SecondsFormat::Millis, true), at))
        }
        Value::String(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            let at = DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc);
            Some((raw.to_string(), at))
        }
        _ => None,
    }
}

/// Every present window of every present family is emitted — no invented or
/// defaulted limits. Families iterate in alphabetical key order, which for
/// the verified cache yields `gemini` before `non-gemini`.
fn normalize_quota(cached_quota: &Value) -> Vec<AntigravityLimitWindow> {
    let Some(families) = cached_quota.as_object() else {
        return Vec::new();
    };
    let mut limits = Vec::new();
    for (family, quota) in families {
        let family_label = family_label(family);
        let mut emitted = false;
        if let Some(windows) = quota.get("windows").and_then(Value::as_array) {
            for window in windows {
                let Some(window_key) = window.get("window").and_then(Value::as_str) else {
                    continue;
                };
                let Some(fraction) = usable_fraction(window.get("remainingFraction")) else {
                    continue;
                };
                limits.push(AntigravityLimitWindow {
                    label: window_label(family_label, window_key),
                    used_percent: fraction_to_percent(fraction),
                    reset_at: validated_reset(window.get("resetTime")),
                });
                emitted = true;
            }
        }
        // Fallback for caches whose families carry only the top-level pair.
        if !emitted {
            if let Some(fraction) = usable_fraction(quota.get("remainingFraction")) {
                limits.push(AntigravityLimitWindow {
                    label: display_label(family_label),
                    used_percent: fraction_to_percent(fraction),
                    reset_at: validated_reset(quota.get("resetTime")),
                });
            }
        }
    }
    limits
}

// ---------- cache parsing (read-only) ----------

/// Quota windows plus the exact snapshot stamp of the selected account.
#[derive(Debug)]
struct ParsedQuota {
    limits: Vec<AntigravityLimitWindow>,
    source_updated_at: Option<(String, DateTime<Utc>)>,
}

fn parse_quota(raw: &str) -> Result<ParsedQuota, AntigravityError> {
    let root: Value = serde_json::from_str(raw).map_err(|_| {
        AntigravityError::new(
            "cache_invalid",
            "Antigravity account cache (antigravity-accounts.json) is not valid JSON.",
        )
    })?;
    let Some(root) = root.as_object() else {
        return Err(AntigravityError::new(
            "cache_invalid",
            "Antigravity account cache is not a JSON object.",
        ));
    };
    let accounts = root
        .get("accounts")
        .and_then(Value::as_array)
        .filter(|accounts| !accounts.is_empty());
    let Some(accounts) = accounts else {
        return Err(no_account());
    };
    // The active account is the one the plugin last used; an out-of-range
    // index falls back to the first stored account rather than erroring.
    let active_index = root
        .get("activeIndex")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let account = accounts.get(active_index).or_else(|| accounts.first());
    let Some(account) = account else {
        return Err(no_account());
    };
    let Some(cached_quota) = account.get("cachedQuota").filter(|quota| quota.is_object()) else {
        return Err(quota_missing());
    };
    let limits = normalize_quota(cached_quota);
    if limits.is_empty() {
        return Err(schema_changed());
    }
    let source_updated_at = parse_source_updated_at(account.get("cachedQuotaUpdatedAt"));
    Ok(ParsedQuota {
        limits,
        source_updated_at,
    })
}

/// `$XDG_CONFIG_HOME/opencode` when that directory exists, else the verified
/// Windows/Unix location `~/.config/opencode`.
fn cache_path() -> Result<PathBuf, AntigravityError> {
    if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
        let xdg_config_home = xdg_config_home.trim();
        if !xdg_config_home.is_empty() {
            let dir = PathBuf::from(xdg_config_home).join("opencode");
            if dir.exists() {
                return Ok(dir.join(CACHE_FILE));
            }
        }
    }
    let home = std::env::home_dir().ok_or_else(|| {
        AntigravityError::new(
            "home_unresolved",
            "Could not locate the user home directory.",
        )
    })?;
    Ok(home.join(".config").join("opencode").join(CACHE_FILE))
}

/// The cache-fallback construction: the selected account's cached windows
/// plus its exact snapshot stamp and freshness verdict (the v0.5 semantics,
/// unchanged). Split out from the I/O so the live/fallback decision is
/// testable without a network.
fn usage_from_cache(raw: &str) -> Result<AntigravityUsage, AntigravityError> {
    let parsed = parse_quota(raw)?;
    let (source_updated_at, updated_at) = match parsed.source_updated_at {
        Some((exact, at)) => (Some(exact), Some(at)),
        None => (None, None),
    };
    Ok(AntigravityUsage {
        data_freshness: data_freshness(updated_at, Utc::now()),
        source_updated_at,
        limits: parsed.limits,
        fallback_failure: None,
    })
}

/// The live construction: windows fetched from Google's accounting endpoint
/// this instant, stamped with the fetch time. A live snapshot is the source
/// of truth, so it always reads fresh — the 24-hour cache threshold does not
/// apply to data that is not cached.
fn usage_live(windows: Vec<AntigravityLimitWindow>, now: DateTime<Utc>) -> AntigravityUsage {
    AntigravityUsage {
        data_freshness: DataSourceFreshness::Fresh,
        source_updated_at: Some(now.to_rfc3339_opts(SecondsFormat::Millis, true)),
        limits: windows,
        fallback_failure: None,
    }
}

async fn read_antigravity_usage() -> Result<AntigravityUsage, AntigravityError> {
    let path = cache_path()?;
    if !path.exists() {
        return Err(cache_missing());
    }
    let raw = fs::read_to_string(&path).map_err(|error| {
        AntigravityError::new(
            "cache_unreadable",
            format!("Could not read the Antigravity account cache: {error}"),
        )
    })?;
    // Live-first: the plugin cache stopped being refreshed when the plugin
    // stopped running (the writer is OpenCode, not this app), so cached
    // windows are a fallback, not the source. Only a credential-bearing
    // account can go live; every live failure — transport, token, schema —
    // degrades to the cache path with its exact existing semantics, and the
    // classified live-failure cause rides the fallback result as diagnostic
    // evidence instead of being discarded.
    let live = match extract_live_credentials(&raw) {
        Some(credentials) => Some(fetch_live_windows(&credentials).await),
        None => None,
    };
    resolve_usage(&raw, live)
}

/// The live/fallback decision, split out of the I/O so the whole matrix is
/// testable without a network. A live success is the live construction;
/// a live failure over a usable cache returns exactly the cache
/// construction with the fresh live-failure cause attached; a live failure
/// without a usable cache fails with the cache's own error, unchanged.
/// `None` is the no-credentials cache — the cache is the only source,
/// exactly as before.
fn resolve_usage(
    raw: &str,
    live: Option<Result<Vec<AntigravityLimitWindow>, AntigravityError>>,
) -> Result<AntigravityUsage, AntigravityError> {
    match live {
        Some(Ok(windows)) => Ok(usage_live(windows, Utc::now())),
        Some(Err(live_failure)) => {
            let mut usage = usage_from_cache(raw)?;
            usage.fallback_failure = Some(live_failure);
            Ok(usage)
        }
        None => usage_from_cache(raw),
    }
}

// ---------- live source: credentials from the plugin cache ----------

/// The selected account's live-quota credentials: its OAuth refresh token and
/// the Cloud Code Assist project the plugin onboarded it to. The refresh token
/// never leaves this process — `Debug` is redacted so a panic/log path cannot
/// print it.
#[derive(Clone)]
struct LiveCredentials {
    refresh_token: String,
    project_id: String,
}

impl std::fmt::Debug for LiveCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveCredentials")
            .field("refresh_token", &"<redacted>")
            .field("project_id", &self.project_id)
            .finish()
    }
}

/// The same account selection the cache path uses (`activeIndex`, falling
/// back to the first stored account), reduced to the two fields the live
/// source needs. `None` when the stored entry predates the plugin's OAuth
/// fields — such a cache can still serve its cached windows as fallback.
fn extract_live_credentials(raw: &str) -> Option<LiveCredentials> {
    let root: Value = serde_json::from_str(raw).ok()?;
    let accounts = root.get("accounts")?.as_array()?;
    if accounts.is_empty() {
        return None;
    }
    let active_index = root
        .get("activeIndex")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let account = accounts.get(active_index).or_else(|| accounts.first())?;
    let refresh_token = account
        .get("refreshToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())?
        .to_string();
    let project_id = ["projectId", "managedProjectId"]
        .iter()
        .find_map(|key| account.get(*key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|project| !project.is_empty())?
        .to_string();
    Some(LiveCredentials {
        refresh_token,
        project_id,
    })
}

// ---------- live source: OAuth token + quota summary ----------

/// The token endpoint refused the refresh with a non-success status: a 4xx
/// means the stored refresh token was rejected (an auth-type failure); any
/// other status is an unexpected response. The bare status number is the
/// only upstream detail retained — no body text, no headers.
fn token_refresh_error(status: u16) -> AntigravityError {
    if (400..=499).contains(&status) {
        AntigravityError::new(
            "auth_invalid",
            format!("Antigravity token refresh was refused (HTTP {status})."),
        )
    } else {
        AntigravityError::new(
            "unexpected_response",
            format!("Antigravity token endpoint returned HTTP {status}."),
        )
    }
}

/// The quota endpoint's failure classes, kept distinct so the fallback
/// cause classifies honestly: transport problems are `network`, while a
/// refused status and an unusable body are both `unexpected_response`.
enum QuotaFetchFailure {
    Transport,
    HttpStatus(u16),
    MalformedBody,
}

fn quota_fetch_error(failure: QuotaFetchFailure) -> AntigravityError {
    match failure {
        QuotaFetchFailure::Transport => AntigravityError::new(
            "network",
            "Antigravity live quota fetch failed: the quota endpoint could not be reached.",
        ),
        QuotaFetchFailure::HttpStatus(status) => AntigravityError::new(
            "unexpected_response",
            format!("Antigravity live quota fetch was refused (HTTP {status})."),
        ),
        QuotaFetchFailure::MalformedBody => AntigravityError::new(
            "unexpected_response",
            "Antigravity live quota endpoint returned an unusable response body.",
        ),
    }
}

/// The live response parsed but carried no usable quota windows — a schema
/// or payload change. Same code the cache path uses for the same verdict,
/// so the failure taxonomy stays uniform.
fn unusable_quota_response() -> AntigravityError {
    AntigravityError::new(
        "unexpected_response",
        "Antigravity live quota response carried no usable quota windows.",
    )
}

struct CachedAccessToken {
    token: String,
    expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for CachedAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedAccessToken")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

fn access_token_cache() -> &'static Mutex<Option<CachedAccessToken>> {
    static CACHE: OnceLock<Mutex<Option<CachedAccessToken>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn live_http_client() -> Result<&'static reqwest::Client, ()> {
    static CLIENT: OnceLock<Result<reqwest::Client, ()>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(LIVE_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ())
    }) {
        Ok(client) => Ok(client),
        Err(()) => Err(()),
    }
}

/// Exchange the stored refresh token for an access token, reusing the cached
/// one until shortly before its expiry. Failures are classified onto the
/// shared error vocabulary (`network`, `auth_invalid`,
/// `unexpected_response`) so the caller can carry the cause through the
/// cache fallback; the messages are authored and display-safe.
async fn get_access_token(refresh_token: &str) -> Result<String, AntigravityError> {
    {
        let guard = access_token_cache().lock().unwrap();
        if let Some(cached) = guard.as_ref() {
            if cached.expires_at - Utc::now()
                > chrono::Duration::seconds(TOKEN_REUSE_MARGIN_SECS)
            {
                return Ok(cached.token.clone());
            }
        }
    }
    let client = live_http_client().map_err(|_| {
        AntigravityError::new(
            "network",
            "Antigravity live quota fetch could not start.",
        )
    })?;
    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|_| {
            AntigravityError::new(
                "network",
                "Antigravity token refresh failed: the token endpoint could not be reached.",
            )
        })?;
    if !response.status().is_success() {
        return Err(token_refresh_error(response.status().as_u16()));
    }
    let body: Value = response.json().await.map_err(|_| {
        AntigravityError::new(
            "unexpected_response",
            "Antigravity token endpoint returned an unusable response body.",
        )
    })?;
    let token = body
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            AntigravityError::new(
                "unexpected_response",
                "Antigravity token endpoint returned an unusable response body.",
            )
        })?
        .to_string();
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|seconds| *seconds > 0)
        .unwrap_or(3600);
    {
        let mut guard = access_token_cache().lock().unwrap();
        *guard = Some(CachedAccessToken {
            token: token.clone(),
            expires_at: Utc::now() + chrono::Duration::seconds(expires_in),
        });
    }
    Ok(token)
}

/// POST the quota summary for the account's project. `Err` carries the
/// failure class so the caller can mirror the IDE's documented 403 retry
/// and classify the cause for the cache fallback.
async fn fetch_quota_summary(
    access_token: &str,
    project_id: &str,
    user_agent: &str,
) -> Result<Value, QuotaFetchFailure> {
    let client = live_http_client().map_err(|_| QuotaFetchFailure::Transport)?;
    let response = client
        .post(QUOTA_URL)
        .bearer_auth(access_token)
        .header("accept", "application/json")
        .header("user-agent", user_agent)
        .json(&serde_json::json!({ "project": project_id }))
        .send()
        .await
        .map_err(|_| QuotaFetchFailure::Transport)?;
    let status = response.status();
    if !status.is_success() {
        return Err(QuotaFetchFailure::HttpStatus(status.as_u16()));
    }
    response
        .json::<Value>()
        .await
        .map_err(|_| QuotaFetchFailure::MalformedBody)
}

/// One live attempt: token refresh, then the quota summary — retried once
/// with the plain client UA when the IDE fingerprint is rejected for quota
/// accounting only (mirrors the reference monitors' behavior). Failures are
/// classified onto the shared error vocabulary so the cache fallback can
/// carry the cause. Windows are the canonical per-family order; an empty
/// parse is a failure so the caller falls back to the cache instead of
/// reporting a schema change as truth.
async fn fetch_live_windows(
    credentials: &LiveCredentials,
) -> Result<Vec<AntigravityLimitWindow>, AntigravityError> {
    let result = fetch_live_windows_inner(credentials).await;
    // Final-boundary value scrub (`secret_scrub`): the refresh token is alive
    // for this whole fetch and the minted access token is cached for the
    // process lifetime, so any exact occurrence of either in a failure
    // message is removed before the failure reaches the cache fallback (and,
    // through it, `fallback_failure`).
    let cache = access_token_cache().lock().unwrap();
    let mut seeds = vec![credentials.refresh_token.as_str()];
    if let Some(cached) = cache.as_ref() {
        seeds.push(cached.token.as_str());
    }
    result.map_err(|mut error| {
        error.message = crate::secret_scrub::scrub_text(&error.message, &seeds);
        error
    })
}

async fn fetch_live_windows_inner(
    credentials: &LiveCredentials,
) -> Result<Vec<AntigravityLimitWindow>, AntigravityError> {
    let access_token = get_access_token(&credentials.refresh_token).await?;
    let summary = match fetch_quota_summary(
        &access_token,
        &credentials.project_id,
        QUOTA_USER_AGENT,
    )
    .await
    {
        Ok(summary) => summary,
        Err(QuotaFetchFailure::HttpStatus(403)) => {
            fetch_quota_summary(
                &access_token,
                &credentials.project_id,
                QUOTA_FALLBACK_USER_AGENT,
            )
            .await
            .map_err(quota_fetch_error)?
        }
        Err(failure) => return Err(quota_fetch_error(failure)),
    };
    let windows = parse_quota_summary(&summary);
    if windows.is_empty() {
        return Err(unusable_quota_response());
    }
    Ok(windows)
}

// ---------- live source: response normalization ----------

/// Live family classification over `groups[]`, mirroring the cache-path
/// family semantics. The Gemini group is the `gemini` family; the
/// third-party group — Google's own description names Claude and GPT models —
/// is the `non-gemini` family the cache uses for the same bucket. Unknown
/// groups fall back to their display name verbatim rather than guessed at.
fn live_family_key(group: &Value) -> String {
    let group = group.as_object();
    let display = group
        .and_then(|group| group.get("displayName"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let description = group
        .and_then(|group| group.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let haystack = format!("{display} {description}").to_lowercase();
    if haystack.contains("gemini") {
        "gemini".to_string()
    } else if haystack.contains("claude") || haystack.contains("3p") || haystack.contains("gpt") {
        "non-gemini".to_string()
    } else if display.is_empty() {
        "Other".to_string()
    } else {
        display
    }
}

/// Live window classification over a bucket, from every naming field Google
/// sends (`window`, `bucketId`, `displayName`) — the same haystack the
/// reference monitors use. `None` when the bucket carries no window naming at
/// all, in which case the bucket is skipped rather than labeled by guess.
fn live_window_key(bucket: &Value) -> Option<String> {
    let bucket = bucket.as_object()?;
    let raw_window = bucket
        .get("window")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|window| !window.is_empty())?;
    let bucket_id = bucket
        .get("bucketId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let display = bucket
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let haystack = format!("{raw_window} {bucket_id} {display}").to_lowercase();
    if haystack.contains("week") {
        Some("weekly".to_string())
    } else if haystack.contains("5h") || haystack.contains("five") {
        Some("5h".to_string())
    } else {
        Some(raw_window.to_string())
    }
}

/// Canonical emission order so live data renders like the cache data:
/// families Gemini, then Claude (the `non-gemini` family), then anything
/// else; within a family
/// the 5-hour window, then weekly, then others. The live response orders
/// buckets per its own whim (observed: weekly first).
fn live_family_rank(family_key: &str) -> usize {
    match family_key {
        "gemini" => 0,
        "non-gemini" => 1,
        _ => 2,
    }
}

fn live_window_rank(window_key: &str) -> usize {
    match window_key {
        "5h" => 0,
        "weekly" => 1,
        _ => 2,
    }
}

/// Parse `v1internal:retrieveUserQuotaSummary`: `groups[].buckets[]`, each
/// bucket carrying `remainingFraction` (0–1 remaining) and `resetTime`
/// (ISO-8601). Same conversion contract as the cache path —
/// `(1 - fraction) * 100`, clamped; unusable fractions skip the bucket;
/// unparsable resets drop the reset line, never the window. Labels dedupe
/// first-wins per canonical rank.
fn parse_quota_summary(body: &Value) -> Vec<AntigravityLimitWindow> {
    let Some(groups) = body.get("groups").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut windows: Vec<(String, String, AntigravityLimitWindow)> = Vec::new();
    for group in groups {
        let family_key = live_family_key(group);
        let Some(buckets) = group.get("buckets").and_then(Value::as_array) else {
            continue;
        };
        for bucket in buckets {
            let Some(window_key) = live_window_key(bucket) else {
                continue;
            };
            let Some(fraction) = usable_fraction(bucket.get("remainingFraction")) else {
                continue;
            };
            windows.push((
                family_key.clone(),
                window_key.clone(),
                AntigravityLimitWindow {
                    label: window_label(family_label(&family_key), &window_key),
                    used_percent: fraction_to_percent(fraction),
                    reset_at: validated_reset(bucket.get("resetTime")),
                },
            ));
        }
    }
    // Canonical order, then first-wins dedupe per label.
    windows.sort_by(|(family_a, window_a, _), (family_b, window_b, _)| {
        (live_family_rank(family_a), live_window_rank(window_a))
            .cmp(&(live_family_rank(family_b), live_window_rank(window_b)))
    });
    let mut seen = std::collections::HashSet::new();
    windows
        .into_iter()
        .filter(|(_, _, window)| seen.insert(window.label.clone()))
        .map(|(_, _, window)| window)
        .collect()
}

// ---------- command ----------

#[tauri::command]
pub async fn get_antigravity_usage() -> Result<AntigravityUsage, AntigravityError> {
    read_antigravity_usage().await
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[tokio::test]
    async fn redirect_is_returned_without_reposting_refresh_token() {
        let destination = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        destination.set_nonblocking(true).unwrap();
        let destination_addr = destination.local_addr().unwrap();
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let source_addr = source.local_addr().unwrap();
        let destination_thread = thread::spawn(move || destination.accept());
        let source_thread = thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut request = [0; 8192];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    format!("HTTP/1.1 302 Found\r\nLocation: http://{destination_addr}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
                )
                .unwrap();
            String::from_utf8_lossy(&request).to_ascii_lowercase()
        });
        let client = live_http_client().unwrap();
        let response = client
            .post(format!("http://{source_addr}/token"))
            .form(&[("refresh_token", "unit-test-refresh-token")])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        assert!(source_thread
            .join()
            .unwrap()
            .contains("unit-test-refresh-token"));
        assert!(
            destination_thread.join().unwrap().unwrap_err().kind()
                == std::io::ErrorKind::WouldBlock
        );
    }

    // Shape mirroring the real local cache (values are synthetic but the
    // field names, nesting and window keys are exactly as observed 2026-09).
    fn realistic_cache() -> String {
        serde_json::json!({
            "version": 4,
            "accounts": [{
                "email": "user@example.com",
                "label": "Test User",
                "refreshToken": "1//super-secret-refresh-token",
                "projectId": "aicode-consumers",
                "enabled": true,
                "fingerprint": {
                    "deviceId": "00000000-0000-0000-0000-000000000000",
                    "sessionToken": "super-secret-session-token"
                },
                "cachedQuota": {
                    "gemini": {
                        "remainingFraction": 0.9001314,
                        "resetTime": "2026-09-18T03:10:11Z",
                        "modelCount": 2,
                        "windows": [
                            { "window": "5h", "remainingFraction": 0.9001314, "resetTime": "2026-09-18T03:10:11Z" },
                            { "window": "weekly", "remainingFraction": 0.9537665, "resetTime": "2026-09-23T21:48:16Z" }
                        ]
                    },
                    "non-gemini": {
                        "remainingFraction": 0.28326866,
                        "resetTime": "2026-09-20T11:48:41Z",
                        "modelCount": 3,
                        "windows": [
                            { "window": "5h", "remainingFraction": 0.8593228, "resetTime": "2026-09-18T07:04:09Z" },
                            { "window": "weekly", "remainingFraction": 0.28326866, "resetTime": "2026-09-20T11:48:41Z" }
                        ]
                    }
                },
                "cachedQuotaUpdatedAt": 1789697612735u64
            }],
            "activeIndex": 0
        })
        .to_string()
    }

    #[test]
    fn normal_cache_yields_all_family_windows() {
        let limits = parse_quota(&realistic_cache()).unwrap().limits;
        assert_eq!(
            limits
                .iter()
                .map(|limit| limit.label.as_str())
                .collect::<Vec<_>>(),
            ["Gemini", "Gemini Weekly", "Claude", "Claude Weekly"]
        );
    }

    #[test]
    fn fractions_convert_to_used_percent() {
        let limits = parse_quota(&realistic_cache()).unwrap().limits;
        // (1 - 0.9001314) * 100, compared via the same expression so the
        // assertion is exact under f64 rounding.
        assert_eq!(limits[0].used_percent, (1.0 - 0.9001314) * 100.0);
        assert!((limits[0].used_percent - 9.99).abs() < 0.01);
        assert_eq!(limits[1].used_percent, (1.0 - 0.9537665) * 100.0);
        // The non-gemini 5h and weekly windows differ and stay separate.
        assert_eq!(limits[2].used_percent, (1.0 - 0.8593228) * 100.0);
        assert_eq!(limits[3].used_percent, (1.0 - 0.28326866) * 100.0);
        assert!((limits[3].used_percent - 71.67).abs() < 0.01);
    }

    #[test]
    fn reset_timestamps_are_preserved_exactly() {
        let limits = parse_quota(&realistic_cache()).unwrap().limits;
        assert_eq!(
            limits[0].reset_at.as_deref(),
            Some("2026-09-18T03:10:11Z")
        );
        assert_eq!(
            limits[1].reset_at.as_deref(),
            Some("2026-09-23T21:48:16Z")
        );
        assert_eq!(
            limits[2].reset_at.as_deref(),
            Some("2026-09-18T07:04:09Z")
        );
        assert_eq!(
            limits[3].reset_at.as_deref(),
            Some("2026-09-20T11:48:41Z")
        );
    }

    #[test]
    fn single_family_cache_yields_only_that_family() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "non-gemini": {
                    "windows": [
                        { "window": "5h", "remainingFraction": 0.5, "resetTime": "2026-09-30T00:00:00Z" }
                    ]
                }
            }}],
            "activeIndex": 0
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Claude");
        assert_eq!(limits[0].used_percent, 50.0);
    }

    #[test]
    fn unknown_family_keys_pass_through_verbatim() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini-ultra": {
                    "windows": [{ "window": "daily", "remainingFraction": 1.0 }]
                }
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "gemini-ultra daily");
        assert_eq!(limits[0].used_percent, 0.0);
        assert_eq!(limits[0].reset_at, None);
    }

    #[test]
    fn top_level_fraction_used_when_windows_absent() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": { "remainingFraction": 0.25, "resetTime": "2026-09-30T00:00:00Z" }
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Gemini");
        assert_eq!(limits[0].used_percent, 75.0);
        assert_eq!(limits[0].reset_at.as_deref(), Some("2026-09-30T00:00:00Z"));
    }

    #[test]
    fn empty_windows_array_falls_back_to_top_level_pair() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": {
                    "remainingFraction": 0.4,
                    "resetTime": "2026-09-30T00:00:00Z",
                    "windows": []
                }
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Gemini");
        assert_eq!(limits[0].used_percent, 60.0);
    }

    #[test]
    fn missing_quota_reports_quota_missing() {
        let no_quota = serde_json::json!({ "accounts": [{ "email": "user@example.com" }] })
            .to_string();
        assert_eq!(parse_quota(&no_quota).unwrap_err().code, "quota_missing");

        let null_quota =
            serde_json::json!({ "accounts": [{ "cachedQuota": null }] }).to_string();
        assert_eq!(parse_quota(&null_quota).unwrap_err().code, "quota_missing");

        let not_object =
            serde_json::json!({ "accounts": [{ "cachedQuota": "0.5" }] }).to_string();
        assert_eq!(parse_quota(&not_object).unwrap_err().code, "quota_missing");
    }

    #[test]
    fn malformed_quota_entries_are_skipped_or_fall_back() {
        // Window without a usable fraction: skipped.
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": { "windows": [
                    { "window": "5h" },
                    { "window": "weekly", "remainingFraction": "0.5" },
                    { "window": "weekly", "remainingFraction": true },
                    { "window": "weekly", "remainingFraction": null }
                ]}
            }}]
        })
        .to_string();
        assert_eq!(parse_quota(&raw).unwrap_err().code, "unexpected_response");

        // A malformed window next to a valid one: only the valid one survives.
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": { "windows": [
                    { "window": "5h" },
                    { "window": "weekly", "remainingFraction": 0.75, "resetTime": "2026-09-30T00:00:00Z" }
                ]}
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Gemini Weekly");
        assert_eq!(limits[0].used_percent, 25.0);

        // Malformed windows but a usable top-level pair: the fallback kicks in.
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": {
                    "remainingFraction": 0.1,
                    "resetTime": "2026-09-30T00:00:00Z",
                    "windows": [{ "remainingFraction": 0.5 }]
                }
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Gemini");
        assert_eq!(limits[0].used_percent, 90.0);

        // Family object with none of the known fields: nothing emitted.
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": { "gemini": {} } }]
        })
        .to_string();
        assert_eq!(parse_quota(&raw).unwrap_err().code, "unexpected_response");
    }

    #[test]
    fn fraction_below_zero_clamps_to_full_usage() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": { "windows": [{ "window": "5h", "remainingFraction": -0.1 }] }
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits[0].used_percent, 100.0);
    }

    #[test]
    fn fraction_above_one_clamps_to_zero_usage() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": { "windows": [{ "window": "5h", "remainingFraction": 1.5 }] }
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits[0].used_percent, 0.0);
    }

    #[test]
    fn overflowing_fraction_literals_reject_the_file() {
        // serde_json refuses out-of-range float literals (1e999) outright, so
        // a glitched file of this kind fails as unparseable JSON.
        let raw = r#"{"accounts":[{"cachedQuota":{"gemini":{
            "remainingFraction": 1e999,
            "windows":[{"window":"5h","remainingFraction":1e999}]
        }}}]}"#;
        assert_eq!(parse_quota(raw).unwrap_err().code, "cache_invalid");
    }

    #[test]
    fn missing_or_malformed_reset_time_keeps_the_window() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": { "windows": [
                    { "window": "5h", "remainingFraction": 0.5 },
                    { "window": "weekly", "remainingFraction": 0.5, "resetTime": "" },
                    { "window": "monthly", "remainingFraction": 0.5, "resetTime": "soon-ish" },
                    { "window": "daily", "remainingFraction": 0.5, "resetTime": 12345 }
                ]}
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits.len(), 4);
        assert!(limits.iter().all(|limit| limit.reset_at.is_none()));
        assert!(limits.iter().all(|limit| limit.used_percent == 50.0));
    }

    #[test]
    fn offsets_and_fractional_seconds_survive_validation() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": { "windows": [
                    { "window": "5h", "remainingFraction": 0.5,
                      "resetTime": "2026-09-30T05:30:00.123+02:00" }
                ]}
            }}]
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(
            limits[0].reset_at.as_deref(),
            Some("2026-09-30T05:30:00.123+02:00")
        );
    }

    #[test]
    fn malformed_json_is_rejected() {
        assert_eq!(parse_quota("not json").unwrap_err().code, "cache_invalid");
        assert_eq!(parse_quota("[1, 2, 3]").unwrap_err().code, "cache_invalid");
    }

    #[test]
    fn missing_or_empty_accounts_are_rejected() {
        assert_eq!(parse_quota("{}").unwrap_err().code, "no_account");
        assert_eq!(
            parse_quota(r#"{"accounts": []}"#).unwrap_err().code,
            "no_account"
        );
        assert_eq!(
            parse_quota(r#"{"accounts": {}}"#).unwrap_err().code,
            "no_account"
        );
    }

    #[test]
    fn out_of_range_active_index_falls_back_to_first_account() {
        let raw = serde_json::json!({
            "accounts": [
                { "cachedQuota": { "gemini": { "windows": [
                    { "window": "5h", "remainingFraction": 0.1 }
                ]}}}
            ],
            "activeIndex": 7
        })
        .to_string();
        let limits = parse_quota(&raw).unwrap().limits;
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Gemini");
        assert_eq!(limits[0].used_percent, 90.0);
    }

    #[test]
    fn wire_format_is_camel_case_and_never_contains_credentials() {
        let raw = realistic_cache();
        let parsed = parse_quota(&raw).unwrap();
        let usage = AntigravityUsage {
            limits: parsed.limits,
            source_updated_at: parsed.source_updated_at.map(|(exact, _)| exact),
            data_freshness: DataSourceFreshness::Stale,
            fallback_failure: None,
        };
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(wire.contains("usedPercent"), "wire format must be camelCase: {wire}");
        assert!(wire.contains("resetAt"), "wire format must be camelCase: {wire}");
        assert!(wire.contains("sourceUpdatedAt"), "wire format must be camelCase: {wire}");
        assert!(wire.contains("dataFreshness"), "wire format must be camelCase: {wire}");
        assert!(wire.contains(r#""dataFreshness":"stale""#), "verdict must be lowercase: {wire}");
        assert!(!wire.contains("used_percent"));
        assert!(!wire.contains("source_updated_at"));
        // Credentials and account identity must never travel toward the WebView.
        assert!(!wire.contains("1//super-secret-refresh-token"));
        assert!(!wire.contains("super-secret-session-token"));
        assert!(!wire.contains("user@example.com"));
        assert!(!wire.contains("fingerprint"));
        assert!(!wire.contains("cachedQuotaUpdatedAt"));
    }

    // Cache document with the given `cachedQuotaUpdatedAt` and one valid window.
    fn cache_with_stamp(stamp: serde_json::Value) -> String {
        serde_json::json!({
            "accounts": [{
                "cachedQuota": {
                    "gemini": { "windows": [{ "window": "5h", "remainingFraction": 0.5 }] }
                },
                "cachedQuotaUpdatedAt": stamp
            }],
            "activeIndex": 0
        })
        .to_string()
    }

    #[test]
    fn source_updated_at_is_the_exact_cached_millisecond_timestamp() {
        let parsed = parse_quota(&realistic_cache()).unwrap();
        let (exact, at) = parsed.source_updated_at.expect("stamp should parse");
        // 1789697612735 epoch millis, rendered to the exact millisecond.
        assert_eq!(exact, "2026-09-18T02:13:32.735Z");
        assert_eq!(at.timestamp_millis(), 1789697612735);
    }

    #[test]
    fn string_cached_quota_updated_at_is_preserved_verbatim() {
        let parsed = parse_quota(&cache_with_stamp(
            serde_json::json!("2026-09-20T11:48:41+02:00"),
        ))
        .unwrap();
        let (exact, at) = parsed.source_updated_at.expect("stamp should parse");
        assert_eq!(exact, "2026-09-20T11:48:41+02:00");
        assert_eq!(at.timestamp_millis(), 1789897721000);
        // Fresh relative to a now that is two minutes after the snapshot.
        assert_eq!(
            data_freshness(
                Some(at),
                DateTime::parse_from_rfc3339("2026-09-20T11:50:41+02:00")
                    .unwrap()
                    .with_timezone(&Utc)
            ),
            DataSourceFreshness::Fresh
        );
    }

    #[test]
    fn malformed_cached_quota_updated_at_degrades_to_unknown_staleness() {
        for stamp in [
            serde_json::json!(true),
            serde_json::json!({ "at": 1 }),
            serde_json::json!([1789697612735u64]),
            serde_json::json!(null),
            serde_json::json!(""),
            serde_json::json!("   "),
            serde_json::json!("not-a-timestamp"),
            // A number stored as text is not an ISO-8601 timestamp either.
            serde_json::json!("1789697612735"),
            // Out of range for epoch milliseconds.
            serde_json::json!(1e30),
        ] {
            let parsed = parse_quota(&cache_with_stamp(stamp.clone())).unwrap();
            assert!(
                parsed.source_updated_at.is_none(),
                "stamp should degrade to None: {stamp}"
            );
            // Indeterminate freshness must read as stale, never fresh.
            assert_eq!(
                data_freshness(
                    parsed.source_updated_at.map(|(_, at)| at),
                    Utc::now()
                ),
                DataSourceFreshness::Stale
            );
        }
    }

    #[test]
    fn missing_cached_quota_updated_at_leaves_windows_visible_but_stale() {
        let raw = serde_json::json!({
            "accounts": [{ "cachedQuota": {
                "gemini": { "windows": [
                    { "window": "5h", "remainingFraction": 0.5, "resetTime": "2026-09-30T00:00:00Z" }
                ]}
            }}],
            "activeIndex": 0
        })
        .to_string();
        let parsed = parse_quota(&raw).unwrap();
        assert!(parsed.source_updated_at.is_none());
        // Quota values are not invalidated or hidden by the missing stamp.
        assert_eq!(parsed.limits.len(), 1);
        assert_eq!(parsed.limits[0].used_percent, 50.0);
        assert_eq!(
            parsed.limits[0].reset_at.as_deref(),
            Some("2026-09-30T00:00:00Z")
        );
    }

    #[test]
    fn freshness_threshold_is_twenty_four_hours() {
        let now = Utc::now();
        let hour = chrono::Duration::hours(1);
        assert_eq!(
            data_freshness(Some(now - hour), now),
            DataSourceFreshness::Fresh
        );
        // Exactly at the threshold is still fresh; only older is stale.
        assert_eq!(
            data_freshness(Some(now - chrono::Duration::hours(STALE_AFTER_HOURS)), now),
            DataSourceFreshness::Fresh
        );
        assert_eq!(
            data_freshness(
                Some(now - chrono::Duration::hours(STALE_AFTER_HOURS) - chrono::Duration::milliseconds(1)),
                now
            ),
            DataSourceFreshness::Stale
        );
        // A modest future-dated stamp (same-machine clock skew) is taken at
        // face value, through the documented tolerance.
        assert_eq!(
            data_freshness(Some(now + chrono::Duration::minutes(1)), now),
            DataSourceFreshness::Fresh
        );
        assert_eq!(
            data_freshness(
                Some(now + chrono::Duration::minutes(MAX_FUTURE_SKEW_MINS)),
                now
            ),
            DataSourceFreshness::Fresh
        );
        assert_eq!(
            data_freshness(
                Some(
                    now + chrono::Duration::minutes(MAX_FUTURE_SKEW_MINS)
                        + chrono::Duration::milliseconds(1)
                ),
                now
            ),
            DataSourceFreshness::Stale
        );
        // An implausible future stamp cannot date a same-machine snapshot, so
        // its age is indeterminate and must never read as fresh.
        assert_eq!(
            data_freshness(Some(now + hour), now),
            DataSourceFreshness::Stale
        );
        assert_eq!(
            data_freshness(Some(now + chrono::Duration::days(365)), now),
            DataSourceFreshness::Stale
        );
        // Indeterminate is conservative.
        assert_eq!(data_freshness(None, now), DataSourceFreshness::Stale);
    }

    #[test]
    fn implausible_future_stamp_keeps_windows_but_reads_stale() {
        // A corrupt or clock-skewed stamp cannot make a snapshot current; the
        // windows and their reset times stay visible, the verdict degrades.
        let ahead = Utc::now() + chrono::Duration::days(30);
        let parsed = parse_quota(&cache_with_stamp(serde_json::json!(
            ahead.timestamp_millis()
        )))
        .unwrap();
        let (_, at) = parsed.source_updated_at.expect("stamp should parse");
        assert_eq!(
            data_freshness(Some(at), Utc::now()),
            DataSourceFreshness::Stale
        );
        assert_eq!(parsed.limits.len(), 1);
        assert_eq!(parsed.limits[0].used_percent, 50.0);
    }

    #[test]
    fn freshness_verdicts_serialize_lowercase() {
        assert_eq!(
            serde_json::to_string(&DataSourceFreshness::Fresh).unwrap(),
            r#""fresh""#
        );
        assert_eq!(
            serde_json::to_string(&DataSourceFreshness::Stale).unwrap(),
            r#""stale""#
        );
    }

    #[test]
    fn realistic_snapshot_is_stale_relative_to_now_and_windows_survive() {
        // The synthetic snapshot is dated 2026-09-18; any wall clock after
        // 2026-09-19 must see it as stale, while all four windows stay.
        let parsed = parse_quota(&realistic_cache()).unwrap();
        let (_, at) = parsed.source_updated_at.expect("stamp should parse");
        assert_eq!(data_freshness(Some(at), Utc::now()), DataSourceFreshness::Stale);
        assert_eq!(parsed.limits.len(), 4);
    }

    #[test]
    fn freshness_comes_from_the_selected_account() {
        // Out-of-range activeIndex falls back to the first account, so the
        // stamp must come from that same account.
        let raw = serde_json::json!({
            "accounts": [
                { "cachedQuota": { "gemini": { "windows": [
                    { "window": "5h", "remainingFraction": 0.1 }
                ]}}, "cachedQuotaUpdatedAt": 1789697612735u64 },
                { "cachedQuota": { "gemini": { "windows": [
                    { "window": "5h", "remainingFraction": 0.2 }
                ]}}, "cachedQuotaUpdatedAt": 1000u64 }
            ],
            "activeIndex": 7
        })
        .to_string();
        let parsed = parse_quota(&raw).unwrap();
        let (exact, _) = parsed.source_updated_at.expect("stamp should parse");
        assert_eq!(exact, "2026-09-18T02:13:32.735Z");
    }

    #[test]
    fn live_cache_parses_if_present() {
        // Passive check against the real local file when it exists; skipped
        // silently otherwise so the suite stays green on clean machines.
        let Ok(path) = cache_path() else { return };
        let Ok(raw) = fs::read_to_string(&path) else { return };
        let parsed = parse_quota(&raw).expect("real cache should parse");
        assert!(!parsed.limits.is_empty());
        let (exact, at) = parsed
            .source_updated_at
            .expect("real cache carries cachedQuotaUpdatedAt");
        let freshness = data_freshness(Some(at), Utc::now());
        println!(
            "source: {exact} freshness: {freshness:?} (age {:.1} h)",
            (Utc::now() - at).num_minutes() as f64 / 60.0
        );
        for limit in &parsed.limits {
            assert!(
                (0.0..=100.0).contains(&limit.used_percent),
                "percent out of range for {}: {}",
                limit.label,
                limit.used_percent
            );
            if let Some(reset_at) = &limit.reset_at {
                assert!(
                    DateTime::parse_from_rfc3339(reset_at).is_ok(),
                    "reset must be ISO-8601: {reset_at}"
                );
            }
            println!(
                "{}: used {:.2}% reset {}",
                limit.label,
                limit.used_percent,
                limit.reset_at.as_deref().unwrap_or("-")
            );
        }
    }

    // ---------- live quota summary parsing ----------
    //
    // Shape captured live from v1internal:retrieveUserQuotaSummary on
    // 2026-09-29 (values verbatim; the response lists each family's weekly
    // bucket BEFORE its 5h bucket — canonical order is normalized here).

    fn live_summary_body() -> Value {
        serde_json::json!({
            "description": "Quota summary",
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "description": "Models within this group: Gemini Flash, Gemini Pro",
                    "buckets": [
                        { "bucketId": "gemini-weekly", "displayName": "Weekly Limit Remaining",
                          "remainingFraction": 0.89034826, "resetTime": "2026-10-04T21:32:12Z", "window": "weekly" },
                        { "bucketId": "gemini-5h", "displayName": "Five Hour Limit Remaining",
                          "remainingFraction": 0.991144, "resetTime": "2026-09-30T03:49:11Z", "window": "5h" }
                    ]
                },
                {
                    "displayName": "Claude and GPT models",
                    "description": "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS",
                    "buckets": [
                        { "bucketId": "3p-weekly", "displayName": "Weekly Limit Remaining",
                          "remainingFraction": 1, "resetTime": "2026-10-06T22:52:14Z", "window": "weekly" },
                        { "bucketId": "3p-5h", "displayName": "Five Hour Limit Remaining",
                          "remainingFraction": 1, "resetTime": "2026-09-30T03:52:14Z", "window": "5h" }
                    ]
                }
            ]
        })
    }

    #[test]
    fn live_summary_parses_all_four_family_windows_in_canonical_order() {
        let limits = parse_quota_summary(&live_summary_body());
        assert_eq!(
            limits
                .iter()
                .map(|limit| limit.label.as_str())
                .collect::<Vec<_>>(),
            // The live body lists weekly first; emission is canonical:
            // family primary (5h), then weekly.
            ["Gemini", "Gemini Weekly", "Claude", "Claude Weekly"]
        );
    }

    #[test]
    fn live_remaining_fraction_converts_to_used_percent() {
        let limits = parse_quota_summary(&live_summary_body());
        // (1 - 0.991144) * 100 and (1 - 0.89034826) * 100, compared via the
        // same expression so the assertions are exact under f64 rounding.
        assert_eq!(limits[0].used_percent, (1.0 - 0.991144) * 100.0);
        assert!((limits[0].used_percent - 0.89).abs() < 0.01);
        assert_eq!(limits[1].used_percent, (1.0 - 0.89034826) * 100.0);
        assert!((limits[1].used_percent - 10.97).abs() < 0.01);
        // A full remaining fraction is 0% used — never 100%: the field is
        // REMAINING, the conversion is explicit, and the direction mismatch
        // behind the stale-cache report cannot recur here.
        assert_eq!(limits[2].used_percent, 0.0);
        assert_eq!(limits[3].used_percent, 0.0);
    }

    #[test]
    fn live_reset_times_are_preserved_and_future_resets_stay_future() {
        let limits = parse_quota_summary(&live_summary_body());
        assert_eq!(
            limits[0].reset_at.as_deref(),
            Some("2026-09-30T03:49:11Z")
        );
        assert_eq!(
            limits[1].reset_at.as_deref(),
            Some("2026-10-04T21:32:12Z")
        );
        // Every captured reset parses and sits after the capture instant
        // (2026-09-29), so the UI counts down instead of reporting
        // "Reset time passed".
        for limit in &limits {
            let reset = DateTime::parse_from_rfc3339(limit.reset_at.as_deref().unwrap())
                .unwrap();
            assert!(reset > DateTime::parse_from_rfc3339("2026-09-29T00:00:00Z").unwrap());
        }
    }

    #[test]
    fn live_third_party_group_maps_to_the_non_gemini_family() {
        // The bucket the cache calls `non-gemini` arrives live as the
        // "Claude and GPT models" group (GPT-OSS included). Same bucket,
        // same family semantics — pinned so the mapping cannot drift.
        let limits = parse_quota_summary(&live_summary_body());
        assert_eq!(limits[2].label, "Claude");
        assert_eq!(limits[3].label, "Claude Weekly");
    }

    #[test]
    fn live_unknown_group_passes_display_name_through() {
        let body = serde_json::json!({
            "groups": [{
                "displayName": "Grok Models",
                "description": "Models within this group: Grok",
                "buckets": [
                    { "window": "5h", "remainingFraction": 0.5, "resetTime": "2026-10-01T00:00:00Z" }
                ]
            }]
        });
        let limits = parse_quota_summary(&body);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Grok Models");
        assert_eq!(limits[0].used_percent, 50.0);
    }

    #[test]
    fn live_unusable_buckets_are_skipped_or_drop_their_reset_line() {
        let body = serde_json::json!({
            "groups": [{
                "displayName": "Gemini Models",
                "buckets": [
                    { "window": "weekly", "resetTime": "2026-10-01T00:00:00Z" },
                    { "window": "weekly", "remainingFraction": "0.5" },
                    { "window": "weekly", "remainingFraction": null },
                    { "window": "weekly", "remainingFraction": 0.75 },
                    { "window": "5h", "remainingFraction": 0.5, "resetTime": "soon-ish" }
                ]
            }]
        });
        let limits = parse_quota_summary(&body);
        // Only the two usable buckets survive; the unparsable reset drops
        // the reset line, never the window.
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].label, "Gemini");
        assert_eq!(limits[0].used_percent, 50.0);
        assert_eq!(limits[0].reset_at, None);
        assert_eq!(limits[1].label, "Gemini Weekly");
        assert_eq!(limits[1].used_percent, 25.0);
    }

    #[test]
    fn live_duplicate_labels_keep_the_first_canonical_window() {
        let body = serde_json::json!({
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [
                        { "window": "5h", "remainingFraction": 0.9, "resetTime": "2026-10-01T00:00:00Z" },
                        { "window": "5h", "remainingFraction": 0.1, "resetTime": "2026-10-02T00:00:00Z" }
                    ]
                },
                {
                    "displayName": "Gemini Pro Models",
                    "buckets": [
                        { "window": "5h", "remainingFraction": 0.5, "resetTime": "2026-10-03T00:00:00Z" }
                    ]
                }
            ]
        });
        let limits = parse_quota_summary(&body);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].used_percent, (1.0 - 0.9) * 100.0);
    }

    #[test]
    fn live_body_without_groups_or_buckets_is_empty() {
        assert!(parse_quota_summary(&serde_json::json!({})).is_empty());
        assert!(parse_quota_summary(&serde_json::json!({ "groups": [] })).is_empty());
        assert!(parse_quota_summary(&serde_json::json!({ "groups": [{}] })).is_empty());
        assert!(parse_quota_summary(&Value::String("nope".into())).is_empty());
    }

    // ---------- provider-derived label sanitization ----------

    #[test]
    fn accepted_labels_are_byte_for_byte_unchanged() {
        for label in ["Gemini", "Gemini Weekly", "Claude", "Claude Weekly"] {
            assert_eq!(display_label(label), label);
        }
        // The composed windows for the authored families are unchanged too.
        assert_eq!(window_label("Gemini", "5h"), "Gemini");
        assert_eq!(window_label("Gemini", "weekly"), "Gemini Weekly");
        assert_eq!(window_label("Claude", "5h"), "Claude");
        assert_eq!(window_label("Claude", "weekly"), "Claude Weekly");
    }

    #[test]
    fn plain_unknown_labels_survive_semantically_unchanged() {
        assert_eq!(display_label("Grok Models"), "Grok Models");
        assert_eq!(display_label("gemini-ultra daily"), "gemini-ultra daily");
        assert_eq!(window_label("Grok Models", "daily"), "Grok Models daily");
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(display_label("  Grok Models  "), "Grok Models");
        assert_eq!(display_label("\tGrok Models\n"), "Grok Models");
        assert_eq!(display_label(" \r\n Grok Models "), "Grok Models");
    }

    #[test]
    fn internal_whitespace_and_newlines_collapse_to_single_spaces() {
        assert_eq!(display_label("Grok \n\n\t Models"), "Grok Models");
        assert_eq!(display_label("Grok    Models"), "Grok Models");
    }

    #[test]
    fn control_characters_are_removed() {
        // NUL and BEL…
        assert_eq!(display_label("Gro\u{0000}k\u{0007} Models"), "Grok Models");
        // …DEL, VT, FF…
        assert_eq!(display_label("Gro\u{007F}\u{000B}\u{000C}k"), "Grok");
        // …and the C1 range never reach the label either.
        assert_eq!(display_label("Gro\u{009B}k Models"), "Grok Models");
    }

    #[test]
    fn ansi_escape_sequences_do_not_survive() {
        // Color and reset sequences vanish whole — no `[31m` residue.
        assert_eq!(display_label("\u{1b}[31mGrok\u{1b}[0m"), "Grok");
        // Cursor and erase sequences too.
        assert_eq!(display_label("\u{1b}[2J\u{1b}[1;2HGrok"), "Grok");
        // An OSC payload (window-title style) does not leak.
        assert_eq!(display_label("\u{1b}]0;evil\u{0007}Grok"), "Grok");
        // Short and charset-designation escapes, plus a trailing bare ESC.
        assert_eq!(display_label("\u{1b}7Grok"), "Grok");
        assert_eq!(display_label("\u{1b}(BGrok"), "Grok");
        assert_eq!(display_label("Grok\u{1b}"), "Grok");
        // A truncated sequence cannot leak its tail either.
        assert_eq!(display_label("Grok\u{1b}[31"), "Grok");
    }

    #[test]
    fn overlong_labels_are_capped_deterministically() {
        assert_eq!(display_label(&"x".repeat(60)), "x".repeat(MAX_LABEL_CHARS));
        // The cap counts characters, not bytes.
        assert_eq!(
            display_label(&"é".repeat(60)).chars().count(),
            MAX_LABEL_CHARS
        );
    }

    #[test]
    fn combined_hostile_input_is_fully_sanitized() {
        // ANSI + control character + newline + overlong padding in one label.
        let hostile = "\u{1b}[31m Gro\u{0000}k \n\n Models\u{0007} \u{1b}[0m ";
        let sanitized = display_label(&format!("{hostile}{}", " x".repeat(45)));
        assert!(sanitized.starts_with("Grok Models"));
        assert_eq!(sanitized.chars().count(), MAX_LABEL_CHARS);
        assert!(!sanitized.chars().any(char::is_control));
        assert!(!sanitized.contains('\u{1b}'));
        assert!(!sanitized.contains("  "));
    }

    #[test]
    fn cache_path_sanitizes_provider_derived_labels() {
        // Unknown family and window keys carry display-hostile text straight
        // from the cache document; the emitted label must be clean.
        let raw = r#"{
            "accounts": [{ "cachedQuota": {
                "\u001b[32m  grok \u001b[0m": { "windows": [
                    { "window": "\u001b[31mdaily\u001b[0m", "remainingFraction": 0.5 }
                ]}
            }}]
        }"#;
        let limits = parse_quota(raw).unwrap().limits;
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "grok daily");
        assert!(!limits[0].label.contains('\u{1b}'));
    }

    #[test]
    fn live_path_sanitizes_provider_derived_labels() {
        let body = serde_json::json!({
            "groups": [{
                "displayName": " \u{1b}[35m Grok \u{1b}[0m Models ",
                "buckets": [
                    { "window": "\u{1b}[31m daily \u{1b}[0m", "remainingFraction": 0.5 }
                ]
            }]
        });
        let limits = parse_quota_summary(&body);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Grok Models daily");
    }

    #[test]
    fn cache_and_live_paths_apply_the_same_label_sanitization() {
        // The same hostile family text entering through the cache key and the
        // live group displayName must come out identical.
        let hostile = "\u{1b}[33m Gro\u{0007}k \u{1b}[0m";
        let mut cached_quota = serde_json::Map::new();
        cached_quota.insert(
            hostile.to_string(),
            serde_json::json!({ "windows": [{ "window": "5h", "remainingFraction": 0.5 }] }),
        );
        let cache_raw = serde_json::json!({
            "accounts": [{ "cachedQuota": Value::Object(cached_quota) }]
        })
        .to_string();
        let live_body = serde_json::json!({
            "groups": [{
                "displayName": hostile,
                "buckets": [{ "window": "5h", "remainingFraction": 0.5 }]
            }]
        });
        let cache_limits = parse_quota(&cache_raw).unwrap().limits;
        let live_limits = parse_quota_summary(&live_body);
        assert_eq!(cache_limits[0].label, live_limits[0].label);
        assert_eq!(cache_limits[0].label, "Grok");
    }

    // ---------- live credentials extraction ----------

    fn credentials_cache_json(account: Value, active_index: Value) -> String {
        serde_json::json!({
            "version": 4,
            "accounts": [account],
            "activeIndex": active_index
        })
        .to_string()
    }

    #[test]
    fn live_credentials_come_from_the_selected_plugin_account() {
        let raw = credentials_cache_json(
            serde_json::json!({
                "email": "user@example.com",
                "refreshToken": "1//refresh-token",
                "projectId": "aicode-consumers"
            }),
            serde_json::json!(0),
        );
        let credentials = extract_live_credentials(&raw).expect("credentials");
        assert_eq!(credentials.project_id, "aicode-consumers");
        // The token itself is pinned only by length: its value must never
        // appear in any Debug rendering.
        assert_eq!(credentials.refresh_token.len(), "1//refresh-token".len());
        assert!(!format!("{credentials:?}").contains("1//refresh-token"));
    }

    #[test]
    fn live_credentials_fall_back_to_managed_project_id() {
        let raw = credentials_cache_json(
            serde_json::json!({
                "refreshToken": "1//refresh-token",
                "managedProjectId": "managed-project"
            }),
            serde_json::json!(0),
        );
        let credentials = extract_live_credentials(&raw).expect("credentials");
        assert_eq!(credentials.project_id, "managed-project");
    }

    #[test]
    fn live_credentials_require_both_fields() {
        for missing in [
            // No refresh token.
            serde_json::json!({ "projectId": "p" }),
            // Blank refresh token.
            serde_json::json!({ "refreshToken": "   ", "projectId": "p" }),
            // No project at all.
            serde_json::json!({ "refreshToken": "1//r" }),
            // Wrong types.
            serde_json::json!({ "refreshToken": 12, "projectId": "p" }),
        ] {
            let raw = credentials_cache_json(missing, serde_json::json!(0));
            assert!(
                extract_live_credentials(&raw).is_none(),
                "credentials must require token + project"
            );
        }
        // A pre-OAuth cache document has no credentials to extract.
        assert!(extract_live_credentials("{}").is_none());
        assert!(extract_live_credentials("not json").is_none());
        assert!(extract_live_credentials(r#"{"accounts": []}"#).is_none());
    }

    // ---------- live/fallback decision ----------

    #[test]
    fn live_usage_is_fresh_and_stamped_with_the_fetch_time() {
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 22, 50, 0).unwrap();
        let usage = usage_live(parse_quota_summary(&live_summary_body()), now);
        assert_eq!(usage.data_freshness, DataSourceFreshness::Fresh);
        assert_eq!(
            usage.source_updated_at.as_deref(),
            Some("2026-09-29T22:50:00.000Z")
        );
        assert_eq!(usage.limits.len(), 4);
    }

    #[test]
    fn cache_fallback_keeps_the_stale_verdict_semantics() {
        // The real local cache from 2026-09-18 (12 days before the live
        // fix): when the live fetch cannot produce windows, this snapshot is
        // what surfaces — stale, with its exact cached windows and resets.
        let usage = usage_from_cache(&realistic_cache()).unwrap();
        assert_eq!(usage.data_freshness, DataSourceFreshness::Stale);
        assert_eq!(
            usage.source_updated_at.as_deref(),
            Some("2026-09-18T02:13:32.735Z")
        );
        assert_eq!(usage.limits.len(), 4);
    }

    #[test]
    fn cache_fallback_surfaces_its_errors_unchanged() {
        // A credential-bearing cache without any quota snapshot still fails
        // as quota_missing on the fallback path (live unavailability does
        // not invent windows).
        let raw = serde_json::json!({
            "accounts": [{ "refreshToken": "1//r", "projectId": "p" }],
            "activeIndex": 0
        })
        .to_string();
        assert_eq!(usage_from_cache(&raw).unwrap_err().code, "quota_missing");
    }

    // ---------- live-failure cause on the cache fallback ----------

    fn live_failure(code: &str) -> AntigravityError {
        AntigravityError::new(code, "authored display-safe cause")
    }

    #[test]
    fn live_success_carries_no_fallback_cause() {
        let usage = resolve_usage(
            &realistic_cache(),
            Some(Ok(parse_quota_summary(&live_summary_body()))),
        )
        .unwrap();
        assert!(usage.fallback_failure.is_none());
        // The live construction is untouched: fresh verdict, and the four
        // canonical live windows.
        assert_eq!(usage.data_freshness, DataSourceFreshness::Fresh);
        assert_eq!(
            usage
                .limits
                .iter()
                .map(|limit| limit.label.as_str())
                .collect::<Vec<_>>(),
            ["Gemini", "Gemini Weekly", "Claude", "Claude Weekly"]
        );
    }

    #[test]
    fn live_success_wire_has_no_fallback_field() {
        let usage = resolve_usage(
            &realistic_cache(),
            Some(Ok(parse_quota_summary(&live_summary_body()))),
        )
        .unwrap();
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(!wire.contains("fallbackFailure"), "{wire}");
    }

    #[test]
    fn live_failure_falls_back_to_cache_and_retains_the_cause() {
        // Representative failure categories: auth-type, transport, schema.
        for code in ["auth_invalid", "network", "unexpected_response"] {
            let usage =
                resolve_usage(&realistic_cache(), Some(Err(live_failure(code)))).unwrap();
            // Cached windows remain exactly usable as before — same labels,
            // same exact snapshot stamp, same stale verdict.
            assert_eq!(
                usage
                    .limits
                    .iter()
                    .map(|limit| limit.label.as_str())
                    .collect::<Vec<_>>(),
                ["Gemini", "Gemini Weekly", "Claude", "Claude Weekly"]
            );
            assert_eq!(
                usage.source_updated_at.as_deref(),
                Some("2026-09-18T02:13:32.735Z")
            );
            assert_eq!(usage.data_freshness, DataSourceFreshness::Stale);
            // The fresh live-failure cause is retained, verbatim.
            let cause = usage.fallback_failure.as_ref().unwrap_or_else(|| {
                panic!("fallback cause must be retained for {code}")
            });
            assert_eq!(cause.code, code);
            assert_eq!(cause.message, "authored display-safe cause");
        }
    }

    #[test]
    fn live_failure_without_usable_cache_fails_as_before() {
        // A credential-bearing cache without any quota snapshot: the cache's
        // own error stands — the live failure never replaces or masks it.
        let raw = serde_json::json!({
            "accounts": [{ "refreshToken": "1//r", "projectId": "p" }],
            "activeIndex": 0
        })
        .to_string();
        let error =
            resolve_usage(&raw, Some(Err(live_failure("network")))).unwrap_err();
        assert_eq!(error.code, "quota_missing");
    }

    #[test]
    fn no_live_attempt_reads_the_cache_unchanged() {
        // The no-credentials path (the cache is the only source) attaches no
        // fallback cause — no live attempt happened to explain.
        let usage = resolve_usage(&realistic_cache(), None).unwrap();
        assert!(usage.fallback_failure.is_none());
        assert_eq!(usage.limits.len(), 4);
    }

    #[test]
    fn fallback_wire_is_display_safe_and_camel_case() {
        let usage = resolve_usage(
            &realistic_cache(),
            Some(Err(live_failure("auth_invalid"))),
        )
        .unwrap();
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(
            wire.contains(r#""fallbackFailure":{"code":"auth_invalid""#),
            "{wire}"
        );
        // No credential, account identity, or cache-internal field may ride
        // the fallback metadata onto the wire.
        assert!(!wire.contains("1//super-secret-refresh-token"));
        assert!(!wire.contains("super-secret-session-token"));
        assert!(!wire.contains("user@example.com"));
        assert!(!wire.contains("fingerprint"));
    }

    #[test]
    fn token_refresh_failures_classify_by_status_class() {
        // 4xx: the stored refresh token was rejected — an auth-type failure.
        for status in [400, 401, 403, 422] {
            let error = token_refresh_error(status);
            assert_eq!(error.code, "auth_invalid", "status {status}");
            assert!(error.message.contains(&format!("HTTP {status}")));
        }
        // Any other status is an unexpected response, not an auth verdict.
        for status in [500, 502, 503] {
            let error = token_refresh_error(status);
            assert_eq!(error.code, "unexpected_response", "status {status}");
            assert!(error.message.contains(&format!("HTTP {status}")));
        }
        // Authored, display-safe messages: a bare status number, nothing else.
        assert!(token_refresh_error(401).message.starts_with("Antigravity "));
    }

    #[test]
    fn quota_fetch_failures_classify_into_the_shared_vocabulary() {
        assert_eq!(
            quota_fetch_error(QuotaFetchFailure::Transport).code,
            "network"
        );
        for status in [429, 500, 503] {
            let error = quota_fetch_error(QuotaFetchFailure::HttpStatus(status));
            assert_eq!(error.code, "unexpected_response", "status {status}");
            assert!(error.message.contains(&format!("HTTP {status}")));
        }
        assert_eq!(
            quota_fetch_error(QuotaFetchFailure::MalformedBody).code,
            "unexpected_response"
        );
        assert_eq!(unusable_quota_response().code, "unexpected_response");
    }

    #[test]
    #[ignore = "live test: requires a real local Antigravity credential and network access"]
    fn live_fetch_returns_windows() {
        let usage = tauri::async_runtime::block_on(read_antigravity_usage()).expect("live read");
        println!(
            "source: {:?} freshness: {:?}",
            usage.source_updated_at.as_deref(),
            usage.data_freshness
        );
        for limit in &usage.limits {
            println!(
                "{}: used {:.2}% reset_at {:?}",
                limit.label,
                limit.used_percent,
                limit.reset_at.as_deref().unwrap_or("-")
            );
        }
        assert!(!usage.limits.is_empty(), "expected at least one window");
    }
}
