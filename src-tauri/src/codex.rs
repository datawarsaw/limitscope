//! OpenAI / Codex provider backend.
//!
//! Reads the local Codex CLI login state (`~/.codex/auth.json`, read-only) and
//! queries the same ChatGPT usage backend the Codex CLI uses. Tokens stay in
//! this process: nothing beyond normalized windows is returned to the WebView,
//! and no auth material is ever logged.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use crate::provider_error::ProviderError;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const USER_AGENT: &str = concat!("rate-limits/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
// Named consts because plain arithmetic is not allowed in match patterns.
const FIVE_HOURS: i64 = 5 * HOUR;
const ONE_WEEK: i64 = 7 * DAY;
const THIRTY_DAYS: i64 = 30 * DAY;

// ---------- data returned to the WebView (camelCase on the wire) ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexLimitWindow {
    pub label: String,
    pub used_percent: f64,
    /// RFC 3339 UTC timestamp, exactly as reported (or derived) upstream.
    pub reset_at: Option<String>,
}

/// Masked attribution of the one local Codex session: the tail of the
/// `tokens.account_id` the request is already scoped with, so the card can
/// name the account without any token material crossing the boundary.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexAccount {
    pub account_hint: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsage {
    pub limits: Vec<CodexLimitWindow>,
    pub plan_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<CodexAccount>,
    /// Optional capability: present only when the usage payload carried an
    /// explicitly reported, account-bound banked balance. Quota windows are
    /// unaffected by its absence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<CodexResetCredits>,
}

/// Provider-specific banked reset-credit observation (v0.7, Codex only).
///
/// Critical semantic distinction (reset-budget discovery 2026-09-30):
/// banked_credits is an explicitly reported balance of banked reset
/// credits — it is NOT a count of replenishments available now, NOT a count
/// of windows/resets/history transitions, and must never be derived from
/// them. currently_applicable is the separate applicability field when
/// the backend reports one; absent means unknown, never zero. There is no
/// expiry here: per-credit expirations live only in the read-only
/// reset-credit details endpoint, which this capability deliberately does
/// not call (smallest surface: the usage payload alone).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetCredits {
    pub banked_credits: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currently_applicable: Option<u32>,
    /// RFC 3339 UTC time of the observation. Consumers must treat the count
    /// as current only within RESET_CREDITS_TTL_SECS of this stamp.
    pub checked_at: String,
    /// Provenance of the observation; today always RESET_CREDITS_SOURCE.
    pub source: &'static str,
}

fn not_logged_in() -> ProviderError {
    ProviderError::new(
        "not_logged_in",
        "No ChatGPT login found for Codex. Open Codex or run `codex login` first.",
    )
}

fn auth_expired() -> ProviderError {
    ProviderError::new(
        "auth_expired",
        "Codex ChatGPT session is expired or was rejected. Open Codex (or run `codex login`) to refresh it, then refresh again.",
    )
}

// ---------- upstream response (only the fields we consume; extra ignored) ----------

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    /// Explicit banked reset-credit summary. Kept untyped and validated
    /// strictly below: a malformed credit block degrades to "credits
    /// unavailable", never to a quota failure.
    #[serde(default)]
    rate_limit_reset_credits: Option<Value>,
}

#[derive(Debug, Deserialize, Default)]
struct ResetCreditSummary {
    #[serde(default)]
    available_count: Option<Value>,
    #[serde(default)]
    applicable_available_count: Option<Value>,
}

/// Strictly validates an explicitly reported count: a JSON integer >= 0 that
/// fits u32. Floats, strings, bools, negatives, nulls, and overflow all read
/// as absent — never coerced, never clamped.
fn strict_count(value: Option<&Value>) -> Option<u32> {
    let number = value?.as_u64()?;
    u32::try_from(number).ok()
}

#[derive(Debug, Deserialize)]
struct RateLimit {
    #[serde(default)]
    primary_window: Option<RateWindow>,
    #[serde(default)]
    secondary_window: Option<RateWindow>,
}

#[derive(Debug, Deserialize)]
struct RateWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    /// Unix seconds, as reported by the usage backend.
    #[serde(default)]
    reset_at: Option<i64>,
}

// ---------- normalization ----------

/// Freshness budget for a banked-credit observation (v0.7).
///
/// Rationale: the WHAM usage payload is re-queried live every refresh cycle
/// (default 5 minutes), and a banked balance can be consumed at any time by
/// a redemption outside this app — visible only on the next fetch.
/// Carried-forward caching without field-level freshness (e.g. the OpenCodex
/// resetCredits cache field) is explicitly untrusted by the discovery, so a
/// count older than roughly three default cycles must stop reading as
/// current rather than risk presenting a redeemed balance as banked.
pub const RESET_CREDITS_TTL_SECS: i64 = 15 * 60;
/// Provenance string stamped on every credit observation from this source.
pub const RESET_CREDITS_SOURCE: &str = "codex-wham-usage";
/// Clock-skew tolerance for observation timestamps (5 minutes), matching the
/// persisted last-good store.
const OBSERVED_FUTURE_TOLERANCE_SECS: i64 = 300;

/// True when a credit observation timestamp is still within its freshness
/// budget: parseable, not future-dated beyond clock-skew tolerance, and not
/// older than the TTL. Anything else is a stale observation and must read as
/// unavailable, never as a current balance.
pub fn reset_credits_fresh(checked_at: &str, now: DateTime<Utc>) -> bool {
    let parsed: DateTime<Utc> = match checked_at.parse() {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    let age = now.signed_duration_since(parsed).num_seconds();
    age >= -OBSERVED_FUTURE_TOLERANCE_SECS && age <= RESET_CREDITS_TTL_SECS
}

/// Display-safe account binding shared by the quota windows and the credit
/// observation of one fetch. The served account wins when present (it is
/// what the windows actually describe); otherwise the requested local scope
/// is used. None when neither proves an identity. Masked with the same
/// `account_hint` vocabulary the failure stamps and the last-good guard use.
fn codex_account(observed_account: Option<&str>, local_account: Option<&str>) -> Option<CodexAccount> {
    let raw = observed_account
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| local_account.map(str::trim).filter(|s| !s.is_empty()))?;
    account_hint(raw).map(|account_hint| CodexAccount { account_hint })
}

/// Binds a credit observation to the account identity the Codex provider
/// uses, or returns None (credits unavailable) when the binding cannot be
/// proven. observed is the account the backend says it served; local is the
/// account scope of the stored credential.
///
/// - Observed and local agree, or no local scope is stored: masked binding.
/// - Observed contradicts the local scope: unknown (a swap mid-flight must
///   not attach one account's balance to another).
/// - Observed absent: unknown (an unbound count must never read as the
///   current account's balance).
fn bind_credit_account(
    observed_account: Option<&str>,
    local_account: Option<&str>,
) -> Option<CodexAccount> {
    let observed = observed_account.map(str::trim).filter(|s| !s.is_empty())?;
    if let Some(local) = local_account.map(str::trim).filter(|s| !s.is_empty()) {
        if observed != local {
            return None;
        }
    }
    account_hint(observed).map(|account_hint| CodexAccount { account_hint })
}

/// Extracts the optional reset-credit capability from an already-validated
/// usage payload. Never fails: any missing/malformed/unbound credit data
/// yields None (capability unavailable) while the quota windows stand.
fn parse_reset_credits(
    summary: Option<&Value>,
    observed_account: Option<&str>,
    local_account: Option<&str>,
    now: DateTime<Utc>,
) -> Option<CodexResetCredits> {
    // A wrongly-typed summary (schema drift) degrades to unavailable here
    // instead of failing the whole usage parse upstream. The documented
    // shape is an object; anything else (including arrays, which serde
    // would otherwise read positionally) is rejected outright.
    let raw = summary?;
    if !raw.is_object() {
        return None;
    }
    let summary: ResetCreditSummary = serde_json::from_value(raw.clone()).ok()?;
    // The balance is only meaningful bound to its account.
    bind_credit_account(observed_account, local_account)?;
    let banked_credits = strict_count(summary.available_count.as_ref())?;
    Some(CodexResetCredits {
        banked_credits,
        currently_applicable: strict_count(summary.applicable_available_count.as_ref()),
        checked_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        source: RESET_CREDITS_SOURCE,
    })
}

fn window_label(limit_window_seconds: Option<i64>) -> String {
    match limit_window_seconds {
        Some(FIVE_HOURS) => "5-hour".to_string(),
        Some(DAY) => "Daily".to_string(),
        Some(ONE_WEEK) => "Weekly".to_string(),
        Some(THIRTY_DAYS) => "30-day".to_string(),
        Some(seconds) => humanize_seconds(seconds),
        None => "Usage".to_string(),
    }
}

fn humanize_seconds(seconds: i64) -> String {
    if seconds <= 0 {
        "Usage".to_string()
    } else if seconds < HOUR {
        format!("{}-minute", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}-hour", seconds / HOUR)
    } else {
        format!("{}-day", seconds / DAY)
    }
}

/// Maps one upstream window to the app's shape, or `None` when the window
/// carries no usable usage figure (no invented limits).
fn normalize_window(window: RateWindow, now: DateTime<Utc>) -> Option<CodexLimitWindow> {
    let used_percent = window.used_percent?.clamp(0.0, 100.0);
    let reset_at = window
        .reset_at
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        .or_else(|| {
            window
                .reset_after_seconds
                .and_then(|seconds| now.checked_add_signed(TimeDelta::seconds(seconds)))
        })
        .map(|reset| reset.to_rfc3339_opts(SecondsFormat::Secs, true));
    Some(CodexLimitWindow {
        label: window_label(window.limit_window_seconds),
        used_percent,
        reset_at,
    })
}

// ---------- local auth state (read-only) ----------

struct CodexAuth {
    access_token: String,
    account_id: Option<String>,
}

// Manual impl so the token can never reach logs through Debug formatting.
impl std::fmt::Debug for CodexAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAuth")
            .field("access_token", &"<redacted>")
            .field("account_id", &self.account_id)
            .finish()
    }
}

fn codex_dir() -> Result<std::path::PathBuf, ProviderError> {
    let home = std::env::home_dir().ok_or_else(|| {
        ProviderError::new("codex_not_installed", "Could not locate the user home directory.")
    })?;
    Ok(codex_home_dir(std::env::var("CODEX_HOME").ok().as_deref(), &home))
}

/// Resolves the Codex home the adapter reads auth state from. A standard
/// `CODEX_HOME` override wins when it names a usable (existing) directory;
/// anything else — unset, empty, or unusable — keeps the default `~/.codex`.
/// Pure so tests never touch the process environment.
fn codex_home_dir(codex_home: Option<&str>, home: &Path) -> std::path::PathBuf {
    if let Some(dir) = codex_home.map(str::trim).filter(|dir| !dir.is_empty()) {
        let dir = std::path::PathBuf::from(dir);
        if dir.is_dir() {
            return dir;
        }
    }
    home.join(".codex")
}

fn load_auth(dir: &Path) -> Result<CodexAuth, ProviderError> {
    if !dir.exists() {
        return Err(ProviderError::new(
            "codex_not_installed",
            "Codex is not installed (~/.codex not found).",
        ));
    }
    let auth_path = dir.join("auth.json");
    let raw = match fs::read_to_string(&auth_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(not_logged_in()),
        Err(error) => {
            return Err(ProviderError::new(
                "auth_unreadable",
                format!("Could not read Codex auth state: {error}"),
            ))
        }
    };
    parse_auth(&raw)
}

fn parse_auth(raw: &str) -> Result<CodexAuth, ProviderError> {
    let value: Value = serde_json::from_str(raw).map_err(|_| {
        ProviderError::new("auth_unreadable", "Codex auth state (auth.json) is not valid JSON.")
    })?;
    let tokens = match value.get("tokens").filter(|tokens| tokens.is_object()) {
        Some(tokens) => tokens,
        None => return Err(not_logged_in()),
    };
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty());
    let Some(access_token) = access_token else {
        return Err(not_logged_in());
    };
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    Ok(CodexAuth {
        access_token: access_token.to_string(),
        account_id,
    })
}

/// Expiry claim from the access token's JWT payload, when parseable.
/// Only the timestamp is used — the token itself stays in place.
fn jwt_expiry(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp")?.as_i64()
}

/// Masked attempted-account hint from `tokens.account_id` — the stable
/// account identifier the usage request is already scoped with (as the
/// `chatgpt-account-id` header). Only its last eight characters are used, so
/// the hint distinguishes account A from account B without exposing the
/// identifier; no JWT is decoded beyond the expiry check above, and no email
/// or token material is involved. `None` when the id is too short to mask
/// meaningfully (the tail would leak most of it) or the tail is not
/// identifier-shaped.
fn account_hint(account_id: &str) -> Option<String> {
    let id = account_id.trim();
    let length = id.chars().count();
    if length < 16 {
        return None;
    }
    let tail: String = id.chars().skip(length - 8).collect();
    if !tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some(tail)
}

/// The identity string the success attribution and every post-resolution
/// failure build, so the runtime's last-good guard compares like-for-like.
fn codex_identity(account_id: Option<&str>) -> Option<String> {
    account_id
        .and_then(account_hint)
        .map(|tail| format!("chatgpt:{tail}"))
}

// ---------- HTTP ----------

fn http_client() -> Result<&'static reqwest::Client, ProviderError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(client) => Ok(client),
        Err(error) => Err(ProviderError::new(
            "unexpected",
            format!("Could not initialize HTTP client: {error}"),
        )),
    }
}

fn unexpected_usage_response(reason: &str) -> ProviderError {
    // Schema changes are deterministic: the structured `transient: false`
    // verdict keeps the frontend from retrying them regardless of wording.
    ProviderError::new(
        "unexpected_response",
        format!("Codex usage response format changed; {reason}"),
    )
}

/// Parses and validates a successful (HTTP 200) usage payload.
///
/// A 200 body without a usable primary rate-limit window is a schema change,
/// not an empty result: answering success here would let the frontend replace
/// its last-good windows with an empty "unknown" card. Mirrors Z.ai and
/// OpenCode Go, which treat unusable successful payloads as provider errors.
///
/// The reset-credit capability stays independent of that verdict: credit
/// fields are parsed best-effort from the same payload, so windows available
/// with credit data missing or malformed still yields a Live quota with the
/// credit capability unavailable — never a quota failure.
fn parse_usage_response(
    body: &[u8],
    now: DateTime<Utc>,
    local_account: Option<&str>,
) -> Result<CodexUsage, ProviderError> {
    let parsed: UsageResponse = serde_json::from_slice(body)
        .map_err(|_| unexpected_usage_response("no known windows found."))?;
    let Some(rate_limit) = parsed.rate_limit else {
        return Err(unexpected_usage_response("no rate-limit payload found."));
    };
    let Some(primary) = rate_limit.primary_window else {
        return Err(unexpected_usage_response(
            "no primary rate-limit window found.",
        ));
    };
    let Some(primary_limit) = normalize_window(primary, now) else {
        return Err(unexpected_usage_response(
            "the primary rate-limit window carries no usable usage figure.",
        ));
    };
    // The secondary window stays best-effort: present and usable → included,
    // otherwise dropped (no invented limits).
    let mut limits = vec![primary_limit];
    limits.extend(
        rate_limit
            .secondary_window
            .and_then(|window| normalize_window(window, now)),
    );
    // Never derive a banked balance from windows, resets, history, or usage
    // drops: only the explicit summary counts, bound to its account.
    let observed_account = parsed.account_id.as_deref();
    Ok(CodexUsage {
        limits,
        plan_type: parsed.plan_type,
        // Windows name the served account when the payload proves one (it is
        // what the windows actually describe); otherwise the requested local
        // scope. Credits bind strictly: unobserved or contradicting accounts
        // leave the capability unavailable rather than misattributed.
        account: codex_account(observed_account, local_account),
        reset_credits: parse_reset_credits(
            parsed.rate_limit_reset_credits.as_ref(),
            observed_account,
            local_account,
            now,
        ),
    })
}

// ---------- fetch + command ----------

async fn fetch_codex_usage() -> Result<CodexUsage, ProviderError> {
    let dir = codex_dir()?;
    let auth = load_auth(&dir)?;
    // The attempted account is known as soon as auth.json is resolved; every
    // failure after this point carries its masked identity so the runtime can
    // never retain one account's last-good data for a different account (the
    // same guard pattern the OpenCode Go backend stamps).
    let identity = codex_identity(auth.account_id.as_deref());
    // Final-boundary value scrub (`secret_scrub`): the access token is alive
    // for this whole fetch, so any exact occurrence of it in a failure
    // message is removed before the error leaves the adapter.
    fetch_usage_for_auth(&auth, identity)
        .await
        .map_err(|error| {
            crate::secret_scrub::scrub_provider_error(error, &[auth.access_token.as_str()])
        })
}

/// The usage fetch for an already-resolved credential. No token refresh: an
/// expired session fails here and stays failed until Codex itself re-auths.
async fn fetch_usage_for_auth(
    auth: &CodexAuth,
    identity: Option<String>,
) -> Result<CodexUsage, ProviderError> {
    // authoritative check (401/403 below).
    if let Some(exp) = jwt_expiry(&auth.access_token) {
        if Utc::now().timestamp() >= exp {
            return Err(auth_expired().with_identity_hint(identity.clone()));
        }
    }

    let client = http_client()?;
    let mut request = client
        .get(USAGE_URL)
        .bearer_auth(&auth.access_token)
        .header("accept", "application/json");
    if let Some(account_id) = &auth.account_id {
        request = request.header("chatgpt-account-id", account_id);
    }

    let response = request.send().await.map_err(|error| {
        crate::provider_error::transport_failure(
            "network",
            format!("Could not reach the Codex usage endpoint: {error}"),
            &error,
        )
        .with_identity_hint(identity.clone())
    })?;
    let status = response.status();
    let retry_after = crate::provider_error::retry_after_header(response.headers()).map(String::from);
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(auth_expired().with_identity_hint(identity.clone()));
    }
    let body = response.bytes().await.map_err(|error| {
        crate::provider_error::transport_failure(
            "network",
            format!("Codex usage response was cut short: {error}"),
            &error,
        )
        .with_identity_hint(identity.clone())
    })?;
    if !status.is_success() {
        return Err(ProviderError::http_failure_with_retry_after(
            status,
            format!("Codex usage endpoint returned HTTP {status}."),
            retry_after.as_deref(),
            Utc::now(),
        )
        .with_identity_hint(identity.clone()));
    }

    // Attribution and credit binding resolve inside the parse (served
    // account wins, requested local scope as fallback), using the same
    // masked identity vocabulary the failure stamps carry.
    parse_usage_response(&body, Utc::now(), auth.account_id.as_deref())
        .map_err(|error| error.with_identity_hint(identity.clone()))
}

#[tauri::command]
pub async fn get_codex_usage() -> Result<CodexUsage, ProviderError> {
    fetch_codex_usage().await
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
    async fn redirect_is_returned_without_forwarding_credentials() {
        let destination = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        destination.set_nonblocking(true).unwrap();
        let destination_addr = destination.local_addr().unwrap();
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let source_addr = source.local_addr().unwrap();
        let (destination_tx, destination_rx) = std::sync::mpsc::channel();
        let destination_thread = thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
            while std::time::Instant::now() < deadline {
                match destination.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0; 2048];
                        let _ = stream.read(&mut request);
                        let _ = destination_tx.send(true);
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
            let _ = destination_tx.send(false);
        });
        let source_thread = thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{destination_addr}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .unwrap();
            String::from_utf8_lossy(&request).to_ascii_lowercase()
        });

        let response = http_client()
            .unwrap()
            .get(format!("http://{source_addr}/usage"))
            .bearer_auth("unit-test-token")
            .header("chatgpt-account-id", "unit-test-account")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        let received = source_thread.join().unwrap();
        assert!(received.contains("authorization: bearer unit-test-token"));
        assert!(received.contains("chatgpt-account-id: unit-test-account"));
        assert!(!destination_rx.recv().unwrap());
        destination_thread.join().unwrap();
    }

    fn window_json(used_percent: f64) -> Value {
        serde_json::json!({
            "used_percent": used_percent,
            "limit_window_seconds": 604_800,
            "reset_after_seconds": 503_573,
            "reset_at": 1_791_011_721_i64,
        })
    }

    #[test]
    fn label_matches_known_window_sizes() {
        assert_eq!(window_label(Some(5 * HOUR)), "5-hour");
        assert_eq!(window_label(Some(DAY)), "Daily");
        assert_eq!(window_label(Some(7 * DAY)), "Weekly");
        assert_eq!(window_label(Some(30 * DAY)), "30-day");
        assert_eq!(window_label(Some(3 * DAY)), "3-day");
        assert_eq!(window_label(Some(90 * MINUTE)), "1-hour");
        assert_eq!(window_label(None), "Usage");
    }

    #[test]
    fn normalize_clamps_percent_and_keeps_reset() {
        let now = Utc::now();
        let over = normalize_window(
            serde_json::from_value(window_json(150.0)).unwrap(),
            now,
        )
        .unwrap();
        assert_eq!(over.used_percent, 100.0);
        assert_eq!(over.label, "Weekly");
        assert_eq!(over.reset_at.as_deref(), Some("2026-10-03T07:15:21Z"));

        let under = normalize_window(
            serde_json::from_value(window_json(-5.0)).unwrap(),
            now,
        )
        .unwrap();
        assert_eq!(under.used_percent, 0.0);
    }

    #[test]
    fn normalize_skips_window_without_usage() {
        let raw = serde_json::json!({ "limit_window_seconds": 604_800 });
        let window: RateWindow = serde_json::from_value(raw).unwrap();
        assert!(normalize_window(window, Utc::now()).is_none());
    }

    #[test]
    fn reset_falls_back_to_relative_seconds() {
        let raw = serde_json::json!({ "used_percent": 42.0, "reset_after_seconds": 3_600 });
        let window: RateWindow = serde_json::from_value(raw).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap();
        let normalized = normalize_window(window, now).unwrap();
        assert_eq!(normalized.reset_at.as_deref(), Some("2026-09-27T13:00:00Z"));
    }

    #[test]
    fn negative_relative_reset_keeps_the_window_for_downstream_plausibility() {
        // A negative relative reset still yields a window with a past stamp;
        // the provider never errors the payload for it — the runtime reset
        // plausibility gate drops the bound downstream without touching
        // siblings.
        let raw = serde_json::json!({ "used_percent": 42.0, "limit_window_seconds": 18_000, "reset_after_seconds": -30 });
        let window: RateWindow = serde_json::from_value(raw).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap();
        let normalized = normalize_window(window, now).unwrap();
        assert_eq!(normalized.label, "5-hour");
        assert_eq!(normalized.reset_at.as_deref(), Some("2026-09-27T11:59:30Z"));
    }

    #[test]
    fn valid_primary_window_parses_to_one_window() {
        // Shape observed live from GET /backend-api/wham/usage on a team plan.
        let body = serde_json::json!({
            "user_id": "user-x",
            "account_id": "00000000-0000-0000-0000-000000000000",
            "email": "someone@example.com",
            "plan_type": "team",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": window_json(1.0),
                "secondary_window": null
            },
            "code_review_rate_limit": null
        });
        let usage = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap();
        assert_eq!(usage.plan_type.as_deref(), Some("team"));
        assert_eq!(usage.limits.len(), 1);
        assert_eq!(usage.limits[0].label, "Weekly");
        assert_eq!(usage.limits[0].used_percent, 1.0);
        assert_eq!(usage.limits[0].reset_at.as_deref(), Some("2026-10-03T07:15:21Z"));
    }

    #[test]
    fn primary_and_secondary_windows_both_normalize() {
        let body = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": window_json(1.0),
                "secondary_window": {
                    "used_percent": 55.5,
                    "limit_window_seconds": 300,
                    "reset_after_seconds": 120
                }
            }
        });
        let usage = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap();
        assert_eq!(usage.limits.len(), 2);
        assert_eq!(usage.limits[0].label, "Weekly");
        assert_eq!(usage.limits[1].label, "5-minute");
        assert_eq!(usage.limits[1].used_percent, 55.5);
        assert!(usage.limits[1].reset_at.is_some());
    }

    #[test]
    fn unusable_secondary_window_is_dropped_not_fatal() {
        let body = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": window_json(1.0),
                "secondary_window": { "limit_window_seconds": 300 }
            }
        });
        let usage = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap();
        assert_eq!(usage.limits.len(), 1);
        assert_eq!(usage.limits[0].label, "Weekly");
    }

    #[test]
    fn missing_rate_limit_is_a_schema_error() {
        let body = serde_json::json!({ "plan_type": "free", "extra_field": 1 });
        let error = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error.message.contains("no rate-limit payload found."));
    }

    #[test]
    fn null_rate_limit_is_a_schema_error() {
        let body = serde_json::json!({ "plan_type": "free", "rate_limit": null });
        let error = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error.message.contains("no rate-limit payload found."));
    }

    #[test]
    fn missing_primary_window_is_a_schema_error() {
        let body = serde_json::json!({
            "plan_type": "free",
            "rate_limit": { "allowed": true, "secondary_window": window_json(2.0) }
        });
        let error = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error.message.contains("no primary rate-limit window found."));
    }

    #[test]
    fn malformed_primary_window_is_a_schema_error() {
        // Present but without a usable usage figure: no invented windows.
        let body = serde_json::json!({
            "plan_type": "free",
            "rate_limit": { "primary_window": { "limit_window_seconds": 300 } }
        });
        let error = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error.message.contains("no usable usage figure."));

        // Null primary window reads the same way.
        let body = serde_json::json!({
            "plan_type": "free",
            "rate_limit": { "primary_window": null }
        });
        let error = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn shape_incompatible_payload_is_a_schema_error() {
        for body in ["[]", "\"ok\"", "{\"rate_limit\": 5}", "not json at all"] {
            let error = parse_usage_response(body.as_bytes(), Utc::now(), None).unwrap_err();
            assert_eq!(error.code, "unexpected_response", "body: {body}");
        }
    }

    /// Schema errors are deterministic: they must carry the explicit
    /// `transient: false` verdict so the frontend never retries them,
    /// whatever their message says.
    #[test]
    fn schema_errors_are_classified_deterministic() {
        let bodies = [
            serde_json::json!({ "plan_type": "free" }),
            serde_json::json!({ "rate_limit": null }),
            serde_json::json!({ "rate_limit": { "primary_window": null } }),
            serde_json::json!({ "rate_limit": { "primary_window": {} } }),
        ];
        for body in bodies {
            let error =
                parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
            assert_eq!(error.code, "unexpected_response");
            assert_eq!(error.transient, Some(false), "{}", error.message);
            assert_eq!(error.http_status, None);
        }
    }

    #[test]
    fn error_metadata_matches_the_retry_contract() {
        // 429/5xx and transport failures are transient; auth and other
        // non-success statuses are not.
        let rate_limited = ProviderError::http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Codex usage endpoint returned HTTP 429 Too Many Requests.",
        );
        assert_eq!(rate_limited.transient, Some(true));
        assert_eq!(rate_limited.http_status, Some(429));
        assert_eq!(
            ProviderError::http_failure(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "Codex usage endpoint returned HTTP 503 Service Unavailable.",
            )
            .transient,
            Some(true)
        );
        assert_eq!(
            ProviderError::http_failure(
                reqwest::StatusCode::NOT_FOUND,
                "Codex usage endpoint returned HTTP 404 Not Found.",
            )
            .transient,
            Some(false)
        );
        assert_eq!(ProviderError::transient("network", "offline").transient, Some(true));
        assert_eq!(auth_expired().transient, Some(false));
        assert_eq!(not_logged_in().transient, Some(false));
    }

    #[test]
    fn error_wire_format_is_camel_case_and_omits_absent_metadata() {
        let wire = serde_json::to_string(&ProviderError::http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Codex usage endpoint returned HTTP 429 Too Many Requests.",
        ))
        .unwrap();
        assert!(wire.contains("\"httpStatus\":429"), "wire: {wire}");
        assert!(wire.contains("\"transient\":true"), "wire: {wire}");

        // Deterministic errors carry the explicit verdict but no status.
        let wire = serde_json::to_string(&auth_expired()).unwrap();
        assert!(wire.contains("\"transient\":false"), "wire: {wire}");
        assert!(!wire.contains("httpStatus"), "wire: {wire}");
        // Error messages must not embed credential material.
        assert!(!wire.to_lowercase().contains("token"), "wire: {wire}");
    }

    #[test]
    fn serialized_usage_never_carries_credential_material() {
        // The upstream body contains account data; only normalized windows,
        // the plan type, and the masked account hint may reach the WebView.
        let body = serde_json::json!({
            "user_id": "user-x",
            "account_id": "acct-1234567890abcdef",
            "email": "someone@example.com",
            "access_token": "sk-secret-token",
            "tokens": { "access_token": "sk-secret-token" },
            "plan_type": "team",
            "rate_limit": { "primary_window": window_json(1.0) }
        });
        let usage = parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap();
        let wire = serde_json::to_string(&usage).unwrap();
        for secret in ["sk-secret-token", "acct-1234567890abcdef", "someone@example.com", "user-x"] {
            assert!(!wire.contains(secret), "leaked: {secret}");
        }
        // The served account binds as a masked hint only; no credit summary
        // was reported, so the capability stays unavailable.
        assert_eq!(
            usage.account,
            Some(CodexAccount { account_hint: "90abcdef".to_string() })
        );
        assert_eq!(usage.reset_credits, None);
        let value: Value = serde_json::from_str(&wire).unwrap();
        // serde_json orders map keys alphabetically; compare as a set.
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["account", "limits", "planType"]);
        assert!(wire.contains("90abcdef"), "masked hint missing: {wire}");
    }

    #[test]
    fn parse_auth_reads_chatgpt_tokens() {
        let raw = serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "id",
                "access_token": " access-token ",
                "refresh_token": "refresh",
                "account_id": "abc-123"
            },
            "last_refresh": "2026-09-22T20:37:07Z"
        })
        .to_string();
        let auth = parse_auth(&raw).unwrap();
        assert_eq!(auth.access_token, "access-token");
        assert_eq!(auth.account_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn parse_auth_rejects_missing_tokens() {
        assert_eq!(parse_auth(r#"{"OPENAI_API_KEY":"sk-x"}"#).unwrap_err().code, "not_logged_in");
        assert_eq!(parse_auth(r#"{"tokens":{}}"#).unwrap_err().code, "not_logged_in");
        assert_eq!(parse_auth("not json").unwrap_err().code, "auth_unreadable");
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("limitscope-codex-test-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn codex_home_absent_uses_the_default_home() {
        let home = Path::new("/users/example");
        assert_eq!(codex_home_dir(None, home), home.join(".codex"));
    }

    #[test]
    fn codex_home_override_wins_when_usable() {
        let dir = unique_temp_dir("override");
        let home = Path::new("/users/example");
        assert_eq!(codex_home_dir(Some(dir.to_str().unwrap()), home), dir);
        // Surrounding whitespace is not part of the path.
        let padded = format!("  {}  ", dir.to_str().unwrap());
        assert_eq!(codex_home_dir(Some(&padded), home), dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_home_empty_or_unusable_override_falls_back_to_default() {
        let home = Path::new("/users/example");
        let fallback = home.join(".codex");
        assert_eq!(codex_home_dir(Some(""), home), fallback);
        assert_eq!(codex_home_dir(Some("   "), home), fallback);
        // A path that does not exist…
        let missing = std::env::temp_dir().join("limitscope-codex-test-missing-does-not-exist");
        assert_eq!(codex_home_dir(Some(missing.to_str().unwrap()), home), fallback);
        // …and one that exists but is a file, not a directory.
        let dir = unique_temp_dir("unusable");
        let file = dir.join("not-a-dir");
        fs::write(&file, b"").unwrap();
        assert_eq!(codex_home_dir(Some(file.to_str().unwrap()), home), fallback);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_auth_reads_auth_json_under_the_resolved_home() {
        // The resolved home → auth.json join is unchanged, including when the
        // home came from a CODEX_HOME-style override.
        let dir = unique_temp_dir("auth");
        fs::write(
            dir.join("auth.json"),
            r#"{"tokens":{"access_token":"at","account_id":"abc-123"}}"#,
        )
        .unwrap();
        let auth = load_auth(&dir).unwrap();
        assert_eq!(auth.access_token, "at");
        assert_eq!(auth.account_id.as_deref(), Some("abc-123"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_auth_error_categories_are_unchanged() {
        // A missing home reads as "not installed"; a home without auth.json
        // reads as "not logged in" — whether defaulted or overridden.
        let missing = std::env::temp_dir().join("limitscope-codex-test-missing-home-does-not-exist");
        assert_eq!(load_auth(&missing).unwrap_err().code, "codex_not_installed");
        let dir = unique_temp_dir("empty-home");
        assert_eq!(load_auth(&dir).unwrap_err().code, "not_logged_in");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn jwt_expiry_reads_payload_claim() {
        let encode = |payload: Value| {
            let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
            format!("e30.{payload}.signature")
        };
        let expired = encode(serde_json::json!({ "exp": 1_000_000 }));
        assert_eq!(jwt_expiry(&expired), Some(1_000_000));
        assert_eq!(jwt_expiry("garbage"), None);
    }

    /// The masked account hint is the last eight characters of the
    /// `tokens.account_id` the request is already scoped with.
    #[test]
    fn account_hint_is_the_account_id_tail() {
        let hint = account_hint("12345678-1234-1234-1234-123456789abc").unwrap();
        assert_eq!(hint, "56789abc");
        // Deterministic per account: the same id derives the same hint.
        assert_eq!(account_hint("12345678-1234-1234-1234-123456789abc"), Some(hint));
        // A different account derives a different hint.
        assert_ne!(
            account_hint("12345678-1234-1234-1234-123456789abc"),
            account_hint("12345678-1234-1234-1234-000000000000")
        );
    }

    /// Short or oddly-shaped account ids get no attribution at all rather
    /// than a hint that exposes most of the identifier.
    #[test]
    fn account_hint_is_dropped_for_short_or_odd_ids() {
        assert_eq!(account_hint(""), None);
        assert_eq!(account_hint("abc-123"), None);
        assert_eq!(account_hint("0123456789012x "), None);
        // 15 chars is still too short; 16 is the guard boundary.
        assert_eq!(account_hint("123456789012345"), None);
        assert!(account_hint("1234567890123456").is_some());
        // Whitespace or punctuation in the tail is not identifier material.
        assert_eq!(account_hint("12345678901234 5x"), None);
        assert_eq!(account_hint("123456789012345.6"), None);
    }

    /// The identity string the failure paths stamp must match the identity
    /// the success attribution carries (the runtime guard compares them).
    #[test]
    fn identity_format_matches_the_success_attribution_input() {
        assert_eq!(
            codex_identity(Some("12345678-1234-1234-1234-123456789abc")),
            Some("chatgpt:56789abc".to_string())
        );
        // No account_id in auth.json: no identity anywhere — the runtime
        // keeps its conservative retention behavior.
        assert_eq!(codex_identity(None), None);
        assert_eq!(codex_identity(Some("abc-123")), None);
    }

    /// The usage wire carries only the masked hint — never the account id,
    /// the token, or any other credential material.
    #[test]
    fn usage_attribution_carries_only_the_masked_tail() {
        let usage = CodexUsage {
            limits: vec![CodexLimitWindow {
                label: "Weekly".to_string(),
                used_percent: 38.0,
                reset_at: Some("2026-10-03T07:15:21Z".to_string()),
            }],
            plan_type: Some("team".to_string()),
            account: Some(CodexAccount {
                account_hint: "56789abc".to_string(),
            }),
            reset_credits: None,
        };
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(wire.contains("\"account\":{\"accountHint\":\"56789abc\"}"), "wire: {wire}");
        for secret in [
            "12345678-1234-1234-1234-123456789abc",
            "access-token",
            "chatgpt:56789abc",
        ] {
            assert!(!wire.contains(secret), "leaked: {secret}");
        }
    }

    #[test]
    #[ignore = "live test: requires a real local Codex login and network access"]
    fn live_fetch_returns_windows() {
        let usage = tauri::async_runtime::block_on(fetch_codex_usage()).expect("live fetch");
        println!("plan_type: {:?}", usage.plan_type);
        if let Some(account) = &usage.account {
            println!("account: chatgpt:{}", account.account_hint);
        } else {
            println!("account: <none derivable from auth.json>");
        }
        for limit in &usage.limits {
            println!(
                "{}: used {:.1}% reset_at {:?}",
                limit.label,
                limit.used_percent,
                limit.reset_at.as_deref().unwrap_or("-")
            );
        }
        assert!(!usage.limits.is_empty(), "expected at least one window");
        // Read-only live evidence for the optional capability: counts only,
        // never raw payloads or identifiers.
        match &usage.reset_credits {
            Some(credits) => println!(
                "reset_credits: banked={} applicable={:?} checked_at={} source={}",
                credits.banked_credits,
                credits.currently_applicable,
                credits.checked_at,
                credits.source
            ),
            None => println!("reset_credits: unavailable"),
        }
    }

    // ---------- v0.7 banked reset credits (Codex-only) ----------

    fn credited_body(
        available_count: Value,
        applicable: Option<Value>,
        account_id: Option<&str>,
    ) -> Value {
        let mut summary = serde_json::json!({ "available_count": available_count });
        if let Some(applicable) = applicable {
            summary["applicable_available_count"] = applicable;
        }
        let mut body = serde_json::json!({
            "plan_type": "team",
            "rate_limit": { "primary_window": window_json(1.0) },
            "rate_limit_reset_credits": summary,
        });
        if let Some(account) = account_id {
            body["account_id"] = Value::String(account.to_string());
        }
        body
    }

    fn credited_usage(
        available_count: Value,
        applicable: Option<Value>,
        observed: Option<&str>,
        local: Option<&str>,
    ) -> CodexUsage {
        let body = credited_body(available_count, applicable, observed);
        parse_usage_response(body.to_string().as_bytes(), Utc::now(), local).unwrap()
    }

    #[test]
    fn explicit_banked_counts_survive_including_zero() {
        // An explicit zero is a known balance, not an unknown one.
        for count in [0, 1, 3] {
            let usage = credited_usage(
                Value::from(count),
                Some(Value::from(0)),
                Some("acct-1234567890abcd"),
                Some("acct-1234567890abcd"),
            );
            assert_eq!(usage.limits.len(), 1, "quota must stand with credits");
            let credits = usage.reset_credits.expect("explicit count must parse");
            assert_eq!(credits.banked_credits, count);
            assert_eq!(credits.currently_applicable, Some(0));
            assert_eq!(credits.source, RESET_CREDITS_SOURCE);
            assert_eq!(
                usage.account,
                Some(CodexAccount { account_hint: "7890abcd".to_string() })
            );
        }
    }

    #[test]
    fn applicability_zero_one_and_missing() {
        let zero = credited_usage(Value::from(3), Some(Value::from(0)), Some("acct-1234567890abcd"), Some("acct-1234567890abcd"));
        assert_eq!(
            zero.reset_credits.expect("credits").currently_applicable,
            Some(0)
        );
        let one = credited_usage(Value::from(3), Some(Value::from(1)), Some("acct-1234567890abcd"), Some("acct-1234567890abcd"));
        assert_eq!(
            one.reset_credits.expect("credits").currently_applicable,
            Some(1)
        );
        // Missing applicability is unknown, never zero — the banked balance
        // itself still stands.
        let missing = credited_usage(Value::from(2), None, Some("acct-1234567890abcd"), Some("acct-1234567890abcd"));
        let credits = missing.reset_credits.expect("banked balance must stand");
        assert_eq!(credits.banked_credits, 2);
        assert_eq!(credits.currently_applicable, None);
    }

    #[test]
    fn malformed_applicability_reads_unknown_and_keeps_the_balance() {
        // A present-but-malformed applicable_available_count degrades to
        // unknown (never zero, never a fabricated count) while the explicitly
        // reported banked balance and the quota windows stand.
        let malformed = [
            Value::from(-1),
            Value::from("1"),
            Value::from(1.5),
            Value::from(4_294_967_296_u64),
            Value::from(true),
        ];
        for applicable in malformed {
            let usage = credited_usage(
                Value::from(3),
                Some(applicable.clone()),
                Some("acct-1234567890abcd"),
                Some("acct-1234567890abcd"),
            );
            assert_eq!(usage.limits.len(), 1, "quota must survive: {applicable}");
            let credits = usage
                .reset_credits
                .unwrap_or_else(|| panic!("banked balance must stand: {applicable}"));
            assert_eq!(credits.banked_credits, 3);
            assert_eq!(
                credits.currently_applicable, None,
                "malformed applicability must read unknown: {applicable}"
            );
        }
    }

    #[test]
    fn missing_banked_count_is_unavailable_while_quota_stays_live() {
        let now = Utc::now();
        let bodies = [
            // No summary at all.
            serde_json::json!({
                "account_id": "acct-1234567890abcd",
                "plan_type": "team",
                "rate_limit": { "primary_window": window_json(1.0) }
            }),
            // Null summary.
            serde_json::json!({
                "account_id": "acct-1234567890abcd",
                "plan_type": "team",
                "rate_limit": { "primary_window": window_json(1.0) },
                "rate_limit_reset_credits": null
            }),
            // Summary without a usable balance.
            serde_json::json!({
                "account_id": "acct-1234567890abcd",
                "plan_type": "team",
                "rate_limit": { "primary_window": window_json(1.0) },
                "rate_limit_reset_credits": { "applicable_available_count": 1 }
            }),
            // Wrongly-typed summary (schema drift): quota must not care.
            serde_json::json!({
                "account_id": "acct-1234567890abcd",
                "plan_type": "team",
                "rate_limit": { "primary_window": window_json(1.0) },
                "rate_limit_reset_credits": "drifted"
            }),
            serde_json::json!({
                "account_id": "acct-1234567890abcd",
                "plan_type": "team",
                "rate_limit": { "primary_window": window_json(1.0) },
                "rate_limit_reset_credits": [3]
            }),
        ];
        for body in bodies {
            let usage =
                parse_usage_response(body.to_string().as_bytes(), now, Some("acct-1234567890abcd")).unwrap();
            assert_eq!(usage.limits.len(), 1, "quota windows must survive");
            assert_eq!(usage.reset_credits, None, "body: {body}");
        }
    }

    #[test]
    fn malformed_counts_are_never_coerced_or_clamped() {
        let malformed = [
            Value::Null,
            Value::from("3"),
            Value::from(1.5),
            Value::from(3.0),
            Value::from(-1),
            Value::from(true),
            serde_json::json!({}),
            serde_json::json!([]),
            serde_json::json!(4_294_967_296_u64),
            serde_json::json!(u64::MAX),
        ];
        for count in malformed {
            let usage = credited_usage(count.clone(), None, Some("acct-1234567890abcd"), Some("acct-1234567890abcd"));
            assert_eq!(usage.limits.len(), 1, "quota must survive: {count}");
            assert_eq!(usage.reset_credits, None, "count: {count}");
        }
    }

    #[test]
    fn windows_resets_and_balances_are_never_counts() {
        // Two windows (two reset timestamps), a monetary-style balance, grant
        // history, and earned totals must never read as a banked balance.
        let body = serde_json::json!({
            "account_id": "acct-1234567890abcd",
            "plan_type": "team",
            "rate_limit": {
                "primary_window": window_json(10.0),
                "secondary_window": {
                    "used_percent": 20.0,
                    "limit_window_seconds": 300,
                    "reset_after_seconds": 120
                }
            },
            "credits": { "has_credits": true, "balance": 42 },
            "total_earned_count": 5,
            "rate_limit_reset_credits": { "credit_items": [{}, {}, {}] }
        });
        let usage =
            parse_usage_response(body.to_string().as_bytes(), Utc::now(), Some("acct-1234567890abcd")).unwrap();
        assert_eq!(usage.limits.len(), 2);
        assert_eq!(usage.reset_credits, None);
    }

    #[test]
    fn credit_detail_rows_never_override_the_explicit_balance() {
        // Detail lists may be capped: the explicit count wins, rows are stats.
        let body = serde_json::json!({
            "account_id": "acct-1234567890abcd",
            "plan_type": "team",
            "rate_limit": { "primary_window": window_json(1.0) },
            "rate_limit_reset_credits": { "available_count": 3, "credit_items": [{}, {}] }
        });
        let usage =
            parse_usage_response(body.to_string().as_bytes(), Utc::now(), Some("acct-1234567890abcd")).unwrap();
        assert_eq!(usage.reset_credits.expect("credits").banked_credits, 3);
    }

    #[test]
    fn account_mismatch_drops_credits_not_quota() {
        // Served account contradicts the stored scope: windows stay bound to
        // what was actually served, the balance goes unavailable.
        let usage = credited_usage(
            Value::from(3),
            Some(Value::from(0)),
            Some("acct-1234567890abcd"),
            Some("acct-0987654321fedc"),
        );
        assert_eq!(usage.limits.len(), 1);
        assert_eq!(usage.reset_credits, None);
        assert_eq!(
            usage.account,
            Some(CodexAccount { account_hint: "7890abcd".to_string() })
        );
    }

    #[test]
    fn unbound_observations_degrade_conservatively() {
        // Observed but no stored scope: the observation binds to its own
        // account and stands.
        let usage = credited_usage(Value::from(3), None, Some("acct-1234567890abcd"), None);
        assert_eq!(usage.reset_credits.expect("credits").banked_credits, 3);
        // No observed account despite a stored scope: unbound, unavailable —
        // quota still attributes to the requested scope.
        let usage = credited_usage(Value::from(3), None, None, Some("acct-1234567890abcd"));
        assert_eq!(usage.reset_credits, None);
        assert_eq!(
            usage.account,
            Some(CodexAccount { account_hint: "7890abcd".to_string() })
        );
        // Neither side proves an identity: unattributed, unavailable.
        let usage = credited_usage(Value::from(3), None, None, None);
        assert_eq!(usage.reset_credits, None);
        assert_eq!(usage.account, None);
        // Too short for the shared hint mask: unattributed, unavailable.
        let usage = credited_usage(Value::from(3), None, Some("ab"), None);
        assert_eq!(usage.reset_credits, None);
        assert_eq!(usage.account, None);
    }

    #[test]
    fn credit_binding_uses_the_shared_account_hint_mask() {
        // Combined-contract pin: credits bind with the same masked identity
        // vocabulary as attribution, failure stamps, and the last-good guard
        // — the account_hint tail, never a separate mask.
        let observed = "0123456789abcdef";
        let usage = credited_usage(Value::from(3), None, Some(observed), None);
        assert_eq!(
            usage.reset_credits.as_ref().expect("credits").banked_credits,
            3
        );
        assert_eq!(
            usage.account,
            Some(CodexAccount { account_hint: "89abcdef".to_string() })
        );
        // The raw id must never survive anywhere on the wire.
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(!wire.contains(observed), "raw id leaked: {wire}");
    }

    #[test]
    fn stale_and_future_observations_read_unavailable() {
        let now = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap();
        let stamp = |secs: i64| {
            (now + chrono::TimeDelta::seconds(secs)).to_rfc3339_opts(SecondsFormat::Secs, true)
        };
        assert!(reset_credits_fresh(&stamp(0), now));
        assert!(reset_credits_fresh(&stamp(-RESET_CREDITS_TTL_SECS), now));
        assert!(!reset_credits_fresh(&stamp(-RESET_CREDITS_TTL_SECS - 1), now));
        assert!(!reset_credits_fresh(&stamp(-3600), now));
        // Clock skew inside tolerance still reads fresh; beyond it does not.
        assert!(reset_credits_fresh(&stamp(240), now));
        assert!(!reset_credits_fresh(&stamp(600), now));
        assert!(!reset_credits_fresh("not-a-timestamp", now));
        assert!(!reset_credits_fresh("", now));
    }

    #[test]
    fn credit_wire_shape_is_redacted_and_stable() {
        let usage = credited_usage(
            Value::from(3),
            Some(Value::from(0)),
            Some("acct-1234567890abcd"),
            Some("acct-1234567890abcd"),
        );
        // Precondition: the raw identity must actually enter the parse
        // pipeline (binding as the masked hint) for the raw-id absence
        // below to mean anything.
        assert_eq!(
            usage.account,
            Some(CodexAccount { account_hint: "7890abcd".to_string() }),
            "local identity did not bind as the masked hint"
        );
        let credits = usage.reset_credits.expect("credits");
        let wire = serde_json::to_string(&credits).unwrap();
        let value: Value = serde_json::from_str(&wire).unwrap();
        // serde_json orders map keys alphabetically; compare as a set.
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["bankedCredits", "checkedAt", "currentlyApplicable", "source"]
        );
        assert!(!wire.contains("acct-1234567890abcd"), "raw identity leaked: {wire}");
    }
}
