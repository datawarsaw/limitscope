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

use crate::provider_error::ProviderError;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const USER_AGENT: &str = concat!("rate-limits/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CODEX_IDENTITY_PREFIX: &str = "chatgpt-v2:";
const CODEX_IDENTITY_DOMAIN: &[u8] = b"limitscope.codex.identity.v2";

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
#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexAccount {
    pub account_hint: String,
    /// Internal full account identity. The wire shape keeps only the masked
    /// display hint; history and last-good retention consume this digest.
    #[serde(skip)]
    pub(crate) identity: Option<String>,
}

impl std::fmt::Debug for CodexAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAccount")
            .field("account_hint", &self.account_hint)
            .field("identity", &self.identity.as_ref().map(|_| "<redacted>"))
            .finish()
    }
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
/// is used. The hint is display-only; the separate identity binds retention.
fn codex_account(
    observed_account: Option<&str>,
    local_account: Option<&str>,
    identity: Option<String>,
) -> Option<CodexAccount> {
    let raw = observed_account
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| local_account.map(str::trim).filter(|s| !s.is_empty()))?;
    account_hint(raw).map(|account_hint| CodexAccount {
        account_hint,
        identity,
    })
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
fn bind_credit_account(observed_account: Option<&str>, local_account: Option<&str>) -> bool {
    let Some(observed) = observed_account.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    if let Some(local) = local_account.map(str::trim).filter(|s| !s.is_empty()) {
        if observed != local {
            return false;
        }
    }
    account_hint(observed).is_some()
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
    if !bind_credit_account(observed_account, local_account) {
        return None;
    }
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
    identity: Option<String>,
}

// Manual impl so the token can never reach logs through Debug formatting.
impl std::fmt::Debug for CodexAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAuth")
            .field("access_token", &"<redacted>")
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "<redacted>"),
            )
            .field("identity", &self.identity.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

fn codex_dir() -> Result<std::path::PathBuf, ProviderError> {
    let home = std::env::home_dir().ok_or_else(|| {
        ProviderError::new(
            "codex_not_installed",
            "Could not locate the user home directory.",
        )
    })?;
    Ok(codex_home_dir(
        std::env::var("CODEX_HOME").ok().as_deref(),
        &home,
    ))
}

/// The Codex home one account reads its credential from.
///
/// An explicit account home wins: it is that account's own credential
/// reference, set by the operator who logged that account in with
/// `codex login` inside its own `CODEX_HOME`. Without one the process falls
/// back to the ambient `CODEX_HOME` and then to `~/.codex` — the single-login
/// behavior this adapter has always had.
///
/// A configured home that does not exist is an error, not a silent fallback:
/// falling back would read the primary account's credential under the second
/// account's name, which is exactly the identity mix-up the per-account
/// model exists to prevent.
pub fn resolve_codex_home(explicit: Option<&str>) -> Result<std::path::PathBuf, ProviderError> {
    if let Some(home) = explicit.map(str::trim).filter(|home| !home.is_empty()) {
        let path = std::path::PathBuf::from(home);
        if !path.is_dir() {
            return Err(ProviderError::new(
                "codex_home_missing",
                format!("The configured Codex home does not exist: {home}"),
            ));
        }
        return Ok(path);
    }
    codex_dir()
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
        ProviderError::new(
            "auth_unreadable",
            "Codex auth state (auth.json) is not valid JSON.",
        )
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
    let id_token = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty());
    let identity = codex_identity_from_tokens(access_token, id_token, account_id.as_deref());
    Ok(CodexAuth {
        access_token: access_token.to_string(),
        account_id,
        identity,
    })
}

/// Loads only the local login state and returns its collision-resistant
/// user/workspace identity. Missing, malformed, or contradictory claims fail
/// closed without exposing the credential or either identifier.
pub(crate) fn current_codex_identity(codex_home: Option<&str>) -> Option<String> {
    let dir = resolve_codex_home(codex_home).ok()?;
    load_auth(&dir).ok()?.identity
}

#[derive(Deserialize)]
struct JwtIdentityClaims {
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<OpenAiIdentityClaims>,
}

#[derive(Deserialize)]
struct OpenAiIdentityClaims {
    #[serde(default)]
    chatgpt_user_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

struct TokenIdentity {
    user: Option<String>,
    workspace: Option<String>,
}

fn nonempty_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Decodes the standard three-part JWT payload exactly as Codex does. The
/// signature is not checked locally; the usage endpoint remains the authority
/// for rejecting the credential.
fn jwt_payload<T: DeserializeOwned>(jwt: &str) -> Option<T> {
    let mut parts = jwt.split('.');
    let (header, payload, signature) = match (parts.next(), parts.next(), parts.next()) {
        (Some(header), Some(payload), Some(signature))
            if !header.is_empty() && !payload.is_empty() && !signature.is_empty() =>
        {
            (header, payload, signature)
        }
        _ => return None,
    };
    let _ = (header, signature);
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn token_identity(jwt: &str) -> Option<TokenIdentity> {
    let claims: JwtIdentityClaims = jwt_payload(jwt)?;
    let auth = claims.auth?;
    let primary = nonempty_id(auth.chatgpt_user_id.as_deref());
    let fallback = nonempty_id(auth.user_id.as_deref());
    let user = match (primary, fallback) {
        (Some(primary), Some(fallback)) if primary == fallback => Some(primary),
        (Some(primary), None) => Some(primary),
        (None, Some(fallback)) => Some(fallback),
        _ => None,
    };
    Some(TokenIdentity {
        user,
        workspace: nonempty_id(auth.chatgpt_account_id.as_deref()),
    })
}

/// SHA-256 over explicitly labelled, length-prefixed fields. Prefixing each
/// field prevents concatenation collisions; neither tokens nor email enter
/// the digest.
fn codex_identity(user: &str, workspace: &str) -> Option<String> {
    let user = nonempty_id(Some(user))?;
    let workspace = nonempty_id(Some(workspace))?;
    let mut digest = Sha256::new();
    for field in [
        CODEX_IDENTITY_DOMAIN,
        b"user".as_slice(),
        user.as_bytes(),
        b"workspace".as_slice(),
        workspace.as_bytes(),
    ] {
        digest.update((field.len() as u64).to_le_bytes());
        digest.update(field);
    }
    Some(format!("{CODEX_IDENTITY_PREFIX}{:x}", digest.finalize()))
}

fn codex_identity_from_tokens(
    access_token: &str,
    id_token: Option<&str>,
    account_id: Option<&str>,
) -> Option<String> {
    let id = token_identity(id_token?)?;
    let access = token_identity(access_token)?;
    let user = id.user?;
    if access.user.as_deref() != Some(user.as_str()) {
        return None;
    }
    let workspace = nonempty_id(account_id)?;
    for claimed_workspace in [id.workspace.as_deref(), access.workspace.as_deref()]
        .into_iter()
        .flatten()
    {
        if claimed_workspace != workspace {
            return None;
        }
    }
    codex_identity(&user, &workspace)
}

/// Expiry claim from the access token's JWT payload, when parseable.
/// Only the timestamp is used — the token itself stays in place.
fn jwt_expiry(token: &str) -> Option<i64> {
    let value: Value = jwt_payload(token)?;
    value.get("exp")?.as_i64()
}

/// Masked attempted-account hint from `tokens.account_id` — the stable
/// account identifier the usage request is already scoped with (as the
/// `chatgpt-account-id` header). Only its last eight characters are used, so
/// the hint distinguishes account A from account B without exposing the
/// identifier. No email or token material enters the hint. `None` when the id is too short to mask
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
#[cfg(test)]
fn parse_usage_response(
    body: &[u8],
    now: DateTime<Utc>,
    local_account: Option<&str>,
) -> Result<CodexUsage, ProviderError> {
    parse_usage_response_with_identity(body, now, local_account, None)
}

fn parse_usage_response_with_identity(
    body: &[u8],
    now: DateTime<Utc>,
    local_account: Option<&str>,
    identity: Option<String>,
) -> Result<CodexUsage, ProviderError> {
    let parsed: UsageResponse = serde_json::from_slice(body)
        .map_err(|_| unexpected_usage_response("no known windows found."))?;
    let observed_account = parsed.account_id.as_deref();
    let local_account = local_account.map(str::trim).filter(|id| !id.is_empty());
    if let (Some(observed), Some(local)) = (observed_account, local_account) {
        if observed.trim() != local {
            return Err(ProviderError::new(
                "unexpected_response",
                "Codex usage response describes a different workspace than the local login.",
            ));
        }
    }
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
    Ok(CodexUsage {
        limits,
        plan_type: parsed.plan_type,
        // Windows name the served account when present; otherwise the
        // requested local scope. The internal digest always comes from the
        // locally read login claims, so success and failure share one identity.
        account: codex_account(observed_account, local_account, identity),
        reset_credits: parse_reset_credits(
            parsed.rate_limit_reset_credits.as_ref(),
            observed_account,
            local_account,
            now,
        ),
    })
}

#[cfg(test)]
pub(crate) fn fixture_collect(auth_json: &str, body: &[u8], now: DateTime<Utc>) -> Result<CodexUsage, ProviderError> {
    let auth = parse_auth(auth_json)?;
    parse_usage_response_with_identity(body, now, auth.account_id.as_deref(), auth.identity.clone())
        .map_err(|error| error.with_identity_hint(auth.identity))
}

// ---------- fetch + command ----------

async fn fetch_codex_usage(codex_home: Option<&str>) -> Result<CodexUsage, ProviderError> {
    let dir = resolve_codex_home(codex_home)?;
    let auth = load_auth(&dir)?;
    // Final-boundary value scrub (`secret_scrub`): the access token is alive
    // for this whole fetch, so any exact occurrence of it in a failure
    // message is removed before the error leaves the adapter.
    fetch_usage_for_auth(&auth).await.map_err(|error| {
        crate::secret_scrub::scrub_provider_error(error, &[auth.access_token.as_str()])
    })
}

/// The usage fetch for an already-resolved credential. No token refresh: an
/// expired session fails here and stays failed until Codex itself re-auths.
async fn fetch_usage_for_auth(auth: &CodexAuth) -> Result<CodexUsage, ProviderError> {
    let identity = auth.identity.clone();
    // authoritative check (401/403 below).
    if let Some(exp) = jwt_expiry(&auth.access_token) {
        if Utc::now().timestamp() >= exp {
            return Err(auth_expired().with_identity_hint(identity.clone()));
        }
    }

    let client = http_client().map_err(|error| error.with_identity_hint(identity.clone()))?;
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
    let retry_after =
        crate::provider_error::retry_after_header(response.headers()).map(String::from);
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

    // A served-workspace contradiction is a failure of this same attempt, so
    // it carries the local identity; success stores that exact identity.
    parse_usage_response_with_identity(
        &body,
        Utc::now(),
        auth.account_id.as_deref(),
        identity.clone(),
    )
    .map_err(|error| error.with_identity_hint(identity.clone()))
}

#[tauri::command]
pub async fn get_codex_usage() -> Result<CodexUsage, ProviderError> {
    fetch_codex_usage(None).await
}

/// The usage read for one explicitly configured account.
///
/// The account's own Codex home is the only thing that differs from
/// `get_codex_usage`: the credential, the request scope and therefore the
/// quota windows are that account's own. Nothing is shared between two
/// accounts and no limit is ever combined.
pub async fn get_codex_usage_for(codex_home: Option<&str>) -> Result<CodexUsage, ProviderError> {
    fetch_codex_usage(codex_home).await
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
        let over =
            normalize_window(serde_json::from_value(window_json(150.0)).unwrap(), now).unwrap();
        assert_eq!(over.used_percent, 100.0);
        assert_eq!(over.label, "Weekly");
        assert_eq!(over.reset_at.as_deref(), Some("2026-10-03T07:15:21Z"));

        let under =
            normalize_window(serde_json::from_value(window_json(-5.0)).unwrap(), now).unwrap();
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
        assert_eq!(
            usage.limits[0].reset_at.as_deref(),
            Some("2026-10-03T07:15:21Z")
        );
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
        let error =
            parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error.message.contains("no rate-limit payload found."));
    }

    #[test]
    fn null_rate_limit_is_a_schema_error() {
        let body = serde_json::json!({ "plan_type": "free", "rate_limit": null });
        let error =
            parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error.message.contains("no rate-limit payload found."));
    }

    #[test]
    fn missing_primary_window_is_a_schema_error() {
        let body = serde_json::json!({
            "plan_type": "free",
            "rate_limit": { "allowed": true, "secondary_window": window_json(2.0) }
        });
        let error =
            parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error
            .message
            .contains("no primary rate-limit window found."));
    }

    #[test]
    fn malformed_primary_window_is_a_schema_error() {
        // Present but without a usable usage figure: no invented windows.
        let body = serde_json::json!({
            "plan_type": "free",
            "rate_limit": { "primary_window": { "limit_window_seconds": 300 } }
        });
        let error =
            parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(error.message.contains("no usable usage figure."));

        // Null primary window reads the same way.
        let body = serde_json::json!({
            "plan_type": "free",
            "rate_limit": { "primary_window": null }
        });
        let error =
            parse_usage_response(body.to_string().as_bytes(), Utc::now(), None).unwrap_err();
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
        assert_eq!(
            ProviderError::transient("network", "offline").transient,
            Some(true)
        );
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
        for secret in [
            "sk-secret-token",
            "acct-1234567890abcdef",
            "someone@example.com",
            "user-x",
        ] {
            assert!(!wire.contains(secret), "leaked: {secret}");
        }
        // The served account binds as a masked hint only; no credit summary
        // was reported, so the capability stays unavailable.
        assert_eq!(
            usage.account,
            Some(CodexAccount {
                account_hint: "90abcdef".to_string(),
                identity: None
            })
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
        assert_eq!(
            parse_auth(r#"{"OPENAI_API_KEY":"sk-x"}"#).unwrap_err().code,
            "not_logged_in"
        );
        assert_eq!(
            parse_auth(r#"{"tokens":{}}"#).unwrap_err().code,
            "not_logged_in"
        );
        assert_eq!(parse_auth("not json").unwrap_err().code, "auth_unreadable");
    }

    fn jwt_with_auth_claims(auth: Value) -> String {
        jwt_with_payload(serde_json::json!({ "https://api.openai.com/auth": auth }))
    }

    fn jwt_with_payload(payload: Value) -> String {
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("fixture-header.{payload}.fixture-signature")
    }

    fn auth_json(access_auth: Value, id_auth: Value, account_id: Option<&str>) -> String {
        let mut tokens = serde_json::json!({
            "access_token": jwt_with_auth_claims(access_auth),
            "id_token": jwt_with_auth_claims(id_auth),
        });
        if let Some(account_id) = account_id {
            tokens["account_id"] = Value::String(account_id.to_string());
        }
        serde_json::json!({ "tokens": tokens }).to_string()
    }

    #[test]
    fn identity_is_a_domain_separated_sha256_of_user_and_workspace() {
        let identity = codex_identity("fixture-user-a", "fixture-workspace-a").unwrap();
        assert!(identity.starts_with(CODEX_IDENTITY_PREFIX));
        let hex = identity.strip_prefix(CODEX_IDENTITY_PREFIX).unwrap();
        assert_eq!(hex.len(), 64);
        assert!(hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        assert_eq!(
            identity,
            codex_identity("fixture-user-a", "fixture-workspace-a").unwrap()
        );
        assert_ne!(
            identity,
            codex_identity("fixture-user-b", "fixture-workspace-a").unwrap()
        );
        assert_ne!(
            identity,
            codex_identity("fixture-user-a", "fixture-workspace-b").unwrap()
        );
        assert_eq!(codex_identity("", "fixture-workspace-a"), None);
        assert_eq!(codex_identity("fixture-user-a", " "), None);
    }

    #[test]
    fn reconnect_keeps_identity_while_both_tokens_rotate() {
        let first = parse_auth(&auth_json(
            serde_json::json!({ "chatgpt_user_id": "fixture-user-a" }),
            serde_json::json!({ "user_id": "fixture-user-a" }),
            Some("fixture-workspace-a"),
        ))
        .unwrap();
        let reconnected = parse_auth(&auth_json(
            serde_json::json!({ "user_id": "fixture-user-a" }),
            serde_json::json!({ "chatgpt_user_id": "fixture-user-a" }),
            Some("fixture-workspace-a"),
        ))
        .unwrap();
        assert!(first.identity.is_some());
        assert_eq!(first.identity, reconnected.identity);
        assert_ne!(first.access_token, reconnected.access_token);
    }

    #[test]
    fn identity_changes_for_a_different_user_or_workspace() {
        let base = parse_auth(&auth_json(
            serde_json::json!({ "chatgpt_user_id": "fixture-user-a" }),
            serde_json::json!({ "chatgpt_user_id": "fixture-user-a" }),
            Some("fixture-workspace-a"),
        ))
        .unwrap()
        .identity;
        let other_user = parse_auth(&auth_json(
            serde_json::json!({ "chatgpt_user_id": "fixture-user-b" }),
            serde_json::json!({ "chatgpt_user_id": "fixture-user-b" }),
            Some("fixture-workspace-a"),
        ))
        .unwrap()
        .identity;
        let other_workspace = parse_auth(&auth_json(
            serde_json::json!({ "chatgpt_user_id": "fixture-user-a" }),
            serde_json::json!({ "chatgpt_user_id": "fixture-user-a" }),
            Some("fixture-workspace-b"),
        ))
        .unwrap()
        .identity;
        assert!(base.is_some());
        assert_ne!(base, other_user);
        assert_ne!(base, other_workspace);
    }

    #[test]
    fn absent_or_contradictory_identity_claims_fail_closed() {
        let claims = |user: &str| serde_json::json!({ "chatgpt_user_id": user });
        let valid = auth_json(
            claims("fixture-user-a"),
            claims("fixture-user-a"),
            Some("fixture-workspace-a"),
        );
        let mut without_id_token: Value = serde_json::from_str(&valid).unwrap();
        without_id_token["tokens"]
            .as_object_mut()
            .unwrap()
            .remove("id_token");
        let missing_id = parse_auth(&without_id_token.to_string()).unwrap();
        let missing_user = parse_auth(&auth_json(
            serde_json::json!({}),
            claims("fixture-user-a"),
            Some("fixture-workspace-a"),
        ))
        .unwrap();
        let user_mismatch = parse_auth(&auth_json(
            claims("fixture-user-b"),
            claims("fixture-user-a"),
            Some("fixture-workspace-a"),
        ))
        .unwrap();
        let missing_workspace = parse_auth(&auth_json(
            claims("fixture-user-a"),
            claims("fixture-user-a"),
            None,
        ))
        .unwrap();
        let workspace_mismatch = parse_auth(&auth_json(
            serde_json::json!({ "chatgpt_user_id": "fixture-user-a", "chatgpt_account_id": "fixture-workspace-b" }),
            claims("fixture-user-a"),
            Some("fixture-workspace-a"),
        ))
        .unwrap();
        let malformed = parse_auth(&serde_json::json!({
            "tokens": { "access_token": "not-a-jwt", "id_token": "also-not-a-jwt", "account_id": "fixture-workspace-a" }
        }).to_string()).unwrap();
        for auth in [
            missing_id,
            missing_user,
            user_mismatch,
            missing_workspace,
            workspace_mismatch,
            malformed,
        ] {
            assert_eq!(auth.identity, None);
        }
    }

    #[test]
    fn current_codex_identity_reads_only_the_local_auth_file() {
        let dir = unique_temp_dir("identity");
        fs::write(
            dir.join("auth.json"),
            auth_json(
                serde_json::json!({ "chatgpt_user_id": "fixture-user-a" }),
                serde_json::json!({ "chatgpt_user_id": "fixture-user-a" }),
                Some("fixture-workspace-a"),
            ),
        )
        .unwrap();
        assert_eq!(
            current_codex_identity(dir.to_str()),
            codex_identity("fixture-user-a", "fixture-workspace-a")
        );
        fs::write(dir.join("auth.json"), "not json").unwrap();
        assert_eq!(current_codex_identity(dir.to_str()), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn auth_debug_and_identity_never_expose_auth_material() {
        let auth = parse_auth(&auth_json(
            serde_json::json!({ "chatgpt_user_id": "fixture-private-user" }),
            serde_json::json!({ "chatgpt_user_id": "fixture-private-user" }),
            Some("fixture-private-workspace"),
        ))
        .unwrap();
        let debug = format!("{auth:?}");
        assert!(!debug.contains("fixture-private-user"));
        assert!(!debug.contains("fixture-private-workspace"));
        assert!(!debug.contains(&auth.access_token));
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "limitscope-codex-test-{}-{label}",
            std::process::id()
        ));
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
        assert_eq!(
            codex_home_dir(Some(missing.to_str().unwrap()), home),
            fallback
        );
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
        let missing =
            std::env::temp_dir().join("limitscope-codex-test-missing-home-does-not-exist");
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
        assert_eq!(
            account_hint("12345678-1234-1234-1234-123456789abc"),
            Some(hint)
        );
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
                identity: None,
            }),
            reset_credits: None,
        };
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(
            wire.contains("\"account\":{\"accountHint\":\"56789abc\"}"),
            "wire: {wire}"
        );
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
        let usage = tauri::async_runtime::block_on(fetch_codex_usage(None)).expect("live fetch");
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
                Some(CodexAccount {
                    account_hint: "7890abcd".to_string(),
                    identity: None
                })
            );
        }
    }

    #[test]
    fn applicability_zero_one_and_missing() {
        let zero = credited_usage(
            Value::from(3),
            Some(Value::from(0)),
            Some("acct-1234567890abcd"),
            Some("acct-1234567890abcd"),
        );
        assert_eq!(
            zero.reset_credits.expect("credits").currently_applicable,
            Some(0)
        );
        let one = credited_usage(
            Value::from(3),
            Some(Value::from(1)),
            Some("acct-1234567890abcd"),
            Some("acct-1234567890abcd"),
        );
        assert_eq!(
            one.reset_credits.expect("credits").currently_applicable,
            Some(1)
        );
        // Missing applicability is unknown, never zero — the banked balance
        // itself still stands.
        let missing = credited_usage(
            Value::from(2),
            None,
            Some("acct-1234567890abcd"),
            Some("acct-1234567890abcd"),
        );
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
            let usage = parse_usage_response(
                body.to_string().as_bytes(),
                now,
                Some("acct-1234567890abcd"),
            )
            .unwrap();
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
            let usage = credited_usage(
                count.clone(),
                None,
                Some("acct-1234567890abcd"),
                Some("acct-1234567890abcd"),
            );
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
        let usage = parse_usage_response(
            body.to_string().as_bytes(),
            Utc::now(),
            Some("acct-1234567890abcd"),
        )
        .unwrap();
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
        let usage = parse_usage_response(
            body.to_string().as_bytes(),
            Utc::now(),
            Some("acct-1234567890abcd"),
        )
        .unwrap();
        assert_eq!(usage.reset_credits.expect("credits").banked_credits, 3);
    }

    #[test]
    fn served_workspace_mismatch_rejects_the_whole_reading() {
        let body = credited_body(
            Value::from(3),
            Some(Value::from(0)),
            Some("acct-1234567890abcd"),
        );
        let error = parse_usage_response(
            body.to_string().as_bytes(),
            Utc::now(),
            Some("acct-0987654321fedc"),
        )
        .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert!(!error.message.contains("acct-"));
    }

    #[tokio::test]
    async fn success_and_post_resolution_failure_share_the_exact_identity() {
        let identity = codex_identity("fixture-user-a", "acct-1234567890abcd").unwrap();
        let auth = CodexAuth {
            access_token: jwt_with_payload(serde_json::json!({
                "exp": 1,
                "https://api.openai.com/auth": { "chatgpt_user_id": "fixture-user-a" }
            })),
            account_id: Some("acct-1234567890abcd".to_string()),
            identity: Some(identity.clone()),
        };
        let failure = fetch_usage_for_auth(&auth).await.unwrap_err();
        assert_eq!(failure.code, "auth_expired");
        assert_eq!(failure.identity_hint.as_deref(), Some(identity.as_str()));

        let body = serde_json::json!({
            "account_id": "acct-1234567890abcd",
            "plan_type": "team",
            "rate_limit": { "primary_window": window_json(1.0) }
        });
        let success = parse_usage_response_with_identity(
            body.to_string().as_bytes(),
            Utc::now(),
            auth.account_id.as_deref(),
            Some(identity.clone()),
        )
        .unwrap();
        assert_eq!(
            success
                .account
                .as_ref()
                .and_then(|account| account.identity.as_deref()),
            Some(identity.as_str())
        );
        assert_eq!(failure.identity_hint, Some(identity));
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
            Some(CodexAccount {
                account_hint: "7890abcd".to_string(),
                identity: None
            })
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
            usage
                .reset_credits
                .as_ref()
                .expect("credits")
                .banked_credits,
            3
        );
        assert_eq!(
            usage.account,
            Some(CodexAccount {
                account_hint: "89abcdef".to_string(),
                identity: None
            })
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
        assert!(!reset_credits_fresh(
            &stamp(-RESET_CREDITS_TTL_SECS - 1),
            now
        ));
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
            Some(CodexAccount {
                account_hint: "7890abcd".to_string(),
                identity: None
            }),
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
            vec![
                "bankedCredits",
                "checkedAt",
                "currentlyApplicable",
                "source"
            ]
        );
        assert!(
            !wire.contains("acct-1234567890abcd"),
            "raw identity leaked: {wire}"
        );
    }
}
