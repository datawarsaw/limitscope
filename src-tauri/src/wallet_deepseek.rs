//! DeepSeek's official prepaid balance.
//!
//! The source is the documented balance endpoint
//! (GET https://api.deepseek.com/user/balance), authenticated with a bearer
//! token. The documented response carries a per-currency list:
//!
//!     {
//!       "is_available": true,
//!       "balance_infos": [
//!         {
//!           "currency": "CNY",
//!           "total_balance": "110.00",
//!           "granted_balance": "10.00",
//!           "topped_up_balance": "100.00"
//!         }
//!       ]
//!     }
//!
//! Two properties of that schema drive the whole design here:
//!
//! - **The amounts are strings.** "110.00" is the documented shape, so a
//!   number is a schema change, not a value to coerce. Amounts are parsed
//!   strictly: a missing, malformed, negative or non-finite amount is an
//!   error, never a fabricated 0.00.
//! - **The currency is per entry, not per response.** A response may carry
//!   USD, CNY or both. Each amount is kept in its own currency; they are
//!   never summed and never converted.
//!
//! Credential resolution reuses the credential stores already present on the
//! machine (the OpenCodex provider config, the OpenCode auth file). Both
//! stores are read-only: nothing is written, and no authentication record is
//! ever created or modified. When no credential exists the wallet reports a
//! clear unavailable state instead of a fabricated balance.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use crate::opencode_go::{self, AuthEntryError};
use crate::provider_error::ProviderError;
use crate::wallet::{normalize_currency, WalletAmount, WalletBalance};

pub const DEEPSEEK_BALANCE_URL: &str = "https://api.deepseek.com/user/balance";
const USER_AGENT: &str = concat!("rate-limits/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The credential source the wallet resolved the key from. Shown in the
/// failure wording so the user knows which store to fix; it never carries
/// key material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepSeekCredentialSource {
    /// The OpenCodex provider config - "providers.deepseek.apiKey".
    OpenCodex,
    /// The OpenCode auth file - the "deepseek" entry.
    OpenCode,
}

// Manual impl so the key can never reach logs through Debug formatting.
struct DeepSeekKey(String);

impl std::fmt::Debug for DeepSeekKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DeepSeekKey").field(&"<redacted>").finish()
    }
}

fn config_file_missing() -> ProviderError {
    ProviderError::new(
        "auth_file_missing",
        "No DeepSeek credential found. Sign in to DeepSeek with OpenCodex or the OpenCode login flow, then refresh again.",
    )
}

fn config_unreadable() -> ProviderError {
    ProviderError::new(
        "auth_unreadable",
        "The OpenCodex config could not be read or parsed, so the DeepSeek credential cannot be determined.",
    )
}

fn credential_missing() -> ProviderError {
    ProviderError::new(
        "credential_missing",
        "No DeepSeek credential found. Sign in to DeepSeek with OpenCodex or the OpenCode login flow, then refresh again.",
    )
}

fn credential_ambiguous() -> ProviderError {
    ProviderError::new(
        "credential_ambiguous",
        "The stored DeepSeek credential does not hold a single key, so the balance cannot be attributed. Reconnect DeepSeek once to rewrite it.",
    )
}

fn auth_invalid() -> ProviderError {
    ProviderError::new(
        "auth_invalid",
        "The DeepSeek API key was rejected. Re-authenticate DeepSeek, then refresh again.",
    )
}

fn invalid_response(message: &str) -> ProviderError {
    ProviderError::new("unexpected_response", message)
}

/// Reads one documented decimal-string amount.
///
/// The official schema carries the amounts as strings ("110.00"). A JSON
/// number is accepted too - the platform has been observed to send both
/// shapes - but a string is parsed strictly: an unparseable, negative or
/// non-finite value is an error, never a zero.
fn amount_field(data: &Value, field: &str) -> Result<f64, ProviderError> {
    let raw = data.get(field).ok_or_else(|| {
        invalid_response("The DeepSeek balance response is missing a required amount field.")
    })?;
    let amount = match raw {
        Value::String(text) => text.trim().parse::<f64>().map_err(|_| {
            invalid_response("The DeepSeek balance response carried a non-numeric amount field.")
        })?,
        Value::Number(number) => number.as_f64().ok_or_else(|| {
            invalid_response("The DeepSeek balance response carried a non-numeric amount field.")
        })?,
        _ => {
            return Err(invalid_response(
                "The DeepSeek balance response carried a non-numeric amount field.",
            ))
        }
    };
    if !amount.is_finite() || amount < 0.0 {
        return Err(invalid_response(
            "The DeepSeek balance response carried an invalid (negative or non-finite) amount.",
        ));
    }
    Ok(amount)
}

/// Pure response validation: the per-currency balances, the source's own
/// availability verdict, or a structured schema error.
///
/// Extra fields are ignored. A missing "balance_infos" array, an entry
/// without a currency, or an entry with a malformed amount invalidates the
/// whole observation: a partially-read balance would be a partial truth
/// about the user's money.
fn parse_balance(body: &[u8]) -> Result<(Vec<WalletAmount>, Option<bool>), ProviderError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| {
        invalid_response(
            "The DeepSeek balance response was not usable JSON (schema change or sign-in redirect).",
        )
    })?;

    // "is_available" is documented as a boolean. A non-boolean is a schema
    // change: the verdict is dropped (never guessed as false), while the
    // balances themselves stay readable.
    let available = match value.get("is_available") {
        Some(Value::Bool(flag)) => Some(*flag),
        _ => None,
    };

    let entries = value
        .get("balance_infos")
        .filter(|entries| entries.is_array())
        .ok_or_else(|| {
            invalid_response(
                "The DeepSeek balance response format changed; the balance_infos array is absent.",
            )
        })?;
    let entries = entries.as_array().expect("filtered to an array above");
    if entries.is_empty() {
        return Err(invalid_response(
            "The DeepSeek balance response carried no balance entries, so no balance can be stated.",
        ));
    }

    let mut balances = Vec::with_capacity(entries.len());
    for entry in entries {
        if !entry.is_object() {
            return Err(invalid_response(
                "The DeepSeek balance response carried a malformed balance entry.",
            ));
        }
        let currency = entry
            .get("currency")
            .and_then(Value::as_str)
            .map(normalize_currency)
            .filter(|currency| !currency.is_empty())
            .ok_or_else(|| {
                invalid_response(
                    "The DeepSeek balance response carried a balance entry without a currency.",
                )
            })?;
        let total = amount_field(entry, "total_balance")?;
        // The granted and topped-up parts are the source's own breakdown of
        // the total. They are optional in the document's shape, and a
        // missing one stays absent - never zero.
        let granted = match entry.get("granted_balance") {
            None | Some(Value::Null) => None,
            Some(_) => Some(amount_field(entry, "granted_balance")?),
        };
        let topped_up = match entry.get("topped_up_balance") {
            None | Some(Value::Null) => None,
            Some(_) => Some(amount_field(entry, "topped_up_balance")?),
        };
        balances.push(WalletAmount {
            currency,
            amount: total,
            granted,
            topped_up,
        });
    }
    Ok((balances, available))
}

/// The OpenCodex provider config path, resolved from the user's home.
fn opencodex_config_path(home: &Path) -> std::path::PathBuf {
    home.join(".opencodex").join("config.json")
}

/// The OpenCode auth file path used for the "deepseek" entry.
fn deepseek_auth_path(home: &Path) -> std::path::PathBuf {
    opencode_go::opencode_data_dir(home).join("auth.json")
}

/// Pure credential resolution over explicit paths. Read-only: nothing is
/// written, and the key is held only for the duration of the fetch.
///
/// Precedence: the OpenCodex provider config wins when it holds a usable
/// DeepSeek key - it is the store the user's own harness routes DeepSeek
/// traffic with, so it names the active account. The OpenCode auth file is
/// the fallback. A malformed OpenCodex config is a hard error rather than a
/// silent fallback, exactly like the OpenCode Go adapter's rule: both
/// stores hold account credentials, and falling back could resurrect a
/// removed account's key as the active one.
fn resolve_key_from_paths(
    config_path: &Path,
    auth_path: &Path,
) -> Result<(DeepSeekKey, DeepSeekCredentialSource), ProviderError> {
    if config_path.exists() {
        let raw = fs::read_to_string(config_path).map_err(|error| {
            ProviderError::new(
                "auth_unreadable",
                format!("Could not read the OpenCodex config: {error}"),
            )
        })?;
        let value: Value = serde_json::from_str(&raw).map_err(|_| config_unreadable())?;
        let key = value
            .get("providers")
            .and_then(|providers| providers.get("deepseek"))
            .filter(|entry| entry.is_object())
            .and_then(|entry| entry.get("apiKey"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|key| !key.is_empty());
        match key {
            Some(key) => {
                return Ok((
                    DeepSeekKey(key.to_string()),
                    DeepSeekCredentialSource::OpenCodex,
                ))
            }
            // A valid config without a DeepSeek entry: OpenCodex is simply
            // not (or no longer) configured for DeepSeek - use OpenCode's.
            None => {}
        }
    }

    if auth_path.exists() {
        let raw = fs::read_to_string(auth_path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => config_file_missing(),
            _ => ProviderError::new(
                "auth_unreadable",
                format!("Could not read the OpenCode auth file: {error}"),
            ),
        })?;
        let key = opencode_go::parse_auth_entry(&raw, "deepseek").map_err(|error| match error {
            AuthEntryError::Unreadable => config_unreadable(),
            AuthEntryError::Missing => credential_missing(),
            AuthEntryError::Ambiguous => credential_ambiguous(),
        })?;
        return Ok((
            DeepSeekKey(key),
            DeepSeekCredentialSource::OpenCode,
        ));
    }

    Err(credential_missing())
}

fn resolve_key(
    home: &Path,
) -> Result<(DeepSeekKey, DeepSeekCredentialSource), ProviderError> {
    resolve_key_from_paths(&opencodex_config_path(home), &deepseek_auth_path(home))
}

fn resolve_active_key() -> Result<(DeepSeekKey, DeepSeekCredentialSource), ProviderError> {
    let home = std::env::home_dir().ok_or_else(|| {
        ProviderError::new(
            "auth_file_missing",
            "Could not locate the user home directory, so the DeepSeek credential cannot be resolved.",
        )
    })?;
    resolve_key(&home)
}

/// Read-only resolution of the active credential's binding.
///
/// Used by the runtime to invalidate a retained balance when the stored
/// credential changed, and by cold-start hydration to prove that a
/// persisted balance still belongs to the credential currently on disk.
/// "None" means "no verifiable identity" and must be treated as a mismatch
/// by callers; it never means "unchanged".
pub fn current_credential_binding() -> Option<String> {
    resolve_active_key()
        .ok()
        .map(|(key, _)| crate::wallet::credential_fingerprint(&key.0))
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

fn scrub_for_key(error: ProviderError, key: &DeepSeekKey) -> ProviderError {
    crate::secret_scrub::scrub_provider_error(error, &[key.0.as_str()])
}

/// The balance fetch for an already-resolved credential. Every failure
/// after this point carries both the display-safe tail and the
/// full-credential binding, so the runtime can tell a balance observed for
/// one credential from a retained balance of another.
async fn fetch_balance_from(
    url: &str,
    key: &DeepSeekKey,
) -> Result<WalletBalance, ProviderError> {
    let identity = opencode_go::key_tail(&key.0).map(|tail| format!("key:{tail}"));
    let binding = Some(crate::wallet::credential_fingerprint(&key.0));
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
                format!("Could not reach the DeepSeek balance endpoint: {error}"),
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
            format!("The DeepSeek balance response was cut short: {error}"),
            &error,
        )
        .with_identity_hint(identity.clone())
        .with_credential_binding(binding.clone())
    })?;
    if !status.is_success() {
        return Err(ProviderError::http_failure_with_retry_after(
            status,
            format!("The DeepSeek balance endpoint returned HTTP {status}."),
            retry_after.as_deref(),
            chrono::Utc::now(),
        )
        .with_identity_hint(identity.clone())
        .with_credential_binding(binding.clone()));
    }

    let (balances, available) = parse_balance(&body).map_err(|error| {
        error
            .with_identity_hint(identity.clone())
            .with_credential_binding(binding.clone())
    })?;
    Ok(WalletBalance {
        balances,
        available,
        identity,
        credential_binding: binding,
    })
}

/// Production fetch: resolve the DeepSeek credential from the user's own
/// harness stores, query the official balance endpoint, and scrub any exact
/// occurrence of the key from the failure before it leaves the adapter.
pub async fn get_deepseek_balance() -> Result<WalletBalance, ProviderError> {
    let (key, _source) = resolve_active_key()?;
    match fetch_balance_from(DEEPSEEK_BALANCE_URL, &key).await {
        Ok(balance) => Ok(balance),
        Err(error) => Err(scrub_for_key(error, &key)),
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn key(raw: &str) -> DeepSeekKey {
        DeepSeekKey(raw.to_string())
    }

    /// The documented example shape, verbatim.
    fn documented_body() -> Vec<u8> {
        serde_json::json!({
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "110.00",
                    "granted_balance": "10.00",
                    "topped_up_balance": "100.00"
                }
            ]
        })
        .to_string()
        .into_bytes()
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
        format!("http://{addr}/user/balance")
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "limitscope-deepseek-wallet-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_documented_example_parses_into_its_currency() {
        let (balances, available) = parse_balance(&documented_body()).unwrap();
        assert_eq!(available, Some(true));
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].currency, "CNY");
        assert_eq!(balances[0].amount, 110.0);
        assert_eq!(balances[0].granted, Some(10.0));
        assert_eq!(balances[0].topped_up, Some(100.0));
    }

    #[test]
    fn a_usd_balance_parses_and_keeps_its_own_currency() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "USD",
                    "total_balance": "1.38",
                    "granted_balance": "0.00",
                    "topped_up_balance": "1.38"
                }
            ]
        })
        .to_string();
        let (balances, available) = parse_balance(body.as_bytes()).unwrap();
        assert_eq!(available, Some(true));
        assert_eq!(balances[0].currency, "USD");
        assert_eq!(balances[0].amount, 1.38);
    }

    #[test]
    fn several_currencies_stay_separate_and_are_never_summed() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [
                { "currency": "USD", "total_balance": "12.50" },
                { "currency": "CNY", "total_balance": "110.00" }
            ]
        })
        .to_string();
        let (balances, _) = parse_balance(body.as_bytes()).unwrap();
        assert_eq!(balances.len(), 2);
        assert_eq!(balances[0].currency, "USD");
        assert_eq!(balances[0].amount, 12.5);
        assert_eq!(balances[1].currency, "CNY");
        assert_eq!(balances[1].amount, 110.0);
        // The single-currency view refuses to pick one of several.
        assert_eq!(crate::wallet::single_amount(&balances, "USD"), None);
    }

    #[test]
    fn a_real_zero_balance_is_preserved_not_treated_as_missing() {
        let body = serde_json::json!({
            "is_available": false,
            "balance_infos": [
                { "currency": "USD", "total_balance": "0.00" }
            ]
        })
        .to_string();
        let (balances, available) = parse_balance(body.as_bytes()).unwrap();
        assert_eq!(available, Some(false));
        assert_eq!(balances[0].amount, 0.0);
        assert!(balances[0].amount.is_finite());
    }

    #[test]
    fn a_missing_breakdown_stays_absent_and_never_becomes_zero() {
        let body = serde_json::json!({
            "balance_infos": [
                { "currency": "USD", "total_balance": "5.00" }
            ]
        })
        .to_string();
        let (balances, available) = parse_balance(body.as_bytes()).unwrap();
        assert_eq!(available, None, "no availability verdict is claimed");
        assert_eq!(balances[0].granted, None);
        assert_eq!(balances[0].topped_up, None);
        assert_eq!(balances[0].amount, 5.0);
    }

    #[test]
    fn a_missing_balance_infos_array_is_a_schema_error() {
        for body in ["{}", "[]", "null", "\"ok\"", "{\"is_available\":true}"] {
            let error = parse_balance(body.as_bytes()).unwrap_err();
            assert_eq!(error.code, "unexpected_response", "body: {body}");
            assert_eq!(error.transient, Some(false));
        }
        let error = parse_balance(br#"{"balance_infos":null}"#).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn an_empty_balance_array_states_no_balance_rather_than_zero() {
        let body = serde_json::json!({ "is_available": false, "balance_infos": [] });
        let error = parse_balance(body.to_string().as_bytes()).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.transient, Some(false));
    }

    #[test]
    fn a_missing_currency_is_a_schema_error() {
        for body in [
            serde_json::json!({ "balance_infos": [{ "total_balance": "1.00" }] }),
            serde_json::json!({ "balance_infos": [{ "currency": "", "total_balance": "1.00" }] }),
            serde_json::json!({ "balance_infos": [{ "currency": 42, "total_balance": "1.00" }] }),
        ] {
            let error = parse_balance(body.to_string().as_bytes()).unwrap_err();
            assert_eq!(error.code, "unexpected_response", "body: {body}");
        }
    }

    #[test]
    fn a_missing_total_balance_is_a_schema_error_not_zero() {
        let body = serde_json::json!({ "balance_infos": [{ "currency": "USD" }] });
        let error = parse_balance(body.to_string().as_bytes()).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn a_malformed_amount_is_rejected_without_coercion() {
        for body in [
            serde_json::json!({ "balance_infos": [{ "currency": "USD", "total_balance": "abc" }] }),
            serde_json::json!({ "balance_infos": [{ "currency": "USD", "total_balance": "" }] }),
            serde_json::json!({ "balance_infos": [{ "currency": "USD", "total_balance": null }] }),
            serde_json::json!({ "balance_infos": [{ "currency": "USD", "total_balance": true }] }),
            serde_json::json!({ "balance_infos": [{ "currency": "USD", "total_balance": [1] }] }),
            serde_json::json!({ "balance_infos": [{ "currency": "USD", "total_balance": "NaN" }] }),
            serde_json::json!({ "balance_infos": [{ "currency": "USD", "total_balance": "Infinity" }] }),
        ] {
            let error = parse_balance(body.to_string().as_bytes()).unwrap_err();
            assert_eq!(error.code, "unexpected_response", "body: {body}");
            assert_eq!(error.transient, Some(false));
        }
    }

    #[test]
    fn a_negative_amount_is_rejected() {
        let body = serde_json::json!({
            "balance_infos": [{ "currency": "USD", "total_balance": "-1.00" }]
        })
        .to_string();
        let error = parse_balance(body.as_bytes()).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn a_malformed_granted_amount_invalidates_the_whole_observation() {
        let body = serde_json::json!({
            "balance_infos": [
                {
                    "currency": "USD",
                    "total_balance": "5.00",
                    "granted_balance": "oops"
                }
            ]
        })
        .to_string();
        let error = parse_balance(body.as_bytes()).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
    }

    #[test]
    fn a_non_finite_availability_flag_is_dropped_not_guessed() {
        let body = serde_json::json!({
            "is_available": "yes",
            "balance_infos": [{ "currency": "USD", "total_balance": "5.00" }]
        })
        .to_string();
        let (balances, available) = parse_balance(body.as_bytes()).unwrap();
        assert_eq!(available, None);
        assert_eq!(balances[0].amount, 5.0);
    }

    #[test]
    fn malformed_json_is_a_schema_error() {
        let error = parse_balance(b"<html>sign in</html>").unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.transient, Some(false));
    }

    #[test]
    fn extra_fields_are_ignored() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [{ "currency": "USD", "total_balance": "5.00" }],
            "unrelated": [1, 2, 3]
        })
        .to_string();
        let (balances, _) = parse_balance(body.as_bytes()).unwrap();
        assert_eq!(balances[0].amount, 5.0);
    }

    #[test]
    fn credential_resolution_prefers_the_opencodex_config() {
        let dir = temp_dir("precedence");
        let config = dir.join("config.json");
        let auth = dir.join("auth.json");
        fs::write(
            &config,
            r#"{"providers":{"deepseek":{"apiKey":"sk-deepseek-config-1234"}}}"#,
        )
        .unwrap();
        fs::write(
            &auth,
            r#"{"deepseek":{"type":"api","key":"sk-deepseek-auth-5678"}}"#,
        )
        .unwrap();
        let (resolved, source) = resolve_key_from_paths(&config, &auth).unwrap();
        assert_eq!(source, DeepSeekCredentialSource::OpenCodex);
        assert_eq!(resolved.0, "sk-deepseek-config-1234");
    }

    #[test]
    fn credential_resolution_falls_back_to_the_opencode_auth_file() {
        let dir = temp_dir("fallback");
        let config = dir.join("config.json");
        let auth = dir.join("auth.json");
        // A valid config without a DeepSeek entry: OpenCodex is simply not
        // configured for DeepSeek, so OpenCode's file is used.
        fs::write(&config, r#"{"providers":{"openrouter":{"apiKey":"sk-or-1234"}}}"#)
            .unwrap();
        fs::write(
            &auth,
            r#"{"deepseek":{"type":"api","key":"  sk-deepseek-auth-5678  "}}"#,
        )
        .unwrap();
        let (resolved, source) = resolve_key_from_paths(&config, &auth).unwrap();
        assert_eq!(source, DeepSeekCredentialSource::OpenCode);
        assert_eq!(resolved.0, "sk-deepseek-auth-5678");
    }

    #[test]
    fn a_missing_credential_is_reported_as_missing() {
        let dir = temp_dir("missing");
        let error = resolve_key_from_paths(&dir.join("config.json"), &dir.join("auth.json"))
            .unwrap_err();
        assert_eq!(error.code, "credential_missing");
        assert_eq!(error.transient, Some(false));
    }

    #[test]
    fn a_malformed_config_is_a_hard_error_not_a_silent_fallback() {
        let dir = temp_dir("malformed-config");
        let config = dir.join("config.json");
        let auth = dir.join("auth.json");
        fs::write(&config, "not json").unwrap();
        fs::write(
            &auth,
            r#"{"deepseek":{"type":"api","key":"sk-deepseek-auth-5678"}}"#,
        )
        .unwrap();
        let error = resolve_key_from_paths(&config, &auth).unwrap_err();
        assert_eq!(error.code, "auth_unreadable");
    }

    #[test]
    fn an_ambiguous_auth_entry_is_refused() {
        let dir = temp_dir("ambiguous");
        let config = dir.join("config.json");
        let auth = dir.join("auth.json");
        for malformed in [
            r#"{"deepseek":{"type":"api","key":["sk-one","sk-two"]}}"#,
            r#"{"deepseek":{"type":"api","key":42}}"#,
            r#"{"deepseek":{"type":"api","key":null}}"#,
        ] {
            fs::write(&auth, malformed).unwrap();
            let error = resolve_key_from_paths(&config, &auth).unwrap_err();
            assert_eq!(error.code, "credential_ambiguous", "input: {malformed}");
        }
    }

    #[test]
    fn a_blank_auth_key_counts_as_missing() {
        let dir = temp_dir("blank");
        let config = dir.join("config.json");
        let auth = dir.join("auth.json");
        fs::write(&auth, r#"{"deepseek":{"type":"api","key":"   "}}"#).unwrap();
        let error = resolve_key_from_paths(&config, &auth).unwrap_err();
        assert_eq!(error.code, "credential_missing");
    }

    #[tokio::test]
    async fn a_valid_http_response_returns_the_balances() {
        let url = serve_once("200 OK", vec![], documented_body());
        let balance = fetch_balance_from(&url, &key("sk-deepseek-test-key-1234"))
            .await
            .unwrap();
        assert_eq!(balance.balances[0].currency, "CNY");
        assert_eq!(balance.balances[0].amount, 110.0);
        assert_eq!(balance.available, Some(true));
        assert_eq!(balance.identity.as_deref(), Some("key:1234"));
    }

    #[tokio::test]
    async fn unauthorized_and_forbidden_map_to_auth_invalid() {
        for status in ["401 Unauthorized", "403 Forbidden"] {
            let url = serve_once(status, vec![], Vec::new());
            let error = fetch_balance_from(&url, &key("sk-deepseek-test-key-1234"))
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
        let error = fetch_balance_from(&url, &key("sk-deepseek-test-key-1234"))
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
        let error = fetch_balance_from(&url, &key("sk-deepseek-test-key-1234"))
            .await
            .unwrap_err();
        assert_eq!(error.transient, Some(true));
        assert_eq!(error.http_status, Some(503));
        assert_eq!(error.retry_after_ms, None);
    }

    #[tokio::test]
    async fn a_malformed_success_body_is_not_transient() {
        let url = serve_once("200 OK", vec![], b"{}".to_vec());
        let error = fetch_balance_from(&url, &key("sk-deepseek-test-key-1234"))
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
        let error = fetch_balance_from(
            &format!("http://{addr}/user/balance"),
            &key("sk-deepseek-test-key-1234"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "network");
        assert_eq!(error.transient, Some(true));
        assert_eq!(error.identity_hint.as_deref(), Some("key:1234"));
    }

    #[test]
    fn errors_are_scrubbed_of_the_resolved_key() {
        let key = key("sk-deepseek-super-secret-9876");
        let leaked = ProviderError::new(
            "unexpected",
            "request to https://api.deepseek.com/user/balance?key=sk-deepseek-super-secret-9876 failed",
        );
        let scrubbed = scrub_for_key(leaked, &key);
        assert!(!scrubbed.message.contains("sk-deepseek-super-secret-9876"));
        assert!(scrubbed.message.contains(crate::secret_scrub::PLACEHOLDER));
    }

    #[test]
    fn key_debug_is_redacted() {
        let rendered = format!("{:?}", key("sk-deepseek-super-secret-9876"));
        assert!(!rendered.contains("sk-deepseek-super-secret-9876"));
    }

    #[test]
    fn the_credential_fingerprint_is_stable_and_distinguishes_equal_tails() {
        let first = crate::wallet::credential_fingerprint("sk-deepseek-test-key-1234");
        let again = crate::wallet::credential_fingerprint("sk-deepseek-test-key-1234");
        let other = crate::wallet::credential_fingerprint("sk-deepseek-other-key-1234");
        assert_eq!(first, again);
        assert_ne!(first, other);
        assert!(first.starts_with("key:sha256:"));
    }

    #[test]
    fn currency_normalization_trims_and_upper_cases() {
        assert_eq!(normalize_currency(" usd "), "USD");
        assert_eq!(normalize_currency("Cny"), "CNY");
        assert_eq!(normalize_currency("  "), "");
    }

    #[test]
    #[ignore = "live test: requires a real local DeepSeek key and network access"]
    fn live_fetch_returns_balances() {
        let balance = tauri::async_runtime::block_on(get_deepseek_balance())
            .expect("live fetch");
        for amount in &balance.balances {
            println!("{}: {:.2}", amount.currency, amount.amount);
            assert!(amount.amount.is_finite() && amount.amount >= 0.0);
        }
    }
}
