//! Read-only MiniMax subscription (Coding Plan) quota.
//!
//! The verified source is the official MiniMax subscription-quota endpoint
//! GET https://www.minimax.io/v1/token_plan/remains, authenticated with
//! 'Authorization: Bearer <Subscription Key>' - the 'sk-cp-...' plan key the
//! MiniMax open platform mints for a Coding Plan. The same route is served by
//! the international host https://api.minimax.io and by the mainland host
//! https://www.minimaxi.com.
//!
//! Live-verified 2026-10-22 against the user's own plan:
//!
//!     {
//!       "model_remains": [
//!         {
//!           "model_name": "general",
//!           "start_time": 1792634466447,
//!           "end_time": 1792652466447,
//!           "remains_time": 17999988,
//!           "current_interval_status": 1,
//!           "current_interval_remaining_percent": 100,
//!           "weekly_start_time": 1792634466447,
//!           "weekly_end_time": 1793239266447,
//!           "weekly_remains_time": 604799988,
//!           "current_weekly_status": 1,
//!           "current_weekly_remaining_percent": 100
//!         }
//!       ],
//!       "base_resp": { "status_code": 0, "status_msg": "success" }
//!     }
//!
//! Semantics proven by that reading, not assumed:
//!
//! - The quota is reported as REMAINING percent, not used percent. The
//!   shared wire contract is 'used_percent', so the adapter converts
//!   explicitly: used = 100 - remaining.
//! - 'end_time' / 'weekly_end_time' are ABSOLUTE epoch milliseconds and are
//!   the window boundaries. The measured interval span was exactly
//!   18 000 000 ms (5 h) and the weekly span exactly 604 800 000 ms (7 d).
//! - 'remains_time' / 'weekly_remains_time' are RELATIVE seconds (18000 s
//!   and 604800 s) - a redundant countdown to the same boundary, never an
//!   epoch stamp. They are ignored; the absolute 'end_time' is the only
//!   timestamp used.
//! - The windows are rolling: both 'start_time' values tracked the moment of
//!   the observation, so 'end_time' is the truthful reset boundary.
//! - 'model_name: "general"' is the SHARED text quota pool. Media and
//!   per-model entries (for example 'video') are separate pools and are
//!   never summed into the shared pool: the adapter reports one pool only,
//!   so a single subscription is never counted twice.
//! - 'current_interval_status' / 'current_weekly_status' equal 3 mean the
//!   window is unlimited; 1 is a normal metered window.
//! - A missing or refused subscription arrives as HTTP 200 with
//!   'base_resp.status_code' 1004 (no API secret key) or 1003 (not a member
//!   of the group). Both are deterministic and never retried.
//!
//! Credential resolution (read-only, this process only):
//!
//! 1. MINIMAX_SUBSCRIPTION_KEY - an explicit subscription key override.
//! 2. The MiniMax Code CLI's managed-login store
//!    (~/.minimax/auth/prod/<region>/mcode-public/auth.json). That store
//!    holds an OAuth ACCESS token, not the plan key; the access token is
//!    exchanged read-only at
//!    GET https://platform.minimax.io/backend/account/token_plan_credit,
//!    whose 'api_key' field carries the account's own 'sk-cp-...' plan key.
//!
//! The store's OAuth REFRESH token is deliberately never used. MiniMax
//! rotates the refresh token on every use: presenting it invalidates the
//! copy the MiniMax Code CLI holds, which signs the user out of their own
//! desktop app. A refresh is the CLI's business, so this adapter only reads
//! the access token the CLI has already persisted. When that access token has
//! expired the adapter reports a deterministic 'not signed in' state and asks
//! for a sign-in instead of taking the session over.
//!
//! Nothing beyond normalized windows and a masked key hint reaches the
//! WebView: no credential is logged, persisted, or printed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider_error::ProviderError;

/// The officially documented subscription-quota route (international).
const QUOTA_URL: &str = "https://www.minimax.io/v1/token_plan/remains";
/// The mainland-China host serving the same route.
const QUOTA_URL_CN: &str = "https://www.minimaxi.com/v1/token_plan/remains";
/// The managed-login account route that hands out the account's own
/// subscription key. Read-only.
const CREDIT_URL: &str = "https://platform.minimax.io/backend/account/token_plan_credit";
/// The mainland-China counterpart of the account route.
const CREDIT_URL_CN: &str = "https://www.minimaxi.com/backend/account/token_plan_credit";

const USER_AGENT: &str = concat!("rate-limits/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The MiniMax Code CLI's credential root.
const MINIMAX_DIR: &str = ".minimax";
/// Explicit subscription-key override.
const SUBSCRIPTION_KEY_ENV: &str = "MINIMAX_SUBSCRIPTION_KEY";

/// Dashboard label for the five-hour pool. Matches the vocabulary the Codex,
/// OpenCode Go and Z.ai adapters already use for the same window.
const FIVE_HOUR_LABEL: &str = "5-hour";
/// Dashboard label for the weekly pool.
const WEEKLY_LABEL: &str = "Weekly";

/// Plausibility bounds for epoch-millisecond timestamps (2100-01-01 UTC as
/// the upper bound; smaller magnitudes are epoch seconds and are rejected
/// rather than promoted).
const MIN_RESET_MS: i64 = 100_000_000_000;
const MAX_RESET_MS: i64 = 4_102_444_800_000;

/// The shared text-quota pool. MiniMax reports the plan's text allowance
/// under this model name; media pools and model-specific pools are separate
/// allowances and are never merged into this one.
const SHARED_POOL_MODEL: &str = "general";

/// Upstream window status meaning "unlimited": the window is not metered at
/// all, so it has no meaningful used percentage.
const STATUS_UNLIMITED: i64 = 3;

/// 'base_resp.status_code' values that mean the request was refused for an
/// account or entitlement reason rather than a transport problem. Both are
/// deterministic and are never retried.
const BASE_RESP_UNAUTHORIZED: i64 = 1004;
const BASE_RESP_NOT_A_MEMBER: i64 = 1003;
const BASE_RESP_PERMISSION_DENIED: i64 = 2029;

/// The subscription-key prefix the open platform mints for a Coding Plan. A
/// pay-as-you-go API key shares the 'sk-' prefix but not the 'sk-cp-' plan
/// prefix, so the two credential classes never get confused.
const SUBSCRIPTION_KEY_PREFIX: &str = "sk-cp-";

// ---------- data returned to the WebView (camelCase on the wire) ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MiniMaxLimitWindow {
    pub label: String,
    /// USED percent in 0-100. MiniMax reports the remaining percent; the
    /// conversion happens in the adapter so the wire contract stays uniform
    /// across every provider.
    pub used_percent: f64,
    /// RFC 3339 UTC timestamp derived from the upstream absolute
    /// 'end_time' / 'weekly_end_time' epoch-millisecond stamp.
    pub reset_at: Option<String>,
}

/// Masked attribution of the windows to the one account whose subscription
/// key resolved. 'key_hint' is the last four characters of the plan key - the
/// same masking convention the Z.ai and OpenCode Go cards use.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MiniMaxAccount {
    pub key_hint: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MiniMaxUsage {
    pub limits: Vec<MiniMaxLimitWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<MiniMaxAccount>,
}

// ---------- structured provider errors ----------

fn credential_missing() -> ProviderError {
    ProviderError::new(
        "credential_missing",
        "No MiniMax subscription key found. Sign in to the MiniMax Code CLI with a Coding Plan, or set MINIMAX_SUBSCRIPTION_KEY.",
    )
}

fn not_signed_in() -> ProviderError {
    ProviderError::new(
        "auth_expired",
        "The MiniMax Code CLI session has expired. Sign in to the MiniMax Code CLI again, then refresh.",
    )
}

fn auth_invalid() -> ProviderError {
    ProviderError::new(
        "auth_invalid",
        "The MiniMax subscription key was rejected. Sign in to the MiniMax Code CLI again, then refresh.",
    )
}

fn not_entitled() -> ProviderError {
    ProviderError::new(
        "not_entitled",
        "This MiniMax account has no active subscription, so the quota endpoint refused the request.",
    )
}

fn minimax_not_installed() -> ProviderError {
    ProviderError::new(
        "minimax_not_installed",
        "MiniMax Code is not installed (no MiniMax credential directory found).",
    )
}

fn unexpected(message: &str) -> ProviderError {
    ProviderError::new("unexpected_response", message)
}

// ---------- upstream response (only the fields we consume) ----------

#[derive(Debug, Deserialize)]
struct QuotaResponse {
    #[serde(default)]
    model_remains: Option<Vec<ModelRemains>>,
    #[serde(default)]
    base_resp: Option<BaseResp>,
}

#[derive(Debug, Deserialize)]
struct BaseResp {
    #[serde(default)]
    status_code: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ModelRemains {
    #[serde(default)]
    model_name: Option<String>,
    /// Absolute window end, epoch milliseconds.
    #[serde(default)]
    end_time: Option<i64>,
    /// Absolute weekly window end, epoch milliseconds.
    #[serde(default)]
    weekly_end_time: Option<i64>,
    #[serde(default)]
    current_interval_status: Option<i64>,
    /// REMAINING percent of the five-hour pool, 0-100 (numeric dialect).
    #[serde(default)]
    current_interval_remaining_percent: Option<f64>,
    /// USED percent of the five-hour pool as a percent string (the
    /// 'remains_percent' dialect speaks this instead).
    #[serde(default)]
    current_interval_used_percent: Option<String>,
    #[serde(default)]
    current_weekly_status: Option<i64>,
    /// REMAINING percent of the weekly pool, 0-100 (numeric dialect).
    #[serde(default)]
    current_weekly_remaining_percent: Option<f64>,
    /// USED percent of the weekly pool as a percent string.
    #[serde(default)]
    current_weekly_used_percent: Option<String>,
}

/// The managed-login account route. Its 'api_key' field carries the
/// account's own subscription key. The manual Debug keeps the key out of
/// every log that formats the parsed response.
#[derive(Deserialize)]
struct CreditResponse {
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    base_resp: Option<BaseResp>,
}

impl std::fmt::Debug for CreditResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreditResponse")
            .field("api_key", &"<redacted>")
            .field("base_resp", &self.base_resp)
            .finish()
    }
}

/// The MiniMax Code CLI's OAuth credential store. Only the fields the
/// adapter needs are read; the token values never leave this process.
#[derive(Debug, Deserialize)]
struct AuthStore {
    #[serde(default)]
    records: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRecord {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_at_ms: Option<i64>,
}

impl std::fmt::Debug for AuthRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRecord")
            .field("access_token", &"<redacted>")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

// ---------- credential resolution (read-only) ----------

/// Manual 'Debug' so a plan key can never reach a log through formatting.
struct SubscriptionKey(String);

impl std::fmt::Debug for SubscriptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SubscriptionKey").field(&"<redacted>").finish()
    }
}

/// Masked hint: the last four characters of the plan key. Deterministic per
/// credential, so the card and the failure attribution stay stable. 'None'
/// when the key is too short to mask meaningfully.
fn account_hint(key: &str) -> Option<String> {
    let tail = key.chars().rev().take(4).collect::<Vec<_>>();
    if key.chars().count() < 12 {
        return None;
    }
    if !tail
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
    {
        return None;
    }
    Some(tail.into_iter().rev().collect())
}

fn identity(key: &str) -> Option<String> {
    account_hint(key).map(|hint| format!("key:{hint}"))
}

/// The MiniMax Code CLI's credential root: '~/.minimax'.
pub(crate) fn minimax_dir(home: &Path) -> PathBuf {
    home.join(MINIMAX_DIR)
}

/// The regional hosts. The CLI's credential directory names the region the
/// account signed into, and the open platform serves each region from its
/// own host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    International,
    Mainland,
}

impl Region {
    fn quota_url(self) -> &'static str {
        match self {
            Region::International => QUOTA_URL,
            Region::Mainland => QUOTA_URL_CN,
        }
    }

    fn credit_url(self) -> &'static str {
        match self {
            Region::International => CREDIT_URL,
            Region::Mainland => CREDIT_URL_CN,
        }
    }
}

/// The CLI stores its credential under the region and build environment it
/// logged into. The international production path is probed first, then the
/// mainland one; the first readable store wins.
fn auth_store_path(dir: &Path) -> Option<(PathBuf, Region)> {
    let candidates = [
        (dir.join("auth/prod/en/mcode-public/auth.json"), Region::International),
        (dir.join("auth/prod/cn/mcode-public/auth.json"), Region::Mainland),
    ];
    candidates
        .into_iter()
        .find(|(path, _)| path.is_file())
}

/// Reads the CLI's stored OAuth access token. The refresh token is never
/// read: MiniMax rotates it on use, and consuming it would sign the user
/// out of their own desktop app.
fn load_managed_access_token() -> Result<(String, Region), ProviderError> {
    let home = std::env::home_dir().ok_or_else(minimax_not_installed)?;
    let dir = minimax_dir(&home);
    let Some((path, region)) = auth_store_path(&dir) else {
        return Err(credential_missing());
    };
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(credential_missing())
        }
        Err(error) => {
            return Err(ProviderError::new(
                "auth_unreadable",
                format!("Could not read the MiniMax credential store: {error}"),
            ))
        }
    };
    let record = parse_auth_store(&raw)?;
    let token = record
        .access_token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .ok_or_else(credential_missing)?;
    // A token that expires within the next minute is treated as expired, so
    // the request never races the boundary.
    if let Some(expires_at) = record.expires_at_ms {
        if expires_at <= Utc::now().timestamp_millis() + 60_000 {
            return Err(not_signed_in());
        }
    }
    Ok((token, region))
}

fn parse_auth_store(raw: &str) -> Result<AuthRecord, ProviderError> {
    let store: AuthStore = serde_json::from_str(raw).map_err(|_| {
        ProviderError::new(
            "auth_unreadable",
            "MiniMax credential store (auth.json) is not valid JSON.",
        )
    })?;
    let Some(records) = store.records.and_then(|value| value.as_object().cloned()) else {
        return Err(credential_missing());
    };
    // The store keys records by a composite account id. Any record that
    // carries a usable access token is acceptable; the first one in the
    // store's own order is taken so the choice is deterministic.
    for value in records.values() {
        if !value.is_object() {
            continue;
        }
        let Ok(record) = serde_json::from_value::<AuthRecord>(value.clone()) else {
            continue;
        };
        if record
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .is_some()
        {
            return Ok(record);
        }
    }
    Err(credential_missing())
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

/// One authenticated GET against the official subscription-quota route.
async fn fetch_quota(
    client: &reqwest::Client,
    key: &SubscriptionKey,
    region: Region,
) -> Result<Vec<MiniMaxLimitWindow>, ProviderError> {
    let response = client
        .get(region.quota_url())
        .bearer_auth(&key.0)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|error| {
            crate::provider_error::transport_failure(
                "network",
                format!("Could not reach the MiniMax quota endpoint: {error}"),
                &error,
            )
        })?;
    let status = response.status();
    let retry_after = crate::provider_error::retry_after_header(response.headers()).map(String::from);
    let body = response.bytes().await.map_err(|error| {
        crate::provider_error::transport_failure(
            "network",
            format!("MiniMax quota response was cut short: {error}"),
            &error,
        )
    })?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(auth_invalid());
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(not_entitled());
    }
    if !status.is_success() {
        return Err(ProviderError::http_failure_with_retry_after(
            status,
            format!("MiniMax quota endpoint returned HTTP {status}."),
            retry_after.as_deref(),
            Utc::now(),
        ));
    }
    parse_quota(&body)
}

/// The managed-login exchange that yields the account's own subscription key.
async fn fetch_subscription_key(
    access_token: &str,
    region: Region,
) -> Result<SubscriptionKey, ProviderError> {
    let client = http_client()?;
    let response = client
        .get(region.credit_url())
        .bearer_auth(access_token)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|error| {
            crate::provider_error::transport_failure(
                "network",
                format!("Could not reach the MiniMax account service: {error}"),
                &error,
            )
        })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|error| {
        crate::provider_error::transport_failure(
            "network",
            format!("MiniMax account response was cut short: {error}"),
            &error,
        )
    })?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(not_signed_in());
    }
    if !status.is_success() {
        return Err(ProviderError::http_failure(
            status,
            format!("MiniMax account service returned HTTP {status}."),
        ));
    }
    let parsed: CreditResponse = serde_json::from_slice(&body)
        .map_err(|_| unexpected("MiniMax account service returned an unreadable response."))?;
    if let Some(code) = parsed.base_resp.and_then(|base| base.status_code) {
        if code != 0 {
            return Err(base_resp_error(code));
        }
    }
    let key = parsed
        .api_key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .ok_or_else(not_entitled)?;
    // The open platform hands out a plan key for subscribed accounts. A
    // different key shape means the account has no Coding Plan (or the
    // route changed), which is an entitlement answer, not a schema error.
    if !key.starts_with(SUBSCRIPTION_KEY_PREFIX) {
        return Err(not_entitled());
    }
    Ok(SubscriptionKey(key))
}

/// Maps the upstream 'base_resp.status_code' vocabulary onto the shared
/// provider error codes. The upstream reports refusals inside an HTTP 200
/// body, so these are the only place entitlement failures are visible.
fn base_resp_error(code: i64) -> ProviderError {
    match code {
        BASE_RESP_UNAUTHORIZED => auth_invalid(),
        BASE_RESP_NOT_A_MEMBER | BASE_RESP_PERMISSION_DENIED => not_entitled(),
        _ => ProviderError::new(
            "unexpected_response",
            "MiniMax refused the quota request for this account.",
        ),
    }
}

// ---------- normalization ----------

/// Converts an upstream REMAINING percent into the wire contract's USED
/// percent. The upstream value is validated to 0-100 first; a value outside
/// that range is a schema change and is never clamped into a
/// plausible-looking number.
fn used_from_remaining(remaining: f64) -> Result<f64, ProviderError> {
    if !remaining.is_finite() || !(0.0..=100.0).contains(&remaining) {
        return Err(unexpected(
            "MiniMax returned a quota percentage outside the 0-100 range.",
        ));
    }
    Ok((100.0 - remaining).clamp(0.0, 100.0))
}

/// Parses the percent-string dialect ('"42.5%"') the 'remains_percent'
/// route speaks. A malformed or out-of-range value is rejected, never
/// clamped.
fn parse_percent_string(raw: &str) -> Result<f64, ProviderError> {
    let trimmed = raw.trim().trim_end_matches('%').trim();
    let value: f64 = trimmed
        .parse()
        .map_err(|_| unexpected("MiniMax returned an unreadable quota percentage."))?;
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(unexpected(
            "MiniMax returned a quota percentage outside the 0-100 range.",
        ));
    }
    Ok(value)
}

/// The used percent of one window, across both upstream dialects.
///
/// 'None' means the upstream did not report this window at all (the caller
/// then emits no window for it) or that the window is unlimited and has no
/// meaningful percentage (the caller emits no window for it either - an
/// unlimited window is never presented as 0% used).
fn window_used_percent(
    remaining: Option<f64>,
    used: Option<&str>,
) -> Result<Option<f64>, ProviderError> {
    if let Some(remaining) = remaining {
        return Ok(Some(used_from_remaining(remaining)?));
    }
    if let Some(used) = used {
        return Ok(Some(parse_percent_string(used)?));
    }
    Ok(None)
}

/// Converts an absolute epoch-millisecond stamp into an RFC 3339 UTC
/// timestamp. Second-shaped values are rejected, never promoted.
fn reset_rfc3339(value: Option<i64>) -> Option<String> {
    let ms = value.filter(|ms| (MIN_RESET_MS..=MAX_RESET_MS).contains(ms))?;
    let stamp: DateTime<Utc> = DateTime::from_timestamp_millis(ms)?;
    Some(stamp.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// Selects the one pool that represents the plan's text allowance.
///
/// The shared pool ('general') is preferred and is what the live plan
/// reports. When a plan exposes only model-specific pools, exactly one
/// non-media pool is accepted - never several, because that would be an
/// ambiguous choice between separate allowances.
fn select_shared_pool(entries: &[ModelRemains]) -> Result<&ModelRemains, ProviderError> {
    if let Some(shared) = entries.iter().find(|entry| {
        entry
            .model_name
            .as_deref()
            .map(|name| name.trim().eq_ignore_ascii_case(SHARED_POOL_MODEL))
            .unwrap_or(false)
    }) {
        return Ok(shared);
    }
    let candidates: Vec<&ModelRemains> = entries
        .iter()
        .filter(|entry| {
            entry
                .model_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| !name.to_ascii_lowercase().contains("video"))
                .unwrap_or(false)
        })
        .collect();
    match candidates.as_slice() {
        [only] => Ok(*only),
        [] => Err(unexpected(
            "MiniMax quota response carries no shared text-quota window.",
        )),
        _ => Err(unexpected(
            "MiniMax quota response carries several model-specific quota pools, so the plan's shared allowance cannot be determined.",
        )),
    }
}

/// Parses the official quota payload into normalized windows.
pub(crate) fn parse_quota(body: &[u8]) -> Result<Vec<MiniMaxLimitWindow>, ProviderError> {
    let parsed: QuotaResponse = serde_json::from_slice(body)
        .map_err(|_| unexpected("MiniMax quota response was not usable JSON."))?;
    if let Some(code) = parsed.base_resp.and_then(|base| base.status_code) {
        if code != 0 {
            return Err(base_resp_error(code));
        }
    }
    let entries = parsed.model_remains.unwrap_or_default();
    let shared = select_shared_pool(&entries)?;
    let mut limits = Vec::new();
    if let Some(limit) = five_hour_window(shared)? {
        limits.push(limit);
    }
    if let Some(limit) = weekly_window(shared)? {
        limits.push(limit);
    }
    if limits.is_empty() {
        return Err(unexpected(
            "MiniMax quota response carries no usable quota window.",
        ));
    }
    Ok(limits)
}

/// The five-hour pool. 'current_interval_status == 3' means the window is
/// unlimited: it has no meaningful used percentage, so no window is emitted
/// rather than a fabricated 0%.
fn five_hour_window(entry: &ModelRemains) -> Result<Option<MiniMaxLimitWindow>, ProviderError> {
    if entry.current_interval_status == Some(STATUS_UNLIMITED) {
        return Ok(None);
    }
    let Some(used) = window_used_percent(
        entry.current_interval_remaining_percent,
        entry.current_interval_used_percent.as_deref(),
    )?
    else {
        return Ok(None);
    };
    Ok(Some(MiniMaxLimitWindow {
        label: FIVE_HOUR_LABEL.to_string(),
        used_percent: used,
        reset_at: reset_rfc3339(entry.end_time),
    }))
}

/// The weekly pool.
fn weekly_window(entry: &ModelRemains) -> Result<Option<MiniMaxLimitWindow>, ProviderError> {
    if entry.current_weekly_status == Some(STATUS_UNLIMITED) {
        return Ok(None);
    }
    let Some(used) = window_used_percent(
        entry.current_weekly_remaining_percent,
        entry.current_weekly_used_percent.as_deref(),
    )?
    else {
        return Ok(None);
    };
    Ok(Some(MiniMaxLimitWindow {
        label: WEEKLY_LABEL.to_string(),
        used_percent: used,
        reset_at: reset_rfc3339(entry.weekly_end_time),
    }))
}

// ---------- entry point ----------

/// Reads the current MiniMax subscription quota.
pub(crate) async fn get_minimax_usage() -> Result<MiniMaxUsage, ProviderError> {
    let (key, region) = resolve_subscription_key().await?;
    let client = http_client()?;
    let limits = fetch_quota(client, &key, region)
        .await
        .map_err(|error| scrub_failure(error, &key))?;
    Ok(MiniMaxUsage {
        limits,
        account: account_hint(&key.0).map(|key_hint| MiniMaxAccount { key_hint }),
    })
}

/// Resolves the subscription key: the explicit environment override first,
/// then the managed-login exchange.
async fn resolve_subscription_key() -> Result<(SubscriptionKey, Region), ProviderError> {
    if let Ok(key) = std::env::var(SUBSCRIPTION_KEY_ENV) {
        let key = key.trim();
        if !key.is_empty() {
            return Ok((SubscriptionKey(key.to_string()), Region::International));
        }
    }
    let (access_token, region) = load_managed_access_token()?;
    let key = fetch_subscription_key(&access_token, region).await?;
    Ok((key, region))
}

/// Final-boundary value scrub ('secret_scrub'): the subscription key is in
/// scope for its own request, so any exact occurrence of it in the failure
/// message is removed before the failure leaves the adapter.
fn scrub_failure(error: ProviderError, key: &SubscriptionKey) -> ProviderError {
    crate::secret_scrub::scrub_provider_error(error, &[key.0.as_str()])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact payload captured live from the official endpoint on
    /// 2026-10-22 for the shared 'general' pool.
    const LIVE_SHARED: &str = r#"{
      "model_remains": [
        {
          "start_time": 1792634466447,
          "end_time": 1792652466447,
          "remains_time": 17999988,
          "current_interval_total_count": 0,
          "current_interval_usage_count": 0,
          "model_name": "general",
          "current_weekly_total_count": 0,
          "current_weekly_usage_count": 0,
          "weekly_start_time": 1792634466447,
          "weekly_end_time": 1793239266447,
          "weekly_remains_time": 604799988,
          "current_interval_status": 1,
          "current_interval_remaining_percent": 100,
          "current_weekly_status": 1,
          "current_weekly_remaining_percent": 100
        }
      ],
      "base_resp": { "status_code": 0, "status_msg": "success" }
    }"#;

    #[test]
    fn parses_the_live_shared_pool_into_two_windows() {
        let limits = parse_quota(LIVE_SHARED.as_bytes()).expect("live payload");
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].label, "5-hour");
        assert_eq!(limits[1].label, "Weekly");
        // The upstream reports REMAINING; the wire carries USED.
        assert!((limits[0].used_percent - 0.0).abs() < 1e-9);
        assert!((limits[1].used_percent - 0.0).abs() < 1e-9);
        // The absolute epoch-millisecond boundary becomes an RFC 3339 stamp.
        assert_eq!(
            limits[0].reset_at.as_deref(),
            Some("2026-10-22T07:01:06.447Z")
        );
        assert_eq!(
            limits[1].reset_at.as_deref(),
            Some("2026-10-29T02:01:06.447Z")
        );
    }

    #[test]
    fn converts_remaining_percent_into_used_percent() {
        let body = r#"{
          "model_remains": [{
            "model_name": "general",
            "end_time": 1792652466447,
            "weekly_end_time": 1793239266447,
            "current_interval_remaining_percent": 25,
            "current_weekly_remaining_percent": 80
          }],
          "base_resp": { "status_code": 0 }
        }"#;
        let limits = parse_quota(body.as_bytes()).expect("converted");
        assert!((limits[0].used_percent - 75.0).abs() < 1e-9);
        assert!((limits[1].used_percent - 20.0).abs() < 1e-9);
    }

    #[test]
    fn reads_the_used_percent_string_dialect() {
        let body = r#"{
          "model_remains": [{
            "model_name": "general",
            "end_time": 1792652466447,
            "weekly_end_time": 1793239266447,
            "current_interval_status": 1,
            "current_interval_used_percent": "19.9%",
            "current_weekly_status": 1,
            "current_weekly_used_percent": "20%"
          }],
          "base_resp": { "status_code": 0 }
        }"#;
        let limits = parse_quota(body.as_bytes()).expect("used dialect");
        assert!((limits[0].used_percent - 19.9).abs() < 1e-9);
        assert!((limits[1].used_percent - 20.0).abs() < 1e-9);
    }

    #[test]
    fn ignores_the_relative_remains_time_fields() {
        // 'remains_time' is a relative countdown in seconds, not an epoch
        // stamp. It must never become a reset timestamp.
        let body = r#"{
          "model_remains": [{
            "model_name": "general",
            "remains_time": 17999988,
            "weekly_remains_time": 604799988,
            "current_interval_remaining_percent": 10,
            "current_weekly_remaining_percent": 10
          }],
          "base_resp": { "status_code": 0 }
        }"#;
        let limits = parse_quota(body.as_bytes()).expect("no relative stamps");
        assert!(limits[0].reset_at.is_none());
        assert!(limits[1].reset_at.is_none());
    }

    #[test]
    fn reports_only_the_shared_pool_and_never_sums_model_pools() {
        let body = r#"{
          "model_remains": [
            { "model_name": "video", "current_interval_remaining_percent": 5 },
            { "model_name": "MiniMax-M3", "current_interval_remaining_percent": 90 },
            {
              "model_name": "general",
              "end_time": 1792652466447,
              "current_interval_remaining_percent": 40,
              "current_weekly_remaining_percent": 60
            }
          ],
          "base_resp": { "status_code": 0 }
        }"#;
        let limits = parse_quota(body.as_bytes()).expect("shared pool only");
        assert_eq!(limits.len(), 2);
        // The shared pool's own numbers, never a combination with the
        // model-specific or media pools.
        assert!((limits[0].used_percent - 60.0).abs() < 1e-9);
        assert!((limits[1].used_percent - 40.0).abs() < 1e-9);
    }

    #[test]
    fn several_model_pools_without_a_shared_pool_are_ambiguous() {
        let body = r#"{
          "model_remains": [
            { "model_name": "MiniMax-M3", "current_interval_remaining_percent": 90 },
            { "model_name": "MiniMax-M2", "current_interval_remaining_percent": 70 }
          ],
          "base_resp": { "status_code": 0 }
        }"#;
        let error = parse_quota(body.as_bytes()).expect_err("ambiguous");
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn a_single_model_pool_without_a_shared_pool_is_accepted() {
        let body = r#"{
          "model_remains": [
            { "model_name": "MiniMax-M3", "current_interval_remaining_percent": 20 }
          ],
          "base_resp": { "status_code": 0 }
        }"#;
        let limits = parse_quota(body.as_bytes()).expect("single pool");
        assert_eq!(limits.len(), 1);
        assert!((limits[0].used_percent - 80.0).abs() < 1e-9);
    }

    #[test]
    fn a_response_without_any_pool_is_not_reported_as_zero() {
        let body = r#"{
          "model_remains": [
            { "model_name": "video", "current_interval_remaining_percent": 5 }
          ],
          "base_resp": { "status_code": 0 }
        }"#;
        let error = parse_quota(body.as_bytes()).expect_err("no text pool");
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn an_unlimited_window_yields_no_fabricated_percentage() {
        let body = r#"{
          "model_remains": [{
            "model_name": "general",
            "current_interval_status": 3,
            "current_interval_remaining_percent": 0,
            "weekly_end_time": 1793239266447,
            "current_weekly_status": 1,
            "current_weekly_remaining_percent": 100
          }],
          "base_resp": { "status_code": 0 }
        }"#;
        let limits = parse_quota(body.as_bytes()).expect("weekly only");
        assert_eq!(limits.len(),  1);
        assert_eq!(limits[0].label, "Weekly");
    }

    #[test]
    fn a_refused_subscription_is_a_deterministic_entitlement_error() {
        let body = r#"{
          "base_resp": { "status_code": 1004, "status_msg": "login fail" }
        }"#;
        let error = parse_quota(body.as_bytes()).expect_err("refused");
        assert_eq!(error.code, "auth_invalid");
        assert_eq!(error.transient, Some(false));

        let body = r#"{ "base_resp": { "status_code": 1003 } }"#;
        let error = parse_quota(body.as_bytes()).expect_err("not a member");
        assert_eq!(error.code, "not_entitled");
        assert_eq!(error.transient, Some(false));
    }

    #[test]
    fn a_malformed_response_is_a_schema_error_not_a_zero_reading() {
        let error = parse_quota(b"not json").expect_err("malformed");
        assert_eq!(error.code, "unexpected_response");
        let error = parse_quota(b"{}").expect_err("empty");
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn a_percentage_outside_the_range_is_never_clamped_into_a_plausible_number() {
        let body = r#"{
          "model_remains": [{
            "model_name": "general",
            "current_interval_remaining_percent": 101,
            "current_weekly_remaining_percent": 100
          }],
          "base_resp": { "status_code": 0 }
        }"#;
        let error = parse_quota(body.as_bytes()).expect_err("out of range");
        assert_eq!(error.code, "unexpected_response");

        let body = r#"{
          "model_remains": [{
            "model_name": "general",
            "current_interval_status": 1,
            "current_interval_used_percent": "180%",
            "current_weekly_status": 1,
            "current_weekly_used_percent": "0%"
          }],
          "base_resp": { "status_code": 0 }
        }"#;
        let error = parse_quota(body.as_bytes()).expect_err("out of range");
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn the_reset_stamp_uses_the_absolute_epoch_millisecond_boundary() {
        // 1792652466447 ms is the five-hour window edge; the relative
        // 'remains_time' of the same payload must not be used instead.
        let body = r#"{
          "model_remains": [{
            "model_name": "general",
            "end_time": 1792652466447,
            "remains_time": 17999988,
            "current_interval_remaining_percent": 50,
            "current_weekly_remaining_percent": 50
          }],
          "base_resp": { "status_code": 0 }
        }"#;
        let limits = parse_quota(body.as_bytes()).expect("absolute stamp");
        assert_eq!(
            limits[0].reset_at.as_deref(),
            Some("2026-10-22T07:01:06.447Z")
        );
    }

    #[test]
    fn second_shaped_timestamps_are_rejected_rather_than_promoted() {
        let body = r#"{
          "model_remains": [{
            "model_name": "general",
            "end_time": 1792652466,
            "current_interval_remaining_percent": 50,
            "current_weekly_remaining_percent": 50
          }],
          "base_resp": { "status_code": 0 }
        }"#;
        let limits = parse_quota(body.as_bytes()).expect("parsed");
        assert!(limits[0].reset_at.is_none());
    }

    #[test]
    fn the_key_hint_masks_the_tail_and_never_leaks_the_key() {
        let key = SubscriptionKey("sk-cp-abcdefghijklmnop".to_string());
        assert_eq!(account_hint(&key.0).as_deref(), Some("mnop"));
        assert_eq!(identity(&key.0).as_deref(), Some("key:mnop"));
        let rendered = format!("{key:?}");
        assert!(!rendered.contains("sk-cp-"));
        assert!(!rendered.contains("abcdefghijklmnop"));
    }

    #[test]
    fn a_short_key_gets_no_attribution() {
        assert!(account_hint("sk-short").is_none());
    }

    #[test]
    fn the_credential_store_parser_accepts_the_cli_shape() {
        let raw = r#"{
          "schemaVersion": 1,
          "records": {
            "com.minimax.mcode.oauth.prod.en:account": {
              "schemaVersion": 1,
              "accessToken": "access-token-value",
              "refreshToken": "refresh-token-value",
              "clientId": "client-id-value",
              "expiresAtMs": 1792639286783
            }
          }
        }"#;
        let record = parse_auth_store(raw).expect("record");
        assert_eq!(record.access_token.as_deref(), Some("access-token-value"));
        assert_eq!(record.expires_at_ms, Some(1792639286783));
        // The refresh token is never taken into the adapter's hands.
        let rendered = format!("{record:?}");
        assert!(!rendered.contains("access-token-value"));
        assert!(!rendered.contains("refresh-token-value"));
    }

    #[test]
    fn an_empty_credential_store_is_a_missing_credential_not_a_crash() {
        let error = parse_auth_store(r#"{ "schemaVersion": 1, "records": {} }"#)
            .expect_err("empty");
        assert_eq!(error.code, "credential_missing");
        let error = parse_auth_store("not json").expect_err("malformed");
        assert_eq!(error.code, "auth_unreadable");
    }

    #[test]
    fn the_credential_root_is_the_minimax_cli_directory() {
        let home = Path::new("/home/example");
        assert_eq!(
            minimax_dir(home),
            PathBuf::from("/home/example").join(".minimax")
        );
    }

    #[test]
    fn the_quota_route_is_the_documented_official_endpoint() {
        assert_eq!(QUOTA_URL, "https://www.minimax.io/v1/token_plan/remains");
        assert_eq!(QUOTA_URL_CN, "https://www.minimaxi.com/v1/token_plan/remains");
    }

    #[test]
    fn the_regional_hosts_are_the_verified_ones() {
        assert_eq!(Region::International.quota_url(), QUOTA_URL);
        assert_eq!(Region::Mainland.quota_url(), QUOTA_URL_CN);
        assert_eq!(
            Region::International.credit_url(),
            "https://platform.minimax.io/backend/account/token_plan_credit"
        );
        assert_eq!(
            Region::Mainland.credit_url(),
            "https://www.minimaxi.com/backend/account/token_plan_credit"
        );
    }

    /// Live probe: reads the real subscription quota with the credential
    /// the collector itself resolves. Ignored by default - it needs a live
    /// MiniMax Coding Plan on this machine.
    #[tokio::test]
    #[ignore = "reads the local MiniMax subscription quota"]
    async fn live_minimax_quota_returns_both_windows() {
        let usage = get_minimax_usage().await.expect("live MiniMax quota");
        assert!(!usage.limits.is_empty(), "at least one quota window");
        assert!(
            usage
                .limits
                .iter()
                .all(|limit| (0.0..=100.0).contains(&limit.used_percent)),
            "every window stays inside 0-100"
        );
        let labels: Vec<&str> = usage.limits.iter().map(|limit| limit.label.as_str()).collect();
        assert!(
            labels.contains(&"5-hour"),
            "the five-hour pool is reported, got {labels:?}"
        );
        assert!(
            labels.contains(&"Weekly"),
            "the weekly pool is reported, got {labels:?}"
        );
        for limit in &usage.limits {
            let Some(reset_at) = limit.reset_at.as_deref() else {
                continue;
            };
            let reset = DateTime::parse_from_rfc3339(reset_at)
                .expect("rfc3339")
                .with_timezone(&Utc);
            assert!(reset > Utc::now(), "a reset must lie in the future");
        }
    }
}
