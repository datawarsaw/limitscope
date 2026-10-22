//! Read-only prepaid wallet balances.
//!
//! A wallet is money, not quota: it has no percentage, no reset clock and no
//! limit window. Two prepaid sources are supported:
//!
//! - OpenRouter's prepaid balance from the official credits endpoint
//!   (GET https://openrouter.ai/api/v1/credits):
//!
//!       remainingUsd = data.total_credits - data.total_usage
//!
//! - DeepSeek's prepaid balance from the official balance endpoint
//!   (GET https://api.deepseek.com/user/balance), which reports one entry
//!   per currency (USD, CNY).
//!
//! Money is always carried per currency. Two currencies are never summed and
//! never converted: a wallet holding USD 12.50 and CNY 1.38 states two
//! balances, not one.
//!
//! Credential resolution reuses the credential stores already present on the
//! machine (the OpenCode auth file, the OpenCodex config). No new credential
//! store is introduced, no authentication record is ever modified, the key
//! never leaves this process, and every failure after the credential was
//! resolved is scrubbed exactly like a provider failure.
//!
//! Validation is strict on purpose: a missing, malformed, negative or
//! non-finite amount is an error, never a fabricated 0.00 balance. A real
//! zero (usage equal to credits, or a reported zero balance) is a valid
//! observation and is preserved.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::opencode_go::{self, AuthEntryError};
use crate::provider_error::ProviderError;

pub const OPENROUTER_CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";
const USER_AGENT: &str = concat!("rate-limits/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// One currency's balance inside a wallet.
///
/// Currencies are never merged: a wallet that reports both USD and CNY
/// carries two of these, and no code path adds, averages or converts them.
#[derive(Debug, Clone, PartialEq)]
pub struct WalletAmount {
    /// ISO currency code exactly as the source reported it (`"USD"`,
    /// `"CNY"`). Never assumed and never defaulted to USD.
    pub currency: String,
    /// The remaining amount in `currency`. Always finite and non-negative.
    pub amount: f64,
    /// The source's own breakdown of `amount`, when it reports one
    /// (DeepSeek: the not-expired granted part and the topped-up part).
    /// Absent means the source states no breakdown — never a zero.
    pub granted: Option<f64>,
    /// The topped-up part of `amount`, when the source reports one.
    pub topped_up: Option<f64>,
}

/// One successful balance observation, plus the masked identity of the
/// credential that produced it (used only for last-good retention).
#[derive(Debug, Clone, PartialEq)]
pub struct WalletBalance {
    /// The wallet's balances, one entry per currency, in source order.
    /// Never empty for a successful observation: a wallet with no amount
    /// to state is a failed observation, not an empty balance.
    pub balances: Vec<WalletAmount>,
    /// The source's own "balance sufficient for API calls" verdict, when it
    /// reports one (DeepSeek's `is_available`). Absent means the source
    /// makes no such claim — never `false`.
    pub available: Option<bool>,
    /// Display-safe masked tail (`key:1234`). Never used as an isolation
    /// boundary: short suffixes collide.
    pub identity: Option<String>,
    /// Deterministic fingerprint of the full credential that produced this
    /// observation. This is the account-isolation identity used for last-good
    /// retention and persistence.
    pub credential_binding: Option<String>,
}

/// Deterministic, collision-resistant binding of the full credential.
///
/// The OpenRouter credits response carries no account identifier, so the
/// credential itself is the only verifiable boundary available to this
/// process. A full SHA-256 digest distinguishes credentials that share the
/// four-character display tail, and it is not reversible key material.
pub(crate) fn credential_fingerprint(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    format!("key:sha256:{hex}")
}

/// Fail-closed identity comparison: both sides must be present, non-blank,
/// and exactly equal. A missing binding on either side can never authorize
/// retaining the other side's balance.
pub(crate) fn credential_bindings_match(attempted: Option<&str>, retained: Option<&str>) -> bool {
    match (attempted.map(str::trim), retained.map(str::trim)) {
        (Some(attempted), Some(retained)) => !attempted.is_empty() && attempted == retained,
        _ => false,
    }
}

/// Normalizes a source-reported currency code to the form the wallet data
/// uses: trimmed and upper-cased. A blank code is never invented — the
/// caller rejects it, because an unnamed amount cannot be displayed
/// honestly.
pub(crate) fn normalize_currency(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

/// The single-currency convenience view of a balance: the amount in
/// `currency`, when the wallet holds exactly one currency and it is `wanted`.
///
/// A wallet holding several currencies has no single amount, so this returns
/// `None` — the caller then shows every currency separately instead of
/// picking one or adding them up.
pub(crate) fn single_amount(balances: &[WalletAmount], wanted: &str) -> Option<f64> {
    if balances.len() != 1 {
        return None;
    }
    let only = balances.first()?;
    if only.currency != wanted {
        return None;
    }
    Some(only.amount)
}

// Manual impl so the key can never reach logs through Debug formatting.
struct OpenRouterKey(String);

impl std::fmt::Debug for OpenRouterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OpenRouterKey").field(&"<redacted>").finish()
    }
}

fn auth_file_missing() -> ProviderError {
    ProviderError::new(
        "auth_file_missing",
        "OpenCode auth file not found (auth.json). Open OpenCode or run the OpenCode login flow first.",
    )
}

fn auth_unreadable() -> ProviderError {
    ProviderError::new(
        "auth_unreadable",
        "The OpenCode auth file (auth.json) could not be read or parsed, so the OpenRouter credential cannot be determined.",
    )
}

fn credential_missing() -> ProviderError {
    ProviderError::new(
        "credential_missing",
        "No OpenRouter credential found in the OpenCode auth file (openrouter entry). Sign in to OpenRouter with the OpenCode login flow, then refresh again.",
    )
}

fn credential_ambiguous() -> ProviderError {
    ProviderError::new(
        "credential_ambiguous",
        "The openrouter entry in the OpenCode auth file does not hold a single key, so the balance cannot be attributed. Reconnect OpenRouter once to rewrite it.",
    )
}

fn auth_invalid() -> ProviderError {
    ProviderError::new(
        "auth_invalid",
        "The OpenRouter API key was rejected. Re-authenticate OpenRouter, then refresh again.",
    )
}

fn invalid_response(message: &str) -> ProviderError {
    ProviderError::new("unexpected_response", message)
}

/// Reads one non-negative finite number from the validated data object.
/// Only JSON numbers are accepted: strings, booleans and null are schema
/// changes, not values to coerce.
fn amount_field(data: &Value, field: &str) -> Result<f64, ProviderError> {
    let raw = data.get(field).ok_or_else(|| {
        invalid_response("OpenRouter credits response is missing a required amount field.")
    })?;
    let amount = raw.as_f64().ok_or_else(|| {
        invalid_response("OpenRouter credits response carried a non-numeric amount field.")
    })?;
    if !amount.is_finite() || amount < 0.0 {
        return Err(invalid_response(
            "OpenRouter credits response carried an invalid (negative or non-finite) amount.",
        ));
    }
    Ok(amount)
}

/// Pure response validation: the remaining USD balance, or a structured
/// schema error. Extra fields are ignored; a missing data object or any
/// missing/ill-typed amount invalidates the whole observation.
fn parse_credits(body: &[u8]) -> Result<f64, ProviderError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| {
        invalid_response("OpenRouter credits response was not usable JSON (schema change or sign-in redirect).")
    })?;
    let data = value
        .get("data")
        .filter(|data| data.is_object())
        .ok_or_else(|| {
            invalid_response("OpenRouter credits response format changed; the data object is absent.")
        })?;
    let total_credits = amount_field(data, "total_credits")?;
    let total_usage = amount_field(data, "total_usage")?;
    let remaining = total_credits - total_usage;
    if !remaining.is_finite() || remaining < 0.0 {
        return Err(invalid_response(
            "OpenRouter reported negative or non-finite remaining credit; the observation was discarded.",
        ));
    }
    Ok(remaining)
}

/// The OpenCode auth-file path used for the OpenRouter entry, resolved the
/// same way OpenCode itself resolves it (XDG data home first, then
/// ~/.local/share/opencode).
fn openrouter_auth_path(home: &Path) -> std::path::PathBuf {
    opencode_go::opencode_data_dir(home).join("auth.json")
}

/// Pure credential resolution over an explicit auth-file path. Read-only:
/// nothing is written, and the key is held only for the duration of the
/// fetch.
fn resolve_key_from_path(path: &Path) -> Result<OpenRouterKey, ProviderError> {
    let raw = fs::read_to_string(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => auth_file_missing(),
        _ => ProviderError::new(
            "auth_unreadable",
            format!("Could not read the OpenCode auth file: {error}"),
        ),
    })?;
    opencode_go::parse_auth_entry(&raw, "openrouter")
        .map_err(|error| match error {
            AuthEntryError::Unreadable => auth_unreadable(),
            AuthEntryError::Missing => credential_missing(),
            AuthEntryError::Ambiguous => credential_ambiguous(),
        })
        .map(OpenRouterKey)
}

fn resolve_key(home: &Path) -> Result<OpenRouterKey, ProviderError> {
    resolve_key_from_path(&openrouter_auth_path(home))
}

fn resolve_active_key() -> Result<OpenRouterKey, ProviderError> {
    let home = std::env::home_dir().ok_or_else(|| {
        ProviderError::new(
            "auth_file_missing",
            "Could not locate the user home directory, so the OpenRouter credential cannot be resolved.",
        )
    })?;
    resolve_key(&home)
}

/// Read-only resolution of the active credential's binding.
///
/// Used by the runtime to invalidate a retained balance when the stored
/// credential changed, and by cold-start hydration to prove that a persisted
/// balance still belongs to the credential currently on disk. `None` means
/// "no verifiable identity" and must be treated as a mismatch by callers; it
/// never means "unchanged".
pub fn current_credential_binding() -> Option<String> {
    resolve_active_key()
        .ok()
        .map(|key| credential_fingerprint(&key.0))
}

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

fn scrub_for_key(error: ProviderError, key: &OpenRouterKey) -> ProviderError {
    crate::secret_scrub::scrub_provider_error(error, &[key.0.as_str()])
}

/// The credits fetch for an already-resolved credential. Every failure after
/// this point carries both the display-safe tail and the full-credential
/// binding, so the runtime can tell a balance observed for one credential
/// from a retained balance of another.
async fn fetch_credits_from(
    url: &str,
    key: &OpenRouterKey,
) -> Result<WalletBalance, ProviderError> {
    let identity = opencode_go::key_tail(&key.0).map(|tail| format!("key:{tail}"));
    let binding = Some(credential_fingerprint(&key.0));
    let client = http_client()?;
    let response = client
        .get(url)
        .bearer_auth(&key.0)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|error| {
            crate::provider_error::transport_failure(
                "network",
                format!("Could not reach the OpenRouter credits endpoint: {error}"),
                &error,
            )
            .with_identity_hint(identity.clone())
            .with_credential_binding(binding.clone())
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(auth_invalid()
            .with_identity_hint(identity.clone())
            .with_credential_binding(binding.clone()));
    }
    let retry_after =
        crate::provider_error::retry_after_header(response.headers()).map(String::from);
    let body = response.bytes().await.map_err(|error| {
        crate::provider_error::transport_failure(
            "network",
            format!("OpenRouter credits response was cut short: {error}"),
            &error,
        )
        .with_identity_hint(identity.clone())
        .with_credential_binding(binding.clone())
    })?;
    if !status.is_success() {
        return Err(ProviderError::http_failure_with_retry_after(
            status,
            format!("OpenRouter credits endpoint returned HTTP {status}."),
            retry_after.as_deref(),
            chrono::Utc::now(),
        )
        .with_identity_hint(identity.clone())
        .with_credential_binding(binding.clone()));
    }

    Ok(WalletBalance {
        balances: vec![WalletAmount {
            currency: "USD".to_string(),
            amount: parse_credits(&body).map_err(|error| {
            error
                .with_identity_hint(identity.clone())
                .with_credential_binding(binding.clone())
        })?,
            granted: None,
            topped_up: None,
        }],
        available: None,
        identity,
        credential_binding: binding,
    })
}

/// Production fetch: resolve the OpenRouter credential from the user's own
/// OpenCode auth file, query the official credits endpoint, and scrub any
/// exact occurrence of the key from the failure before it leaves the adapter.
pub async fn get_openrouter_credits() -> Result<WalletBalance, ProviderError> {
    let key = resolve_active_key()?;
    match fetch_credits_from(OPENROUTER_CREDITS_URL, &key).await {
        Ok(balance) => Ok(balance),
        Err(error) => Err(scrub_for_key(error, &key)),
    }
}

// ---------- tests ----------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn credits_body(total_credits: f64, total_usage: f64) -> Vec<u8> {
        serde_json::json!({
            "data": {
                "total_credits": total_credits,
                "total_usage": total_usage
            }
        })
        .to_string()
        .into_bytes()
    }

    fn key(raw: &str) -> OpenRouterKey {
        OpenRouterKey(raw.to_string())
    }

    /// A single USD balance, the shape the OpenRouter collector produces.
    pub(crate) fn usd_balance(amount: f64) -> WalletBalance {
        WalletBalance {
            balances: vec![WalletAmount {
                currency: "USD".to_string(),
                amount,
                granted: None,
                topped_up: None,
            }],
            available: None,
            identity: None,
            credential_binding: None,
        }
    }

    /// Serves exactly one canned HTTP response, then closes. Returns the URL.
    fn serve_once(
        status_line: &'static str,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let mut response = format!(
                "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&body);
        });
        format!("http://{addr}/api/v1/credits")
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "limitscope-wallet-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn valid_response_yields_remaining_balance() {
        assert_eq!(parse_credits(&credits_body(25.5, 10.25)).unwrap(), 15.25);
        assert_eq!(parse_credits(&credits_body(10.0, 10.0)).unwrap(), 0.0);
    }

    #[test]
    fn a_real_zero_balance_is_preserved_not_treated_as_missing() {
        let remaining = parse_credits(&credits_body(7.0, 7.0)).unwrap();
        assert!(remaining == 0.0);
        assert!(remaining.is_finite());
    }

    #[test]
    fn extra_fields_are_ignored() {
        let body = serde_json::json!({
            "data": { "total_credits": 12.0, "total_usage": 2.0, "currency": "USD" },
            "unrelated": [1, 2, 3]
        })
        .to_string();
        assert_eq!(parse_credits(body.as_bytes()).unwrap(), 10.0);
    }

    #[test]
    fn missing_data_object_is_a_schema_error() {
        for body in ["{}", "[]", "null", "\"ok\""] {
            let error = parse_credits(body.as_bytes()).unwrap_err();
            assert_eq!(error.code, "unexpected_response", "body: {body}");
            assert_eq!(error.transient, Some(false));
        }
        let error = parse_credits(br#"{"data": null}"#).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn missing_amount_fields_are_schema_errors() {
        let missing_usage = serde_json::json!({ "data": { "total_credits": 5.0 } });
        let missing_credits = serde_json::json!({ "data": { "total_usage": 5.0 } });
        for body in [missing_usage, missing_credits] {
            assert_eq!(
                parse_credits(body.to_string().as_bytes()).unwrap_err().code,
                "unexpected_response"
            );
        }
    }

    #[test]
    fn non_numeric_amounts_are_rejected_without_coercion() {
        for body in [
            serde_json::json!({ "data": { "total_credits": "10", "total_usage": 1 } }),
            serde_json::json!({ "data": { "total_credits": true, "total_usage": 1 } }),
            serde_json::json!({ "data": { "total_credits": null, "total_usage": 1 } }),
            serde_json::json!({ "data": { "total_credits": [10], "total_usage": 1 } }),
        ] {
            let error = parse_credits(body.to_string().as_bytes()).unwrap_err();
            assert_eq!(error.code, "unexpected_response");
            assert_eq!(error.transient, Some(false));
        }
    }

    #[test]
    fn negative_amounts_and_negative_remaining_are_rejected() {
        for (credits, usage) in [(-1.0, 0.0), (10.0, -1.0), (5.0, 6.0)] {
            let body = credits_body(credits, usage);
            let error = parse_credits(&body).unwrap_err();
            assert_eq!(error.code, "unexpected_response", "{credits} / {usage}");
            assert_eq!(error.transient, Some(false));
        }
    }

    #[test]
    fn non_finite_amounts_are_rejected() {
        // JSON cannot carry NaN/Infinity as literals; an out-of-range number
        // is the wire-level equivalent and must fail the same way.
        let body = br#"{"data":{"total_credits":1e999,"total_usage":0}}"#;
        assert_eq!(
            parse_credits(body).unwrap_err().code,
            "unexpected_response"
        );
    }

    #[test]
    fn malformed_json_is_a_schema_error() {
        let error = parse_credits(b"<html>sign in</html>").unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.transient, Some(false));
    }

    #[test]
    fn auth_file_errors_are_explicit() {
        let dir = temp_dir("auth-missing");
        let missing = resolve_key_from_path(&dir.join("auth.json")).unwrap_err();
        assert_eq!(missing.code, "auth_file_missing");

        let path = dir.join("auth.json");
        fs::write(&path, "not json").unwrap();
        assert_eq!(resolve_key_from_path(&path).unwrap_err().code, "auth_unreadable");

        fs::write(&path, r#"{"openai": {"type": "oauth"}}"#).unwrap();
        assert_eq!(resolve_key_from_path(&path).unwrap_err().code, "credential_missing");

        fs::write(&path, r#"{"openrouter": {"type": "api", "key": "   "}}"#).unwrap();
        assert_eq!(resolve_key_from_path(&path).unwrap_err().code, "credential_missing");

        for malformed in [
            r#"{"openrouter": {"type": "api", "key": ["sk-one", "sk-two"]}}"#,
            r#"{"openrouter": {"type": "api", "key": 42}}"#,
            r#"{"openrouter": {"type": "api", "key": null}}"#,
        ] {
            fs::write(&path, malformed).unwrap();
            assert_eq!(
                resolve_key_from_path(&path).unwrap_err().code,
                "credential_ambiguous",
                "input: {malformed}"
            );
        }

        fs::write(
            &path,
            r#"{"openrouter": {"type": "api", "key": "  sk-or-test-key-1234  "}}"#,
        )
        .unwrap();
        let resolved = resolve_key_from_path(&path).unwrap();
        assert_eq!(resolved.0, "sk-or-test-key-1234");
    }

    #[tokio::test]
    async fn valid_http_response_returns_balance() {
        let url = serve_once("200 OK", vec![], credits_body(30.0, 4.5));
        let balance = fetch_credits_from(&url, &key("sk-or-test-key-1234"))
            .await
            .unwrap();
        assert_eq!(balance.balances.len(), 1);
        assert_eq!(balance.balances[0].currency, "USD");
        assert_eq!(balance.balances[0].amount, 25.5);
        assert_eq!(balance.identity.as_deref(), Some("key:1234"));
    }

    #[tokio::test]
    async fn unauthorized_and_forbidden_map_to_auth_invalid() {
        for status in ["401 Unauthorized", "403 Forbidden"] {
            let url = serve_once(status, vec![], Vec::new());
            let error = fetch_credits_from(&url, &key("sk-or-test-key-1234"))
                .await
                .unwrap_err();
            assert_eq!(error.code, "auth_invalid", "status: {status}");
            assert_eq!(error.transient, Some(false));
            assert_eq!(error.http_status, None);
            assert_eq!(error.identity_hint.as_deref(), Some("key:1234"));
        }
    }

    #[tokio::test]
    async fn rate_limit_honors_retry_after() {
        let url = serve_once(
            "429 Too Many Requests",
            vec![("Retry-After", "60".to_string())],
            Vec::new(),
        );
        let error = fetch_credits_from(&url, &key("sk-or-test-key-1234"))
            .await
            .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.transient, Some(true));
        assert_eq!(error.http_status, Some(429));
        assert_eq!(error.retry_after_ms, Some(60_000));
    }

    #[tokio::test]
    async fn server_error_is_transient() {
        let url = serve_once("503 Service Unavailable", vec![], Vec::new());
        let error = fetch_credits_from(&url, &key("sk-or-test-key-1234"))
            .await
            .unwrap_err();
        assert_eq!(error.transient, Some(true));
        assert_eq!(error.http_status, Some(503));
        assert_eq!(error.retry_after_ms, None);
    }

    #[tokio::test]
    async fn malformed_success_body_is_not_transient() {
        let url = serve_once("200 OK", vec![], b"{}".to_vec());
        let error = fetch_credits_from(&url, &key("sk-or-test-key-1234"))
            .await
            .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.transient, Some(false));
        assert_eq!(error.identity_hint.as_deref(), Some("key:1234"));
    }

    #[tokio::test]
    async fn transport_failure_is_network_and_transient() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let error = fetch_credits_from(&format!("http://{addr}/credits"), &key("sk-or-test-key-1234"))
            .await
            .unwrap_err();
        assert_eq!(error.code, "network");
        assert_eq!(error.transient, Some(true));
        assert_eq!(error.identity_hint.as_deref(), Some("key:1234"));
    }

    #[test]
    fn errors_are_scrubbed_of_the_resolved_key() {
        let key = key("sk-or-super-secret-9876");
        let leaked = ProviderError::new(
            "unexpected",
            "request to https://example.test?key=sk-or-super-secret-9876 failed",
        );
        let scrubbed = scrub_for_key(leaked, &key);
        assert!(!scrubbed.message.contains("sk-or-super-secret-9876"));
        assert!(
            scrubbed
                .message
                .contains(crate::secret_scrub::PLACEHOLDER),
            "{}",
            scrubbed.message
        );
    }

    #[test]
    fn key_debug_is_redacted() {
        let rendered = format!("{:?}", key("sk-or-super-secret-9876"));
        assert!(!rendered.contains("sk-or-super-secret-9876"));
    }

    #[test]
    fn the_credential_fingerprint_is_stable_and_collision_resistant_for_display_tails() {
        let first = credential_fingerprint("sk-or-test-key-1234");
        let again = credential_fingerprint("sk-or-test-key-1234");
        let other = credential_fingerprint("sk-or-other-key-1234");

        assert_eq!(first, again, "the same credential maps to one binding");
        assert_ne!(
            first, other,
            "credentials sharing the four-character display tail stay distinct"
        );
        assert!(first.starts_with("key:sha256:"));
        assert_eq!(first.len(), "key:sha256:".len() + 64);
        assert_eq!(
            opencode_go::key_tail("sk-or-test-key-1234"),
            opencode_go::key_tail("sk-or-other-key-1234"),
            "the display tail alone is not an isolation boundary"
        );
    }

    #[test]
    fn the_binding_comparator_fails_closed() {
        let first = credential_fingerprint("sk-or-test-key-1234");
        let other = credential_fingerprint("sk-or-other-key-1234");

        assert!(credential_bindings_match(Some(&first), Some(&first)));
        assert!(!credential_bindings_match(Some(&first), Some(&other)));
        assert!(!credential_bindings_match(None, Some(&first)));
        assert!(!credential_bindings_match(Some(&first), None));
        assert!(!credential_bindings_match(None, None));
        assert!(!credential_bindings_match(Some(""), Some("")));
        assert!(!credential_bindings_match(Some("   "), Some("   ")));
        assert!(!credential_bindings_match(Some(&first), Some("   ")));
    }

    #[test]
    fn short_or_odd_keys_get_no_identity() {
        let balance = WalletBalance {
            balances: vec![WalletAmount {
                currency: "USD".to_string(),
                amount: 1.0,
                granted: None,
                topped_up: None,
            }],
            available: None,
            identity: opencode_go::key_tail("short").map(|tail| format!("key:{tail}")),
            credential_binding: None,
        };
        assert_eq!(balance.identity, None);
    }

    #[test]
    #[ignore = "live test: requires a real local OpenRouter key and network access"]
    fn live_fetch_returns_balance() {
        let balance = tauri::async_runtime::block_on(get_openrouter_credits())
            .expect("live fetch");
        let amount = single_amount(&balance.balances, "USD").expect("a USD balance");
        println!("remaining USD: {:.2}", amount);
        assert!(amount.is_finite() && amount >= 0.0);
    }
}
