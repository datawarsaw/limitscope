//! OpenCode Go provider backend.
//!
//! Resolves the *currently active* OpenCode Go API key, read-only, then
//! queries the official Go usage endpoint
//! (`GET https://opencode.ai/zen/go/v1/usage`). Resolution order:
//!
//! 1. OpenCodex's provider config (`~/.opencodex/config.json`,
//!    `providers.opencode-go.apiKey`) — the credential that harness actively
//!    routes Go traffic with, so an account switched or removed there takes
//!    effect on the next refresh.
//! 2. OpenCode's auth file (`~/.local/share/opencode/auth.json`,
//!    `opencode-go.key`) — the fallback when OpenCodex is absent or carries
//!    no Go entry.
//!
//! A malformed OpenCodex config is a hard error rather than a silent
//! fallback: falling back could resurrect a removed account's key as the
//! "active" one. The key stays in this process: nothing beyond normalized
//! windows plus a last-four key fingerprint (account attribution) is
//! returned to the WebView, and no auth material is ever logged.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::provider_error::ProviderError;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const USER_AGENT: &str = concat!("rate-limits/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// ---------- data returned to the WebView (camelCase on the wire) ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeGoLimitWindow {
    pub label: String,
    pub used_percent: f64,
    /// Provider-reported ISO-8601 timestamp, preserved exactly as sent
    /// (legacy `resetInSec` responses derive one instead). `None` when the
    /// upstream timestamp is missing or malformed.
    pub reset_at: Option<String>,
}

/// Attribution of the windows to the one account whose credential resolved
/// as active (OpenCodex's config first, OpenCode's auth file as fallback).
/// Both harnesses keep at most a single OpenCode Go key, so there is nothing
/// to enumerate: the quota shown always belongs to that stored credential.
/// `keyHint` is its last four characters — the same tail the OpenCode
/// console prints when masking keys — so the user can tell WHICH account's
/// state is on the card without any secret material crossing the boundary.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeGoAccount {
    pub key_hint: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeGoUsage {
    pub limits: Vec<OpenCodeGoLimitWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<OpenCodeGoAccount>,
}

fn opencode_not_installed() -> ProviderError {
    ProviderError::new(
        "opencode_not_installed",
        "OpenCode is not installed (OpenCode data directory not found).",
    )
}

fn auth_file_missing() -> ProviderError {
    ProviderError::new(
        "auth_file_missing",
        "OpenCode auth file not found (auth.json). Open OpenCode or run `opencode auth login` first.",
    )
}

fn opencodex_config_unreadable() -> ProviderError {
    ProviderError::new(
        "opencodex_config_unreadable",
        "OpenCodex config (~/.opencodex/config.json) exists but could not be read or parsed, so the active OpenCode Go account cannot be determined.",
    )
}

fn credential_missing() -> ProviderError {
    ProviderError::new(
        "credential_missing",
        "No active opencode-go credential found (checked OpenCodex's config and OpenCode's auth.json). Sign in to OpenCode Go in either harness, then refresh again.",
    )
}

/// The stored entry does not hold a single usable key (e.g. the `key` field
/// is not a string). Refusing loudly is the honest verdict: guessing any one
/// value would report some account's quota as if it were "the" OpenCode Go
/// state (MIC-297).
fn credential_ambiguous() -> ProviderError {
    ProviderError::new(
        "credential_ambiguous",
        "The opencode-go entry in the OpenCode auth file does not hold a single key, so the account it belongs to cannot be determined. Reconnect OpenCode Go once (`opencode auth login`) to rewrite it.",
    )
}

fn auth_invalid() -> ProviderError {
    ProviderError::new(
        "auth_invalid",
        "The OpenCode Go API key was rejected. Re-authenticate OpenCode (`opencode auth login`), then refresh again.",
    )
}

fn not_entitled() -> ProviderError {
    ProviderError::new(
        "not_entitled",
        "This account has no OpenCode Go plan, so the usage endpoint refused access.",
    )
}

// ---------- upstream response (only the fields we consume; extra ignored) ----------
//
// Current shape (observed live 2026-09): usage.{rolling,weekly,monthly} with
// `percent` in 0–100 units and an absolute ISO-8601 `resetsAt` per window.
// Legacy shape also seen in the wild: {rolling,weekly,monthly}Usage with
// `usagePercent` and a relative `resetInSec`. Both are feature-detected.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageResponse {
    #[serde(default)]
    usage: Option<UsageGroup>,
    #[serde(default)]
    rolling_usage: Option<LegacyWindow>,
    #[serde(default)]
    weekly_usage: Option<LegacyWindow>,
    #[serde(default)]
    monthly_usage: Option<LegacyWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageGroup {
    #[serde(default)]
    rolling: Option<CurrentWindow>,
    #[serde(default)]
    weekly: Option<CurrentWindow>,
    #[serde(default)]
    monthly: Option<CurrentWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentWindow {
    // A `status` field ("ok" / "rate-limited") is also sent but is purely
    // informational — `percent` carries the used figure, so it is ignored.
    #[serde(default)]
    percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyWindow {
    #[serde(default)]
    usage_percent: Option<f64>,
    #[serde(default)]
    reset_in_sec: Option<f64>,
}

// ---------- normalization ----------

/// Only windows that are actually present upstream are emitted — no invented
/// or defaulted limits. Order: rolling (5h), weekly, monthly.
fn normalize_usage(response: &UsageResponse, now: DateTime<Utc>) -> Vec<OpenCodeGoLimitWindow> {
    let mut limits = Vec::new();
    if let Some(usage) = &response.usage {
        for (window, label) in [
            (&usage.rolling, "5-hour"),
            (&usage.weekly, "Weekly"),
            (&usage.monthly, "30-day"),
        ] {
            if let Some(window) = window {
                if let Some(percent) = window.percent {
                    limits.push(OpenCodeGoLimitWindow {
                        label: label.to_string(),
                        used_percent: percent.clamp(0.0, 100.0),
                        reset_at: validated_reset(&window.resets_at),
                    });
                }
            }
        }
    }
    if limits.is_empty() {
        for (window, label) in [
            (&response.rolling_usage, "5-hour"),
            (&response.weekly_usage, "Weekly"),
            (&response.monthly_usage, "30-day"),
        ] {
            if let Some(window) = window {
                if let Some(percent) = window.usage_percent {
                    let reset_at = window
                        .reset_in_sec
                        .and_then(|seconds| now.checked_add_signed(TimeDelta::seconds(seconds.round() as i64)))
                        .map(|reset| reset.to_rfc3339_opts(SecondsFormat::Secs, true));
                    limits.push(OpenCodeGoLimitWindow {
                        label: label.to_string(),
                        used_percent: percent.clamp(0.0, 100.0),
                        reset_at,
                    });
                }
            }
        }
    }
    limits
}

/// The reset timestamp is passed through exactly as the provider sent it; a
/// value that does not parse as ISO-8601 is dropped (the UI renders the
/// window without a reset line) rather than guessed at.
fn validated_reset(raw: &Option<String>) -> Option<String> {
    let raw = raw.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    match DateTime::parse_from_rfc3339(raw) {
        Ok(_) => Some(raw.to_string()),
        Err(_) => None,
    }
}

fn parse_windows(body: &[u8], now: DateTime<Utc>) -> Result<Vec<OpenCodeGoLimitWindow>, ProviderError> {
    let parsed: UsageResponse = serde_json::from_slice(body).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "OpenCode Go usage response was not usable JSON (schema change or sign-in redirect).",
        )
    })?;
    let limits = normalize_usage(&parsed, now);
    if limits.is_empty() {
        return Err(ProviderError::new(
            "unexpected_response",
            "OpenCode Go usage response format changed; no known windows found.",
        ));
    }
    Ok(limits)
}

// ---------- local auth state (read-only) ----------

// Manual impl so the key can never reach logs through Debug formatting.
struct OpenCodeGoKey(String);

impl std::fmt::Debug for OpenCodeGoKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OpenCodeGoKey").field(&"<redacted>").finish()
    }
}

/// OpenCode resolves its auth file XDG-style: `$XDG_DATA_HOME/opencode`
/// when that directory exists, else `~/.local/share/opencode` (also the
/// layout OpenCode uses on Windows).
fn opencode_data_dir(home: &Path) -> PathBuf {
    if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
        let xdg_data_home = xdg_data_home.trim();
        if !xdg_data_home.is_empty() {
            let dir = PathBuf::from(xdg_data_home).join("opencode");
            if dir.exists() {
                return dir;
            }
        }
    }
    home.join(".local").join("share").join("opencode")
}

/// Parses `~/.opencodex/config.json` for the active `opencode-go` API key
/// (`providers.opencode-go.apiKey` — the credential OpenCodex actually
/// routes Go traffic with). `Ok(None)` = the config is valid but carries no
/// usable Go credential (OpenCodex not configured for Go, or the account
/// was removed there) → caller falls back to OpenCode's auth file. `Err` =
/// the config exists but is malformed — a hard error, because silently
/// falling back could resurrect a removed account's key as the active one.
fn parse_opencodex_config(raw: &str) -> Result<Option<String>, ProviderError> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| opencodex_config_unreadable())?;
    let key = value
        .get("providers")
        .and_then(|providers| providers.get("opencode-go"))
        .filter(|entry| entry.is_object())
        .and_then(|entry| entry.get("apiKey"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty());
    Ok(key.map(str::to_string))
}

/// Pure precedence core: OpenCodex's active Go key wins, OpenCode's auth
/// file is the fallback; `None` = that store is absent entirely. Unlike a
/// "first error remembered" fallback chain, a malformed OpenCodex config is
/// a hard error here: both stores hold *account* credentials, and silently
/// falling back could resurrect a removed account's key as the active one.
fn resolve_credentials(
    opencodex: Option<Result<Option<String>, ProviderError>>,
    opencode: Option<Result<String, ProviderError>>,
) -> Result<String, ProviderError> {
    if let Some(result) = opencodex {
        match result {
            Ok(Some(key)) => return Ok(key),
            // Valid config without a Go entry: OpenCodex is simply not
            // (or no longer) configured for OpenCode Go — use OpenCode's.
            Ok(None) => {}
            Err(error) => return Err(error),
        }
    }
    match opencode {
        Some(result) => result,
        None => Err(opencode_not_installed()),
    }
}

/// Resolves the currently active OpenCode Go credential, read-only, from
/// the user's own harness stores.
fn resolve_credential(home: &Path) -> Result<OpenCodeGoKey, ProviderError> {
    let opencodex_config = home.join(".opencodex").join("config.json");
    let opencodex = opencodex_config.exists().then(|| {
        fs::read_to_string(&opencodex_config)
            .map_err(|_| opencodex_config_unreadable())
            .and_then(|raw| parse_opencodex_config(&raw))
    });
    let dir = opencode_data_dir(home);
    let opencode = dir.exists().then(|| {
        fs::read_to_string(dir.join("auth.json"))
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => auth_file_missing(),
                _ => ProviderError::new(
                    "auth_unreadable",
                    format!("Could not read the OpenCode auth file: {error}"),
                ),
            })
            .and_then(|raw| parse_auth(&raw))
    });
    resolve_credentials(opencodex, opencode).map(OpenCodeGoKey)
}

/// Like [`resolve_credential`], anchored at the process user's home.
fn resolve_active_credential() -> Result<OpenCodeGoKey, ProviderError> {
    let home = std::env::home_dir().ok_or_else(|| {
        ProviderError::new(
            "opencode_not_installed",
            "Could not locate the user home directory.",
        )
    })?;
    resolve_credential(&home)
}

fn parse_auth(raw: &str) -> Result<String, ProviderError> {
    let value: Value = serde_json::from_str(raw).map_err(|_| {
        ProviderError::new(
            "auth_unreadable",
            "OpenCode auth state (auth.json) is not valid JSON.",
        )
    })?;
    let entry = value.get("opencode-go").filter(|entry| entry.is_object());
    let Some(entry) = entry else {
        return Err(credential_missing());
    };
    let key = match entry.get("key") {
        // Absent or blank: nothing stored, same as no entry.
        None => return Err(credential_missing()),
        Some(Value::String(key)) => key.trim().to_string(),
        // A non-string value cannot be resolved to one credential. Never
        // fall back to "the first thing that looks like a key".
        Some(_) => return Err(credential_ambiguous()),
    };
    if key.is_empty() {
        return Err(credential_missing());
    }
    Ok(key)
}

/// Last four characters of the stored key — the tail the OpenCode console
/// itself shows when masking keys (`…1234`). Deterministic per credential,
/// so the label stays stable across refreshes and the user can match the
/// card to an account without any usable secret crossing the boundary.
/// `None` when the key is too short to mask meaningfully or the tail is not
/// printable key material.
fn account_hint(key: &str) -> Option<OpenCodeGoAccount> {
    let tail = key.chars().rev().take(4).collect::<Vec<_>>();
    // Real Go keys are ~67 chars; anything shorter than 12 would leak too
    // much of the key through the hint, so it gets no attribution at all.
    if key.chars().count() < 12 {
        return None;
    }
    if !tail
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
    {
        return None;
    }
    let tail: String = tail.into_iter().rev().collect();
    Some(OpenCodeGoAccount { key_hint: tail })
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

// ---------- fetch + command ----------

async fn fetch_opencode_go_usage() -> Result<OpenCodeGoUsage, ProviderError> {
    let key = resolve_active_credential()?;
    // Final-boundary value scrub (`secret_scrub`): the key is alive for this
    // whole fetch, so any exact occurrence of it in a failure message is
    // removed before the error leaves the adapter.
    fetch_usage_for_key(&key)
        .await
        .map_err(|error| crate::secret_scrub::scrub_provider_error(error, &[key.0.as_str()]))
}

/// The usage fetch for an already-resolved credential. Every failure after
/// this point carries the credential's masked identity so the runtime can
/// never retain one account's last-good data for a different account
/// (MIC-297 follow-up: attribution must survive the error path too).
async fn fetch_usage_for_key(key: &OpenCodeGoKey) -> Result<OpenCodeGoUsage, ProviderError> {
    // The identity string matches the attribution the success path builds
    // (`build_opencode_attribution` in the runtime), so the failure path and
    // the retained snapshot compare like-for-like.
    let identity = account_hint(&key.0).map(|account| format!("key:{}", account.key_hint));
    let client = http_client()?;
    let response = client
        .get(USAGE_URL)
        .bearer_auth(&key.0)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|error| {
            crate::provider_error::transport_failure(
                "network",
                format!("Could not reach the OpenCode Go usage endpoint: {error}"),
                &error,
            )
            .with_identity_hint(identity.clone())
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(auth_invalid().with_identity_hint(identity.clone()));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(not_entitled().with_identity_hint(identity.clone()));
    }
    let retry_after = crate::provider_error::retry_after_header(response.headers()).map(String::from);
    let body = response.bytes().await.map_err(|error| {
        crate::provider_error::transport_failure(
            "network",
            format!("OpenCode Go usage response was cut short: {error}"),
            &error,
        )
        .with_identity_hint(identity.clone())
    })?;
    if !status.is_success() {
        return Err(ProviderError::http_failure_with_retry_after(
            status,
            format!("OpenCode Go usage endpoint returned HTTP {status}."),
            retry_after.as_deref(),
            Utc::now(),
        )
        .with_identity_hint(identity.clone()));
    }

    Ok(OpenCodeGoUsage {
        limits: parse_windows(&body, Utc::now())
            .map_err(|error| error.with_identity_hint(identity.clone()))?,
        account: account_hint(&key.0),
    })
}

#[tauri::command]
pub async fn get_opencode_go_usage() -> Result<OpenCodeGoUsage, ProviderError> {
    fetch_opencode_go_usage().await
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
    async fn redirect_is_returned_without_forwarding_bearer_token() {
        let destination = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        destination.set_nonblocking(true).unwrap();
        let destination_addr = destination.local_addr().unwrap();
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let source_addr = source.local_addr().unwrap();
        let destination_thread = thread::spawn(move || destination.accept());
        let source_thread = thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    format!("HTTP/1.1 302 Found\r\nLocation: http://{destination_addr}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
                )
                .unwrap();
            String::from_utf8_lossy(&request).to_ascii_lowercase()
        });
        let response = http_client()
            .unwrap()
            .get(format!("http://{source_addr}/usage"))
            .bearer_auth("unit-test-token")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        assert!(source_thread
            .join()
            .unwrap()
            .contains("authorization: bearer unit-test-token"));
        assert!(
            destination_thread.join().unwrap().unwrap_err().kind()
                == std::io::ErrorKind::WouldBlock
        );
    }

    fn now_utc() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap()
    }

    // Shape observed live from GET /zen/go/v1/usage on 2026-09-27.
    fn current_shape_body() -> Value {
        serde_json::json!({
            "usage": {
                "rolling": {
                    "status": "ok",
                    "percent": 0,
                    "resetsAt": "2026-09-27T19:32:57.751Z"
                },
                "weekly": {
                    "status": "ok",
                    "percent": 0,
                    "resetsAt": "2026-09-28T00:00:00.000Z"
                },
                "monthly": {
                    "status": "rate-limited",
                    "percent": 100,
                    "resetsAt": "2026-10-03T20:48:17.000Z"
                }
            }
        })
    }

    #[test]
    fn parse_auth_reads_opencode_go_key() {
        let raw = serde_json::json!({
            "openai": { "type": "oauth", "refresh": "r", "access": "a", "expires": 1 },
            "openrouter": { "type": "api", "key": "sk-or" },
            "xai": { "type": "oauth", "refresh": "r", "access": "a", "expires": 1 },
            "opencode-go": { "type": "api", "key": "  oc-go-key  " }
        })
        .to_string();
        assert_eq!(parse_auth(&raw).unwrap(), "oc-go-key");
    }

    #[test]
    fn parse_auth_rejects_missing_or_empty_credential() {
        assert_eq!(parse_auth(r#"{}"#).unwrap_err().code, "credential_missing");
        assert_eq!(
            parse_auth(r#"{"openai":{"type":"oauth"}}"#).unwrap_err().code,
            "credential_missing"
        );
        assert_eq!(
            parse_auth(r#"{"opencode-go":{"type":"api"}}"#).unwrap_err().code,
            "credential_missing"
        );
        assert_eq!(
            parse_auth(r#"{"opencode-go":{"type":"api","key":"   "}}"#)
                .unwrap_err()
                .code,
            "credential_missing"
        );
        assert_eq!(parse_auth("not json").unwrap_err().code, "auth_unreadable");
    }

    /// MIC-297: an entry whose `key` is not a single string (e.g. a list of
    /// keys left by some tooling) must fail loudly instead of silently
    /// reporting whichever value the parser happened to see first.
    #[test]
    fn parse_auth_rejects_non_string_key_as_ambiguous() {
        for malformed in [
            r#"{"opencode-go":{"type":"api","key":["sk-one","sk-two"]}}"#,
            r#"{"opencode-go":{"type":"api","key":{"a":"sk-one"}}}"#,
            r#"{"opencode-go":{"type":"api","key":123}}"#,
            r#"{"opencode-go":{"type":"api","key":null}}"#,
        ] {
            assert_eq!(
                parse_auth(malformed).unwrap_err().code,
                "credential_ambiguous",
                "input: {malformed}"
            );
        }
    }

    fn opencodex_config_with(api_key: &str) -> String {
        serde_json::json!({
            "providers": {
                "opencode-go": {
                    "adapter": "openai-chat",
                    "baseUrl": "https://opencode.ai/zen/go/v1",
                    "authMode": "key",
                    "apiKey": api_key
                }
            }
        })
        .to_string()
    }

    #[test]
    fn parse_opencodex_config_reads_the_active_key() {
        assert_eq!(
            parse_opencodex_config(&opencodex_config_with("oc-go-key")).unwrap(),
            Some("oc-go-key".to_string())
        );
        // Surrounding whitespace is not part of the key (mirrors parse_auth).
        assert_eq!(
            parse_opencodex_config(&opencodex_config_with("  oc-go-key  ")).unwrap(),
            Some("oc-go-key".to_string())
        );
    }

    #[test]
    fn parse_opencodex_config_without_usable_go_entry_yields_none() {
        // No opencode-go provider at all: OpenCodex is simply not configured
        // for Go, so the caller falls back to OpenCode's auth file.
        assert_eq!(
            parse_opencodex_config(r#"{"providers":{"openai":{}}}"#).unwrap(),
            None
        );
        // An entry without (or with a blank) apiKey is not usable either —
        // a removed account must not resolve to an empty credential.
        assert_eq!(
            parse_opencodex_config(r#"{"providers":{"opencode-go":{"authMode":"key"}}}"#).unwrap(),
            None
        );
        assert_eq!(
            parse_opencodex_config(r#"{"providers":{"opencode-go":{"apiKey":"   "}}}"#).unwrap(),
            None
        );
    }

    #[test]
    fn parse_opencodex_config_rejects_malformed_json() {
        assert_eq!(
            parse_opencodex_config("not json").unwrap_err().code,
            "opencodex_config_unreadable"
        );
    }

    #[test]
    fn opencodex_credential_wins_over_opencode_auth_file() {
        // The auth file still holds the OLD account's key: it must not be
        // used, and the active (credit-bearing) credential wins.
        let key = resolve_credentials(
            Some(Ok(Some("opencodex-active-key".to_string()))),
            Some(Ok("stale-auth-file-key".to_string())),
        )
        .unwrap();
        assert_eq!(key, "opencodex-active-key");
    }

    #[test]
    fn opencode_auth_file_is_the_fallback_source() {
        // OpenCodex absent entirely, or present without a Go entry.
        assert_eq!(
            resolve_credentials(None, Some(Ok("oc-go-key".to_string()))).unwrap(),
            "oc-go-key"
        );
        assert_eq!(
            resolve_credentials(
                Some(Ok(None)),
                Some(Ok("oc-go-key".to_string()))
            )
            .unwrap(),
            "oc-go-key"
        );
    }

    #[test]
    fn malformed_opencodex_config_is_a_hard_error_not_a_fallback() {
        // Even with a (possibly stale) OpenCode key available: falling back
        // could resurrect a removed account as the active one.
        assert_eq!(
            resolve_credentials(
                Some(Err(opencodex_config_unreadable())),
                Some(Ok("stale-auth-file-key".to_string())),
            )
            .unwrap_err()
            .code,
            "opencodex_config_unreadable"
        );
    }

    #[test]
    fn missing_stores_map_to_actionable_errors() {
        // Neither store present: OpenCode itself is not installed.
        assert_eq!(
            resolve_credentials(None, None).unwrap_err().code,
            "opencode_not_installed"
        );
        // OpenCodex has no Go entry; the OpenCode data dir exists but its
        // auth file does not.
        assert_eq!(
            resolve_credentials(Some(Ok(None)), Some(Err(auth_file_missing())))
                .unwrap_err()
                .code,
            "auth_file_missing"
        );
    }

    /// One stored credential, one account: the fingerprint is exactly the
    /// key's last four characters.
    #[test]
    fn account_hint_is_the_key_tail() {
        let hint = account_hint("sk-abcdef123456").unwrap();
        assert_eq!(hint.key_hint, "3456");
        // Stable across repeated derivations (same key -> same label input).
        assert_eq!(account_hint("sk-abcdef123456"), Some(hint));
        assert!(account_hint("sk-abc12345").is_none());
    }

    #[test]
    fn account_hint_is_dropped_for_short_or_odd_tails() {
        // Too short to mask meaningfully — no attribution rather than a
        // hint that exposes most of the key.
        assert_eq!(account_hint("sk-key1"), None);
        assert_eq!(account_hint(""), None);
        // Whitespace or punctuation in the tail is not key material.
        assert_eq!(account_hint("sk-abcdef12 4"), None);
        assert_eq!(account_hint("sk-abcdef1.34"), None);
        // Dashes/underscores are common in key alphabets and stay usable.
        assert_eq!(account_hint("sk-abcdefab-d").unwrap().key_hint, "ab-d");
    }

    #[test]
    fn current_shape_parses_all_windows_and_preserves_resets() {
        let body = current_shape_body().to_string();
        let limits = parse_windows(body.as_bytes(), now_utc()).unwrap();
        assert_eq!(
            limits
                .iter()
                .map(|limit| limit.label.as_str())
                .collect::<Vec<_>>(),
            ["5-hour", "Weekly", "30-day"]
        );
        assert_eq!(limits[0].used_percent, 0.0);
        assert_eq!(limits[2].used_percent, 100.0);
        // Timestamps are passed through exactly as the provider sent them.
        assert_eq!(
            limits[0].reset_at.as_deref(),
            Some("2026-09-27T19:32:57.751Z")
        );
        assert_eq!(
            limits[1].reset_at.as_deref(),
            Some("2026-09-28T00:00:00.000Z")
        );
        assert_eq!(
            limits[2].reset_at.as_deref(),
            Some("2026-10-03T20:48:17.000Z")
        );
    }

    #[test]
    fn only_present_windows_are_emitted() {
        let body = serde_json::json!({
            "usage": {
                "weekly": { "status": "ok", "percent": 8.4, "resetsAt": "2026-09-28T00:00:00.000Z" }
            }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes(), now_utc()).unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Weekly");
        assert_eq!(limits[0].used_percent, 8.4);
    }

    #[test]
    fn percent_clamps_out_of_range_values() {
        let body = serde_json::json!({
            "usage": {
                "rolling": { "percent": 150.0 },
                "weekly": { "percent": -5.0 }
            }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes(), now_utc()).unwrap();
        assert_eq!(limits[0].used_percent, 100.0);
        assert_eq!(limits[1].used_percent, 0.0);
    }

    #[test]
    fn malformed_reset_is_dropped_but_window_kept() {
        let body = serde_json::json!({
            "usage": {
                "weekly": { "status": "ok", "percent": 12.0, "resetsAt": "soon-ish" }
            }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes(), now_utc()).unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "Weekly");
        assert_eq!(limits[0].used_percent, 12.0);
        assert_eq!(limits[0].reset_at, None);
    }

    #[test]
    fn legacy_shape_falls_back_to_relative_resets() {
        let body = serde_json::json!({
            "rollingUsage": { "usagePercent": 12.5, "resetInSec": 3600 },
            "weeklyUsage": { "usagePercent": 34.0, "resetInSec": 86_400 },
            "monthlyUsage": { "usagePercent": 5.0, "resetInSec": 1_209_600 }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes(), now_utc()).unwrap();
        assert_eq!(
            limits
                .iter()
                .map(|limit| limit.label.as_str())
                .collect::<Vec<_>>(),
            ["5-hour", "Weekly", "30-day"]
        );
        assert_eq!(limits[0].used_percent, 12.5);
        assert_eq!(limits[0].reset_at.as_deref(), Some("2026-09-27T13:00:00Z"));
        assert_eq!(limits[1].reset_at.as_deref(), Some("2026-09-28T12:00:00Z"));
        assert_eq!(limits[2].reset_at.as_deref(), Some("2026-10-11T12:00:00Z"));
    }

    #[test]
    fn current_shape_takes_precedence_over_legacy() {
        let body = serde_json::json!({
            "usage": {
                "rolling": { "status": "ok", "percent": 3.0, "resetsAt": "2026-09-27T19:32:57.751Z" }
            },
            "rollingUsage": { "usagePercent": 99.0, "resetInSec": 60 }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes(), now_utc()).unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].used_percent, 3.0);
    }

    #[test]
    fn html_body_and_empty_usage_are_rejected() {
        let html = b"<html><body><a href=\"/sign-in\">Sign in</a></body></html>";
        assert_eq!(parse_windows(html, now_utc()).unwrap_err().code, "unexpected_response");

        let empty = serde_json::json!({ "usage": {} }).to_string();
        assert_eq!(
            parse_windows(empty.as_bytes(), now_utc()).unwrap_err().code,
            "unexpected_response"
        );

        let no_percent = serde_json::json!({ "usage": { "weekly": { "status": "ok" } } }).to_string();
        assert_eq!(
            parse_windows(no_percent.as_bytes(), now_utc())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
    }

    #[test]
    fn non_object_payloads_are_schema_errors() {
        // A JSON array (proxy interstitial, truncated body) must not parse
        // as an empty success — only an object with known windows parses.
        let array = serde_json::json!([1, 2, 3]).to_string();
        let error = parse_windows(array.as_bytes(), now_utc()).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.transient, Some(false));
    }

    /// The structured retry verdict (`transient`/`httpStatus`) must mirror
    /// what the frontend used to infer from message wording.
    #[test]
    fn error_metadata_matches_the_retry_contract() {
        assert_eq!(
            ProviderError::transient("network", "offline").transient,
            Some(true)
        );
        assert_eq!(auth_invalid().transient, Some(false));
        assert_eq!(not_entitled().transient, Some(false));
        assert_eq!(credential_missing().transient, Some(false));
        assert_eq!(
            ProviderError::http_failure(
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                "OpenCode Go usage endpoint returned HTTP 429 Too Many Requests.",
            )
            .transient,
            Some(true)
        );
        assert_eq!(
            ProviderError::http_failure(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "OpenCode Go usage endpoint returned HTTP 503 Service Unavailable.",
            )
            .http_status,
            Some(503)
        );
        assert_eq!(
            ProviderError::http_failure(
                reqwest::StatusCode::NOT_FOUND,
                "OpenCode Go usage endpoint returned HTTP 404 Not Found.",
            )
            .transient,
            Some(false)
        );
    }

    #[test]
    fn error_wire_format_is_camel_case_and_omits_absent_metadata() {
        let wire = serde_json::to_string(&ProviderError::http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "OpenCode Go usage endpoint returned HTTP 429 Too Many Requests.",
        ))
        .unwrap();
        assert!(wire.contains("\"httpStatus\":429"), "wire: {wire}");
        assert!(wire.contains("\"transient\":true"), "wire: {wire}");

        let wire = serde_json::to_string(&auth_invalid()).unwrap();
        assert!(wire.contains("\"transient\":false"), "wire: {wire}");
        assert!(!wire.contains("httpStatus"), "wire: {wire}");
        // Error messages must not embed the API key.
        assert!(!wire.contains("super-secret-key"), "wire: {wire}");
    }

    /// MIC-297 window shapes: the two real-world quota states from the bug
    /// report (one account exhausted, another partial) must each survive
    /// normalization intact — windows are never merged or averaged.
    #[test]
    fn exhausted_and_partial_states_are_kept_distinct() {
        let body = serde_json::json!({
            "usage": {
                "rolling": { "status": "rate-limited", "percent": 100, "resetsAt": "2026-09-27T19:32:57.751Z" },
                "weekly": { "status": "ok", "percent": 50.0, "resetsAt": "2026-09-28T00:00:00.000Z" },
                "monthly": { "status": "ok", "percent": 50.0, "resetsAt": "2026-10-03T20:48:17.000Z" }
            }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes(), now_utc()).unwrap();
        assert_eq!(limits[0].used_percent, 100.0);
        assert_eq!(limits[1].used_percent, 50.0);
        assert_eq!(limits[2].used_percent, 50.0);
    }

    #[test]
    fn wire_format_is_camel_case_and_never_contains_the_key() {
        let raw = serde_json::json!({ "opencode-go": { "type": "api", "key": "super-secret-key-abc" } })
            .to_string();
        let key = parse_auth(&raw).unwrap();
        assert_eq!(key, "super-secret-key-abc");

        let usage = OpenCodeGoUsage {
            limits: parse_windows(current_shape_body().to_string().as_bytes(), now_utc()).unwrap(),
            account: account_hint(&key),
        };
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(wire.contains("usedPercent"), "wire format must be camelCase: {wire}");
        assert!(wire.contains("resetAt"), "wire format must be camelCase: {wire}");
        assert!(!wire.contains("used_percent"));
        // The masked tail derived from the resolved key must itself be on
        // the wire — otherwise the leak check below could pass on an
        // attribution-free payload.
        assert!(wire.contains("\"keyHint\":\"-abc\""), "wire: {wire}");
        // The credential must never travel toward the WebView.
        assert!(!wire.contains("super-secret-key-abc"));
    }

    /// The only credential-derived data on the wire is the last-four
    /// fingerprint — the same masking the OpenCode console uses — so the
    /// card can name the account without leaking the key.
    #[test]
    fn wire_attribution_carries_only_the_key_tail() {
        let raw = serde_json::json!({ "opencode-go": { "type": "api", "key": "super-secret-key-abc" } })
            .to_string();
        let key = parse_auth(&raw).unwrap();
        let usage = OpenCodeGoUsage {
            limits: parse_windows(current_shape_body().to_string().as_bytes(), now_utc()).unwrap(),
            account: account_hint(&key),
        };
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(wire.contains("\"account\":{\"keyHint\":\"-abc\"}"), "wire: {wire}");

        // No attribution at all when the key is too short to mask.
        let usage = OpenCodeGoUsage {
            limits: Vec::new(),
            account: account_hint("sk-abc12"),
        };
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(!wire.contains("account"), "wire: {wire}");
        assert!(!wire.contains("sk-abc12"), "wire: {wire}");
    }

    #[test]
    #[ignore = "live test: requires a real local OpenCode Go key and network access"]
    fn live_fetch_returns_windows() {
        let usage = tauri::async_runtime::block_on(fetch_opencode_go_usage()).expect("live fetch");
        for limit in &usage.limits {
            println!(
                "{}: used {:.1}% reset_at {:?}",
                limit.label,
                limit.used_percent,
                limit.reset_at.as_deref().unwrap_or("-")
            );
        }
        assert!(!usage.limits.is_empty(), "expected at least one window");
    }
}
