//! ZCode coding-plan reset cards — passive observation (production).
//!
//! Observes ZCode's free reset cards via
//! `GET https://zcode.z.ai/api/v1/coding-plan/reset/status` — the endpoint the
//! ZCode app itself calls (request shape verified 2026-10-03 from the
//! installed app's `getCodingPlanResetStatus`,
//! `resolveCodingPlanResetAuthorization`, and
//! `createCodingPlanResetHeaders`; live-proven the same day through this
//! exact code path).
//!
//! Passive credential contract — ZCode owns login, refresh, rotation, and all
//! credential-store writes; this module only ever reads:
//! - reads `~/.zcode/v2/credentials.json` read-only, once per attempt;
//! - decrypts `zcodejwttoken` and `oauth:zai:access_token` in this process
//!   with the app-derived `enc:v1` key (the same scheme as `zai.rs`) — no
//!   browser-cookie or OS-keychain extraction, no refresh, no rotation, no
//!   retry loop: exactly one GET per invocation;
//! - sends `Authorization: Bearer <zcodejwttoken>` and
//!   `X-Bigmodel-Authorization: <oauth:zai:access_token>` (the app sends the
//!   access token raw, without a Bearer prefix) plus
//!   `Bigmodel-Target-Type: PERSONAL`;
//! - the HTTP client is the shared `zai::http_client()` — redirects are
//!   refused (`Policy::none()`), so an authenticated request can never be
//!   relayed elsewhere.
//!
//! The module's ONLY upstream URL is the read-only status GET. The endpoint's
//! mutating siblings (spending a card, requesting an opportunity, marking
//! history read) are deliberately absent from this source; a test below pins
//! that invariant against the module text itself. Raw response bodies are
//! never logged: body bytes flow only into the parser, and errors carry at
//! most a sanitized envelope message or an HTTP status.
//!
//! Wire exposure rides the Z.ai provider's snapshot entry
//! (`ProviderUsageDto::zcode_reset_cards`), refreshed by the ordinary runtime
//! cycle — no second scheduler, no tauri command of its own. A failed
//! observation never fails the Z.ai quota refresh, and a fresh observation is
//! retained (within the TTL below) when a later observation fails, mirroring
//! the Codex reset-credit freshness convention.
//!
//! Semantics note: these are ZCode reset CARDS — specific grants for a named
//! window with their own expiry. They are deliberately NOT merged into the
//! Codex reset-credit shape (an explicitly reported banked balance), and
//! there is no cross-provider "available resets" DTO.

use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::provider_error::ProviderError;
use crate::zai::{
    auth_invalid, credential_missing, credential_secret, decrypt_credential, epoch_ms_to_rfc3339,
    http_client, not_entitled, sanitize_upstream_detail, zcode_dir, zcode_not_installed,
};

const RESET_STATUS_URL: &str = "https://zcode.z.ai/api/v1/coding-plan/reset/status";

/// Freshness budget for a retained reset-card observation.
///
/// Rationale (mirrors `codex::RESET_CREDITS_TTL_SECS`): the status endpoint is
/// re-queried live every refresh cycle (default 5 minutes), and a card can be
/// consumed by the ZCode app itself at any time — visible only on the next
/// fetch. An observation older than roughly three default cycles must stop
/// reading as current rather than risk presenting a spent card as available.
pub const OBSERVATION_TTL_SECS: i64 = 15 * 60;
/// Clock-skew tolerance for observation timestamps (5 minutes), matching the
/// Codex credit observation and the persisted last-good store.
const OBSERVED_FUTURE_TOLERANCE_SECS: i64 = 300;

// ---------- data returned to the WebView (camelCase on the wire) ----------

/// One ZCode reset card: a specific grant for a named window with its own
/// expiry — not a fungible credit balance.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodeResetCard {
    pub target: ZCodeResetTarget,
    /// RFC 3339 UTC timestamp derived from the upstream `expire_at` value.
    /// `None` when the upstream stamp is missing or implausible — the card
    /// itself still counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ZCodeResetTarget {
    FiveHour,
    Weekly,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodeResetStatus {
    pub five_hour_cards: Vec<ZCodeResetCard>,
    pub weekly_cards: Vec<ZCodeResetCard>,
    /// Internal observation stamp (never serialized): when this status was
    /// actually observed upstream. The retention TTL and the runtime's
    /// stale-trim compare against it, so a retained observation cannot read
    /// as fresher than it is. Parsed payloads are stamped by the fetch entry.
    #[serde(skip)]
    pub observed_at: DateTime<Utc>,
}

/// The safe facts the live proof records: counts and presence booleans only —
/// no credential material, no account identifiers, no raw response bodies.
#[derive(Debug, Clone, Copy)]
pub struct ResetStatusFacts {
    pub five_hour_count: usize,
    pub weekly_count: usize,
    /// At least one card in the bucket carried a usable expiry stamp.
    pub five_hour_expiries_present: bool,
    pub weekly_expiries_present: bool,
    /// Any of `latest_five_hour_reset_history`, `latest_week_reset_history`,
    /// `has_unread_history` appeared in the upstream payload.
    pub history_fields_present: bool,
}

// ---------- credentials (passive, backend-only) ----------

/// The zcode.z.ai session JWT (`zcodejwttoken` store entry).
struct ZcodeSessionJwt(String);

/// The Z.ai OAuth access token (`oauth:zai:access_token` store entry), sent
/// raw in `X-Bigmodel-Authorization`.
struct ZaiAccessToken(String);

impl std::fmt::Debug for ZcodeSessionJwt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ZcodeSessionJwt").field(&"<redacted>").finish()
    }
}

impl std::fmt::Debug for ZaiAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ZaiAccessToken").field(&"<redacted>").finish()
    }
}

/// The two credentials GET /reset/status needs. `Debug` is redacted through
/// the member impls, and the manual `Serialize` impl below always fails so no
/// generic serde path can ever carry credential material toward the wire.
#[derive(Debug)]
struct ResetCredentials {
    zcode_session_jwt: ZcodeSessionJwt,
    zai_access_token: ZaiAccessToken,
}

impl Serialize for ResetCredentials {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(<S::Error as serde::ser::Error>::custom(
            "ZCode reset credentials never cross the wire",
        ))
    }
}

/// Strictly read-only store read. ZCode stays the only writer of the file.
fn read_store_string(path: &Path) -> Result<String, ProviderError> {
    match fs::read_to_string(path) {
        Ok(raw) => Ok(raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(zcode_not_installed())
        }
        Err(error) => Err(ProviderError::new(
            "auth_unreadable",
            format!("Could not read the ZCode credential store: {error}"),
        )),
    }
}

/// Reads the credential store once and decrypts the two reset-status
/// credentials in this process.
fn load_reset_credentials() -> Result<ResetCredentials, ProviderError> {
    let path = zcode_dir()?.join("credentials.json");
    parse_reset_credentials(&read_store_string(&path)?, &credential_secret()?)
}

/// Extracts and decrypts the two reset-status credentials. Both entries are
/// mandatory for this endpoint (the app refuses to call it without either).
fn parse_reset_credentials(raw: &str, secret: &str) -> Result<ResetCredentials, ProviderError> {
    let value: Value = serde_json::from_str(raw).map_err(|_| {
        ProviderError::new(
            "auth_unreadable",
            "ZCode credential store (credentials.json) is not valid JSON.",
        )
    })?;
    let Value::Object(entries) = value else {
        return Err(credential_missing());
    };
    let load = |key: &str| -> Result<String, ProviderError> {
        let stored = entries
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|stored| !stored.is_empty())
            .ok_or_else(|| {
                ProviderError::new(
                    "credential_missing",
                    format!(
                        "The ZCode credential store has no usable `{key}` entry. Sign in to ZCode first."
                    ),
                )
            })?;
        decrypt_credential(stored, secret)
    };
    Ok(ResetCredentials {
        zcode_session_jwt: ZcodeSessionJwt(load("zcodejwttoken")?),
        zai_access_token: ZaiAccessToken(load("oauth:zai:access_token")?),
    })
}

/// The exact header set the ZCode app sends for this endpoint. A team context
/// would additionally carry `Bigmodel-Organization`/`Bigmodel-Project`; no
/// team context can be derived passively from the store, so personal scope is
/// the only honest assertion.
fn auth_headers(credentials: &ResetCredentials) -> Result<Vec<(String, String)>, ProviderError> {
    let jwt = credentials.zcode_session_jwt.0.trim();
    if jwt.is_empty() {
        return Err(credential_missing());
    }
    // The prefix test reads bytes, never a `str` slice: the store value
    // arrives decrypted-but-unvalidated (plaintext entries pass through), so
    // a multibyte character straddling byte 7 is possible, and a `str` slice
    // there panics with a message that embeds the credential itself.
    let bearer = if jwt
        .as_bytes()
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"bearer "))
    {
        jwt.to_string()
    } else {
        format!("Bearer {jwt}")
    };
    let access = credentials.zai_access_token.0.trim();
    if access.is_empty() {
        return Err(credential_missing());
    }
    Ok(vec![
        ("authorization".to_string(), bearer),
        ("x-bigmodel-authorization".to_string(), access.to_string()),
        ("bigmodel-target-type".to_string(), "PERSONAL".to_string()),
    ])
}

fn auth_header_map(
    credentials: &ResetCredentials,
) -> Result<reqwest::header::HeaderMap, ProviderError> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in auth_headers(credentials)? {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .expect("hardcoded header names are valid");
        let value = reqwest::header::HeaderValue::from_str(&value).map_err(|_| {
            ProviderError::new(
                "auth_unreadable",
                "ZCode credential material is not header-safe.",
            )
        })?;
        headers.insert(name, value);
    }
    Ok(headers)
}

// ---------- upstream response (only what we consume; extra ignored) ----------
//
// Wire shape verified from the installed ZCode app (its zod schema for
// /reset/status): envelope `{ code, msg?, data? }` with code 0 = success, and
// `data`:
// {
//   available_five_hour_resets:     [{ expire_at: number > 0 }],
//   available_week_resets:          [{ expire_at: number > 0 }],
//   latest_five_hour_reset_history: { used_at: number } | null,
//   latest_week_reset_history:      { used_at: number } | null,
//   has_unread_history:             boolean
// }

#[derive(Debug, serde::Deserialize)]
struct ResetCardEntry {
    #[serde(default, rename = "expire_at")]
    expire_at: Option<Value>,
}

#[derive(Debug, serde::Deserialize)]
struct ResetStatusData {
    #[serde(default, rename = "available_five_hour_resets")]
    available_five_hour_resets: Option<Vec<ResetCardEntry>>,
    #[serde(default, rename = "available_week_resets")]
    available_week_resets: Option<Vec<ResetCardEntry>>,
    // Unknown upstream fields (history entries, future additions) are
    // tolerated and never parsed.
    #[serde(flatten)]
    _extra: Value,
}

/// The zcode.z.ai envelope answers business failures with HTTP 200 and a
/// non-zero `code` (0 = success) — the same pattern the api.z.ai monitor
/// endpoint uses with 200/401/403, but with a different success code.
///
/// The envelope `msg` is provider-controlled free text that would be embedded
/// in a user-visible message, so it passes the same display-safety gate the
/// Z.ai monitor errors use (`sanitize_upstream_detail`) and degrades to an
/// authored sentence when nothing visible remains.
fn envelope_error(code: i64, body: &Value) -> Option<ProviderError> {
    if code == 0 {
        return None;
    }
    let detail = body
        .get("msg")
        .and_then(Value::as_str)
        .and_then(sanitize_upstream_detail)
        .unwrap_or_else(|| "request failed".to_string());
    Some(match code {
        401 => auth_invalid(),
        403 => not_entitled(),
        _ => ProviderError::new(
            "unexpected_response",
            format!("ZCode reset-status endpoint rejected the request (code {code}): {detail}"),
        )
        .with_transient(code == 429 || (500..=599).contains(&code)),
    })
}

fn cards(entries: Option<Vec<ResetCardEntry>>, target: ZCodeResetTarget) -> Vec<ZCodeResetCard> {
    entries
        .unwrap_or_default()
        .into_iter()
        .map(|entry| ZCodeResetCard {
            target,
            expires_at: entry
                .expire_at
                .as_ref()
                .and_then(epoch_ms_to_rfc3339)
                .map(|reset| reset.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        })
        .collect()
}

fn parse_reset_status(body: &[u8]) -> Result<(ZCodeResetStatus, ResetStatusFacts), ProviderError> {
    let parsed: Value = serde_json::from_slice(body).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "ZCode reset-status response was not usable JSON (schema change).",
        )
    })?;
    let Some(code) = parsed.get("code").and_then(Value::as_i64) else {
        return Err(ProviderError::new(
            "unexpected_response",
            "ZCode reset-status response has no envelope code (schema change).",
        ));
    };
    if let Some(error) = envelope_error(code, &parsed) {
        return Err(error);
    }
    let Some(data) = parsed.get("data") else {
        return Err(ProviderError::new(
            "unexpected_response",
            "ZCode reset-status response has no data payload (schema change).",
        ));
    };
    let Some(data_object) = data.as_object() else {
        return Err(ProviderError::new(
            "unexpected_response",
            "ZCode reset-status data payload is not an object (schema change).",
        ));
    };
    let payload: ResetStatusData = serde_json::from_value(data.clone()).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "ZCode reset-status response format changed; reset arrays are not lists.",
        )
    })?;
    let five_hour_cards = cards(payload.available_five_hour_resets, ZCodeResetTarget::FiveHour);
    let weekly_cards = cards(payload.available_week_resets, ZCodeResetTarget::Weekly);
    let facts = ResetStatusFacts {
        five_hour_count: five_hour_cards.len(),
        weekly_count: weekly_cards.len(),
        five_hour_expiries_present: five_hour_cards
            .iter()
            .any(|card| card.expires_at.is_some()),
        weekly_expiries_present: weekly_cards.iter().any(|card| card.expires_at.is_some()),
        history_fields_present: data_object.contains_key("latest_five_hour_reset_history")
            || data_object.contains_key("latest_week_reset_history")
            || data_object.contains_key("has_unread_history"),
    };
    Ok((
        ZCodeResetStatus {
            five_hour_cards,
            weekly_cards,
            // Placeholder stamp; `retain_observation` (the freshness
            // authority) re-stamps with the observation clock.
            observed_at: Utc::now(),
        },
        facts,
    ))
}

// ---------- HTTP ----------

/// One passive observation: read the credentials from the store, then exactly
/// one GET against the status URL. No retry, no candidate loop, no refresh:
/// on an auth refusal the caller sees the error and ZCode remains the only
/// writer of the credential store.
async fn fetch_status_facts(
    client: &reqwest::Client,
    url: &str,
    credentials: &ResetCredentials,
) -> Result<(ZCodeResetStatus, ResetStatusFacts), ProviderError> {
    let response = client
        .get(url)
        .headers(auth_header_map(credentials)?)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|error| {
            ProviderError::transient(
                "network",
                format!("Could not reach the ZCode reset-status endpoint: {error}"),
            )
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(auth_invalid());
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(not_entitled());
    }
    let body = response.bytes().await.map_err(|error| {
        ProviderError::transient(
            "network",
            format!("ZCode reset-status response was cut short: {error}"),
        )
    })?;
    if !status.is_success() {
        return Err(ProviderError::http_failure(
            status,
            format!("ZCode reset-status endpoint returned HTTP {status}."),
        ));
    }
    parse_reset_status(&body)
}

/// Injectable-core fetch: one GET against `url` with `credentials`. Tests use
/// it to exercise transport behavior hermetically (redirect refusal,
/// connection refusal) without touching the real store.
async fn fetch_reset_status_from(
    client: &reqwest::Client,
    url: &str,
    credentials: &ResetCredentials,
) -> Result<ZCodeResetStatus, ProviderError> {
    // Final-boundary value scrub (`secret_scrub`): both credential values are
    // alive for this whole fetch, so any exact occurrence of either in a
    // failure message (e.g. an upstream envelope echoing the session JWT
    // back) is removed before the error leaves the adapter.
    fetch_status_facts(client, url, credentials)
        .await
        .map(|(status, _)| status)
        .map_err(|error| {
            crate::secret_scrub::scrub_provider_error(
                error,
                &[
                    credentials.zcode_session_jwt.0.as_str(),
                    credentials.zai_access_token.0.as_str(),
                ],
            )
        })
}

// ---------- retention (last-good for the observation) ----------

/// The last successful observation, retained so a transient observation
/// failure cannot erase fresh reset-card data while it is still within its
/// freshness budget. In-memory only, process-local, never persisted.
static LAST_OBSERVATION: OnceLock<Mutex<Option<ZCodeResetStatus>>> = OnceLock::new();

fn last_observation() -> &'static Mutex<Option<ZCodeResetStatus>> {
    LAST_OBSERVATION.get_or_init(|| Mutex::new(None))
}

/// True when an observation timestamp is still within its freshness budget:
/// not future-dated beyond clock-skew tolerance and not older than the TTL.
/// The same predicate the runtime's stale-trim applies to retained entries.
pub fn cards_fresh(observed_at: &DateTime<Utc>, now: DateTime<Utc>) -> bool {
    let age = now.signed_duration_since(*observed_at).num_seconds();
    (-OBSERVED_FUTURE_TOLERANCE_SECS..=OBSERVATION_TTL_SECS).contains(&age)
}

/// Retention core (pure, testable): a fresh observation replaces the cache
/// and passes through; a failure passes through unless a cached observation
/// is still within its freshness budget, which stands in as the last-good
/// result — a spent card can thus never read as available, but a transient
/// observation failure also cannot erase data that was current moments ago.
fn retain_observation(
    cache: &mut Option<ZCodeResetStatus>,
    result: Result<ZCodeResetStatus, ProviderError>,
    now: DateTime<Utc>,
) -> Result<ZCodeResetStatus, ProviderError> {
    match result {
        Ok(mut status) => {
            status.observed_at = now;
            *cache = Some(status.clone());
            Ok(status)
        }
        Err(error) => {
            let fresh = cache
                .as_ref()
                .is_some_and(|cards| cards_fresh(&cards.observed_at, now));
            if fresh {
                Ok(cache.clone().expect("freshness checked above"))
            } else {
                *cache = None;
                Err(error)
            }
        }
    }
}

/// The production entry point, called once per Z.ai provider job inside the
/// ordinary runtime cycle: one passive GET, then retention. Never fails the
/// caller's quota path — the runtime consumes this `Result` independently.
pub async fn fetch_reset_status() -> Result<ZCodeResetStatus, ProviderError> {
    let result = match load_reset_credentials() {
        Ok(credentials) => {
            fetch_reset_status_from(http_client()?, RESET_STATUS_URL, &credentials).await
        }
        Err(error) => Err(error),
    };
    let mut cache = last_observation().lock().unwrap();
    retain_observation(&mut cache, result, Utc::now())
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zai::{ENC_IV_LEN, ENC_PREFIX, ENC_TAG_LEN};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use sha2::Digest as _;

    const TEST_SECRET: &str = "unit-test-secret";

    /// Seals a plaintext the same way the ZCode app does (mirrors the helper
    /// in `zai.rs` tests) so store-format compatibility is covered end to end.
    fn seal(plaintext: &str, iv: &[u8]) -> String {
        let key = sha2::Sha256::digest(TEST_SECRET.as_bytes());
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let sealed = cipher
            .encrypt(Nonce::from_slice(iv), plaintext.as_bytes())
            .unwrap();
        let (ciphertext, tag) = sealed.split_at(sealed.len() - ENC_TAG_LEN);
        format!(
            "{ENC_PREFIX}{}.{}.{}",
            URL_SAFE_NO_PAD.encode(iv),
            URL_SAFE_NO_PAD.encode(tag),
            URL_SAFE_NO_PAD.encode(ciphertext)
        )
    }

    /// Live-verified shape (from the installed app's zod schema) with two
    /// five-hour cards, one weekly card, and the history fields present.
    fn status_body() -> Value {
        serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": {
                "available_five_hour_resets": [
                    { "expire_at": 1792137600000_i64 },
                    { "expire_at": 1792224000000_i64 }
                ],
                "available_week_resets": [
                    { "expire_at": 1792742400000_i64 }
                ],
                "latest_five_hour_reset_history": null,
                "latest_week_reset_history": null,
                "has_unread_history": false
            }
        })
    }

    fn credential_body(zcode_jwt: &str, access_token: &str) -> String {
        serde_json::json!({
            "zcodejwttoken": zcode_jwt,
            "oauth:zai:access_token": access_token
        })
        .to_string()
    }

    fn test_credentials() -> ResetCredentials {
        ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("jwt-value".to_string()),
            zai_access_token: ZaiAccessToken("access-value".to_string()),
        }
    }

    // ---------- mutation boundary (source-level proof) ----------

    /// LimitScope observes only. The module source itself must not contain
    /// the endpoint's mutating siblings, any request but GET, any upstream
    /// URL but the status endpoint, or any logging of response bodies. The
    /// forbidden path literals are assembled from fragments so this test
    /// cannot reintroduce the very strings it forbids.
    #[test]
    fn module_source_contains_no_mutating_endpoints_or_logging() {
        let source = include_str!("zcode_reset.rs");

        let forbidden_paths = [
            format!("reset/{}", "use"),
            format!("reset/{}", "opportunity"),
            format!("reset/{}/{}", "history", "read"),
        ];
        for path in forbidden_paths {
            assert!(
                !source.contains(&path),
                "mutating endpoint path must be absent from the module: {path}"
            );
        }

        // The only HTTP method the module ever issues is GET. The needle
        // literals are assembled from fragments so this assertion cannot
        // reintroduce the very strings it forbids.
        assert!(
            !source.contains(&format!(".{}(", "post")),
            "no POST request may exist"
        );
        assert!(
            !source.contains(&format!("Method::{}", "POST")),
            "no POST method may exist"
        );
        assert!(
            !source.contains(&format!(".{}(", "put")),
            "no PUT request may exist"
        );

        // Every absolute URL in the module must be the read-only status
        // endpoint (the constant and strings that merely repeat it).
        let scheme_needle = format!("{}://", "https");
        for (offset, _) in source.match_indices(&scheme_needle) {
            assert!(
                source[offset..].starts_with(RESET_STATUS_URL),
                "unexpected upstream URL at byte {offset}: the status endpoint is the only one allowed"
            );
        }

        // Raw response bodies are never logged: no log macros exist (the
        // needle literals are assembled from fragments for the same reason).
        assert!(!source.contains(&format!("{}println!", "e")));
        assert!(!source.contains(&format!("{}::", "log")));
        assert!(!source.contains(&format!("{}::", "tracing")));
    }

    // ---------- parser matrix ----------

    // 1. no cards
    #[test]
    fn empty_arrays_parse_to_no_cards() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "available_five_hour_resets": [],
                "available_week_resets": []
            }
        })
        .to_string();
        let (status, facts) = parse_reset_status(body.as_bytes()).unwrap();
        assert!(status.five_hour_cards.is_empty());
        assert!(status.weekly_cards.is_empty());
        assert_eq!(facts.five_hour_count, 0);
        assert_eq!(facts.weekly_count, 0);
        assert!(!facts.five_hour_expiries_present);
        assert!(!facts.weekly_expiries_present);
    }

    // 2. one 5h card
    #[test]
    fn one_five_hour_card_parses_with_target_and_expiry() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "available_five_hour_resets": [{ "expire_at": 1792137600000_i64 }],
                "available_week_resets": []
            }
        })
        .to_string();
        let (status, _) = parse_reset_status(body.as_bytes()).unwrap();
        assert_eq!(status.five_hour_cards.len(), 1);
        assert_eq!(status.five_hour_cards[0].target, ZCodeResetTarget::FiveHour);
        // 1792137600000 ms == 2026-10-16T08:00:00Z
        assert_eq!(
            status.five_hour_cards[0].expires_at.as_deref(),
            Some("2026-10-16T08:00:00Z")
        );
        assert!(status.weekly_cards.is_empty());
    }

    // 3. one weekly card
    #[test]
    fn one_weekly_card_parses() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "available_week_resets": [{ "expire_at": 1792742400000_i64 }]
            }
        })
        .to_string();
        let (status, facts) = parse_reset_status(body.as_bytes()).unwrap();
        assert!(status.five_hour_cards.is_empty());
        assert_eq!(status.weekly_cards.len(), 1);
        assert_eq!(status.weekly_cards[0].target, ZCodeResetTarget::Weekly);
        // 1792742400000 ms == 2026-10-23T08:00:00Z
        assert_eq!(
            status.weekly_cards[0].expires_at.as_deref(),
            Some("2026-10-23T08:00:00Z")
        );
        assert!(facts.weekly_expiries_present);
    }

    // 4. multiple cards
    #[test]
    fn multiple_cards_keep_upstream_order_and_counts() {
        let (status, facts) = parse_reset_status(status_body().to_string().as_bytes()).unwrap();
        assert_eq!(status.five_hour_cards.len(), 2);
        assert_eq!(status.weekly_cards.len(), 1);
        // Order follows the upstream arrays.
        assert_eq!(
            status.five_hour_cards[0].expires_at.as_deref(),
            Some("2026-10-16T08:00:00Z")
        );
        assert_eq!(
            status.five_hour_cards[1].expires_at.as_deref(),
            Some("2026-10-17T08:00:00Z")
        );
        assert_eq!(facts.five_hour_count, 2);
        assert_eq!(facts.weekly_count, 1);
        assert!(facts.five_hour_expiries_present);
        assert!(facts.weekly_expiries_present);
        assert!(facts.history_fields_present);
    }

    // 5. expiry parsing (epoch seconds are promoted; epoch ms pass through)
    #[test]
    fn expiry_accepts_epoch_seconds_and_milliseconds() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "available_five_hour_resets": [
                    { "expire_at": 1792137600_i64 },
                    { "expire_at": 1792137600000_i64 }
                ]
            }
        })
        .to_string();
        let (status, _) = parse_reset_status(body.as_bytes()).unwrap();
        // Both forms denote the same instant.
        assert_eq!(
            status.five_hour_cards[0].expires_at,
            status.five_hour_cards[1].expires_at
        );
        assert_eq!(
            status.five_hour_cards[0].expires_at.as_deref(),
            Some("2026-10-16T08:00:00Z")
        );
    }

    // 6. malformed expiry
    #[test]
    fn malformed_expiry_keeps_the_card_without_a_date() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "available_five_hour_resets": [
                    { "expire_at": "not-a-date" },
                    { "expire_at": -5 },
                    { "expire_at": 0 },
                    { "expire_at": 4_200_000_000_000_i64 },
                    {}
                ],
                "available_week_resets": []
            }
        })
        .to_string();
        let (status, facts) = parse_reset_status(body.as_bytes()).unwrap();
        // The cards still count; only the unusable stamp is dropped.
        assert_eq!(status.five_hour_cards.len(), 5);
        assert!(status
            .five_hour_cards
            .iter()
            .all(|card| card.expires_at.is_none()));
        assert!(!facts.five_hour_expiries_present);
    }

    // 7. missing arrays
    #[test]
    fn missing_arrays_degrade_to_no_cards() {
        let body = serde_json::json!({ "code": 0, "data": {} }).to_string();
        let (status, facts) = parse_reset_status(body.as_bytes()).unwrap();
        assert!(status.five_hour_cards.is_empty());
        assert!(status.weekly_cards.is_empty());
        assert_eq!(facts.five_hour_count, 0);
        assert_eq!(facts.weekly_count, 0);
        // No history fields in this payload either.
        assert!(!facts.history_fields_present);
    }

    // 8. unknown additional fields
    #[test]
    fn unknown_fields_are_ignored() {
        let body = serde_json::json!({
            "code": 0,
            "msg": "ok",
            "success": true,
            "data": {
                "available_five_hour_resets": [
                    { "expire_at": 1792137600000_i64, "id": "card-1", "kind": "bonus" }
                ],
                "available_week_resets": [],
                "some_future_field": { "nested": [1, 2, 3] }
            },
            "trace_id": "whatever"
        })
        .to_string();
        let (status, _) = parse_reset_status(body.as_bytes()).unwrap();
        assert_eq!(status.five_hour_cards.len(), 1);
        assert_eq!(
            status.five_hour_cards[0].expires_at.as_deref(),
            Some("2026-10-16T08:00:00Z")
        );
    }

    // ---------- credential matrix ----------

    // 9. missing zcodejwttoken
    #[test]
    fn missing_zcode_jwt_is_credential_missing() {
        let raw = serde_json::json!({ "oauth:zai:access_token": "access-value" }).to_string();
        let error = parse_reset_credentials(&raw, TEST_SECRET).unwrap_err();
        assert_eq!(error.code, "credential_missing");
    }

    // 10. missing OAuth token
    #[test]
    fn missing_oauth_access_token_is_credential_missing() {
        let raw = serde_json::json!({ "zcodejwttoken": "jwt-value" }).to_string();
        let error = parse_reset_credentials(&raw, TEST_SECRET).unwrap_err();
        assert_eq!(error.code, "credential_missing");
    }

    // 11. malformed credential file
    #[test]
    fn malformed_credential_file_is_auth_unreadable() {
        let error = parse_reset_credentials("not json", TEST_SECRET).unwrap_err();
        assert_eq!(error.code, "auth_unreadable");
        // A valid-JSON non-object store holds no credentials at all.
        let error = parse_reset_credentials("[]", TEST_SECRET).unwrap_err();
        assert_eq!(error.code, "credential_missing");
        // Blank entries are as good as missing ones.
        let raw = credential_body("   ", "access-value");
        let error = parse_reset_credentials(&raw, TEST_SECRET).unwrap_err();
        assert_eq!(error.code, "credential_missing");
    }

    #[test]
    fn credentials_decrypt_from_the_real_store_format() {
        let raw = serde_json::json!({
            "zcodejwttoken": seal("jwt-sentinel-plain", &[3u8; ENC_IV_LEN]),
            "oauth:zai:access_token": seal("access-sentinel-plain", &[4u8; ENC_IV_LEN]),
            "unrelated:entry": "untouched"
        })
        .to_string();
        let credentials = parse_reset_credentials(&raw, TEST_SECRET).unwrap();
        assert_eq!(credentials.zcode_session_jwt.0, "jwt-sentinel-plain");
        assert_eq!(credentials.zai_access_token.0, "access-sentinel-plain");
    }

    // 12. Debug output redacts credentials
    #[test]
    fn credential_debug_output_is_redacted() {
        let credentials = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("jwt-sentinel-plain".to_string()),
            zai_access_token: ZaiAccessToken("access-sentinel-plain".to_string()),
        };
        let debug = format!("{credentials:?}");
        assert!(debug.contains("ResetCredentials"), "debug: {debug}");
        assert!(!debug.contains("jwt-sentinel-plain"), "debug: {debug}");
        assert!(!debug.contains("access-sentinel-plain"), "debug: {debug}");
        assert!(debug.contains("<redacted>"), "debug: {debug}");
    }

    // 13. serde/wire cannot serialize credentials
    #[test]
    fn credentials_cannot_be_serialized_and_wire_stays_clean() {
        let credentials = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("jwt-sentinel-plain".to_string()),
            zai_access_token: ZaiAccessToken("access-sentinel-plain".to_string()),
        };
        let attempt = serde_json::to_string(&credentials);
        assert!(attempt.is_err(), "credentials must not serialize");
        assert!(attempt
            .unwrap_err()
            .to_string()
            .contains("never cross the wire"));

        // The wire struct derived from a live-shape body never carries
        // credential material (the sentinels above existed as credentials).
        let raw = credential_body("jwt-sentinel-plain", "access-sentinel-plain");
        let credentials = parse_reset_credentials(&raw, TEST_SECRET).unwrap();
        let headers = auth_headers(&credentials).unwrap();
        let (status, _) = parse_reset_status(status_body().to_string().as_bytes()).unwrap();
        let wire = serde_json::to_string(&status).unwrap();
        assert!(wire.contains("fiveHourCards"), "wire: {wire}");
        assert!(wire.contains("weeklyCards"), "wire: {wire}");
        assert!(wire.contains("expiresAt"), "wire: {wire}");
        assert!(!wire.contains("sentinel"), "wire: {wire}");
        assert!(!wire.contains("observed_at"), "internal stamp stays off the wire: {wire}");
        // Headers are request-side only and are not part of any wire struct.
        assert!(headers.iter().any(|(name, value)| name == "authorization"
            && value.starts_with("Bearer ")));
    }

    // 14. credential reader byte-identical
    #[test]
    fn credential_reader_leaves_the_store_byte_identical() {
        // A realistic store: sealed values, unrelated entries, trailing newline.
        let raw = format!(
            "{}\n",
            serde_json::json!({
                "zcodejwttoken": seal("jwt-sentinel-plain", &[3u8; ENC_IV_LEN]),
                "oauth:zai:access_token": seal("access-sentinel-plain", &[4u8; ENC_IV_LEN]),
                "unrelated:entry": "untouched"
            })
        );
        let path = std::env::temp_dir().join(format!(
            "limitscope-zcode-reset-bytes-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, &raw).unwrap();

        let before = fs::read(&path).unwrap();
        let store = read_store_string(&path).unwrap();
        let parsed = parse_reset_credentials(&store, TEST_SECRET).unwrap();
        let after = fs::read(&path).unwrap();

        assert_eq!(before, after, "the reader must not write the store");
        assert_eq!(parsed.zcode_session_jwt.0, "jwt-sentinel-plain");
        fs::remove_file(&path).ok();
    }

    // 15. auth failure degrades without affecting ordinary Z.ai quota
    #[test]
    fn reset_credential_failure_does_not_disturb_the_zai_quota_reader() {
        // A store that carries a valid Z.ai plan key but no reset credentials:
        // each reader answers for itself, independently.
        let raw = serde_json::json!({
            "account-provider:coding-plan:account:zai-individual-coding-plan:account:uuid:api-key":
                "individual-key"
        })
        .to_string();
        let reset_error = parse_reset_credentials(&raw, TEST_SECRET).unwrap_err();
        assert_eq!(reset_error.code, "credential_missing");

        let zai_keys = crate::zai::parse_credentials(&raw, TEST_SECRET).unwrap();
        assert_eq!(zai_keys.len(), 1);
        assert_eq!(zai_keys[0].0, "individual-key");

        // And the reverse: a store with only reset credentials never becomes
        // a Z.ai quota candidate.
        let raw = credential_body("jwt-value", "access-value");
        let reset = parse_reset_credentials(&raw, TEST_SECRET).unwrap();
        assert_eq!(reset.zcode_session_jwt.0, "jwt-value");
        assert_eq!(
            crate::zai::parse_credentials(&raw, TEST_SECRET).unwrap_err().code,
            "credential_missing"
        );
    }

    // ---------- envelope / HTTP classification ----------

    #[test]
    fn envelope_zero_is_success_and_business_codes_map_to_credential_errors() {
        assert!(envelope_error(0, &serde_json::json!({})).is_none());
        assert_eq!(
            parse_reset_status(serde_json::json!({
                "code": 401, "msg": "token expired or incorrect"
            }).to_string().as_bytes())
            .unwrap_err()
            .code,
            "auth_invalid"
        );
        assert_eq!(
            parse_reset_status(serde_json::json!({
                "code": 403, "msg": "forbidden"
            }).to_string().as_bytes())
            .unwrap_err()
            .code,
            "not_entitled"
        );
    }

    #[test]
    fn envelope_other_codes_classify_transiency() {
        let envelope = |code: i64| {
            serde_json::json!({ "code": code, "msg": "boom" }).to_string()
        };
        assert_eq!(
            parse_reset_status(envelope(429).as_bytes()).unwrap_err().transient,
            Some(true)
        );
        assert_eq!(
            parse_reset_status(envelope(500).as_bytes()).unwrap_err().transient,
            Some(true)
        );
        assert_eq!(
            parse_reset_status(envelope(400).as_bytes()).unwrap_err().transient,
            Some(false)
        );
        assert_eq!(
            parse_reset_status(envelope(401).as_bytes()).unwrap_err().transient,
            Some(false)
        );
    }

    /// The envelope `msg` is provider-controlled free text that reaches a
    /// user-visible message, so it passes the same display-safety gate the
    /// Z.ai monitor errors use: hostile text is stripped and capped, benign
    /// text is unchanged, and the retry classification is unaffected.
    #[test]
    fn envelope_upstream_message_is_sanitized_before_display() {
        let hostile = "\u{1b}[31mEVIL\u{1b}[0m  injected\u{0}text";
        let error = parse_reset_status(
            serde_json::json!({ "code": 400, "msg": hostile }).to_string().as_bytes(),
        )
        .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(
            error.message,
            "ZCode reset-status endpoint rejected the request (code 400): EVIL injectedtext"
        );
        assert!(!error.message.contains('\u{1b}'));
        assert!(!error.message.chars().any(char::is_control));

        // Overlong upstream text is capped deterministically.
        let long = "x".repeat(crate::zai::MAX_UPSTREAM_DETAIL_CHARS + 50);
        let error = parse_reset_status(
            serde_json::json!({ "code": 400, "msg": long }).to_string().as_bytes(),
        )
        .unwrap_err();
        let detail_len = error.message.split(": ").last().unwrap().chars().count();
        assert_eq!(detail_len, crate::zai::MAX_UPSTREAM_DETAIL_CHARS);

        // Empty or escape-only text degrades to the authored fallback.
        let absent = parse_reset_status(br#"{"code":400}"#).unwrap_err();
        assert!(
            absent.message.ends_with(": request failed"),
            "{}",
            absent.message
        );

        // Sanitizing the detail never changes the retry verdict.
        assert_eq!(
            parse_reset_status(
                serde_json::json!({ "code": 500, "msg": hostile }).to_string().as_bytes(),
            )
            .unwrap_err()
            .transient,
            Some(true)
        );
    }

    #[test]
    fn missing_or_malformed_envelope_is_a_schema_change() {
        let html = b"<html><body>Please sign in</body></html>";
        assert_eq!(parse_reset_status(html).unwrap_err().code, "unexpected_response");
        assert_eq!(
            parse_reset_status(serde_json::json!({ "data": {} }).to_string().as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
        assert_eq!(
            parse_reset_status(serde_json::json!({ "code": 0 }).to_string().as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
        assert_eq!(
            parse_reset_status(serde_json::json!({ "code": 0, "data": [1] }).to_string().as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
    }

    #[test]
    fn auth_headers_match_the_apps_request_shape() {
        // A token already carrying a Bearer prefix is passed through as-is
        // (the app only prepends when the prefix is absent).
        let credentials = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("Bearer jwt-value".to_string()),
            zai_access_token: ZaiAccessToken("access-value".to_string()),
        };
        let headers = auth_headers(&credentials).unwrap();
        assert_eq!(
            headers,
            vec![
                ("authorization".to_string(), "Bearer jwt-value".to_string()),
                ("x-bigmodel-authorization".to_string(), "access-value".to_string()),
                ("bigmodel-target-type".to_string(), "PERSONAL".to_string()),
            ]
        );

        // Without a prefix, Bearer is added to the session JWT only.
        let credentials = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("jwt-value".to_string()),
            zai_access_token: ZaiAccessToken("access-value".to_string()),
        };
        let headers = auth_headers(&credentials).unwrap();
        assert_eq!(headers[0].1, "Bearer jwt-value");
        assert_eq!(headers[1].1, "access-value");
        // Empty credential material is refused before any request is built.
        let blank = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("   ".to_string()),
            zai_access_token: ZaiAccessToken("access-value".to_string()),
        };
        assert_eq!(auth_headers(&blank).unwrap_err().code, "credential_missing");
        let blank_access = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("jwt-value".to_string()),
            zai_access_token: ZaiAccessToken("  ".to_string()),
        };
        assert_eq!(
            auth_headers(&blank_access).unwrap_err().code,
            "credential_missing"
        );
    }

    #[test]
    fn bearer_prefix_test_survives_a_multibyte_store_value() {
        // A plaintext store entry passes through decrypt_credential
        // unvalidated, so the session JWT can be any text. When byte 7 falls
        // inside a multibyte character ("bearer" + é spans bytes 6..8), the
        // old `str`-slice prefix test panicked there — and std's slice-panic
        // message embeds the credential value itself. The byte-wise test
        // must take the prepend branch instead, never panic.
        let credentials = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("bearer\u{00e9}-token".to_string()),
            zai_access_token: ZaiAccessToken("access-value".to_string()),
        };
        let headers = auth_headers(&credentials).unwrap();
        assert_eq!(headers[0].1, "Bearer bearer\u{00e9}-token");

        // The ASCII passthrough still compares byte-wise case-insensitively.
        let upper = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt("BEARER jwt-value".to_string()),
            zai_access_token: ZaiAccessToken("access-value".to_string()),
        };
        let headers = auth_headers(&upper).unwrap();
        assert_eq!(headers[0].1, "BEARER jwt-value");
    }

    // ---------- transport behavior (hermetic, local sockets) ----------

    /// The shared client refuses redirects: a 302 from the status endpoint is
    /// surfaced as-is, never followed, so the credentials cannot be relayed
    /// to a redirect target.
    #[tokio::test]
    async fn reset_status_redirect_is_returned_without_following() {
        let destination = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        // Non-blocking: the assertion below is that nothing ever connects,
        // so a blocking accept would hang the test instead of proving it.
        destination.set_nonblocking(true).unwrap();
        let destination_addr = destination.local_addr().unwrap();
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let source_addr = source.local_addr().unwrap();
        let destination_thread = std::thread::spawn(move || destination.accept());
        let source_thread = std::thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{destination_addr}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .unwrap();
            String::from_utf8_lossy(&request).to_ascii_lowercase()
        });
        let response = fetch_reset_status_from(
            http_client().unwrap(),
            &format!("http://{source_addr}/status"),
            &test_credentials(),
        )
        .await
        .unwrap_err();
        // The 302 is a non-success status: the fetch surfaces a plain HTTP
        // failure and never contacts the redirect target.
        assert_eq!(response.code, "unexpected_response");
        assert_eq!(response.http_status, Some(302));
        assert!(source_thread
            .join()
            .unwrap()
            .contains("authorization: bearer jwt-value"));
        assert!(
            destination_thread.join().unwrap().unwrap_err().kind()
                == std::io::ErrorKind::WouldBlock,
            "the redirect target must never be contacted"
        );
    }

    /// A transport-level failure (connection refused) is a transient network
    /// error, never a credential verdict, and carries no upstream text.
    #[tokio::test]
    async fn transport_failure_is_a_transient_network_error() {
        // Port 9 (discard) is not served locally: the connection is refused.
        let error = fetch_reset_status_from(
            http_client().unwrap(),
            "http://127.0.0.1:9/status",
            &test_credentials(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "network");
        assert_eq!(error.transient, Some(true));
    }

    /// End-to-end leak simulation: an upstream envelope whose
    /// provider-controlled `msg` echoes the in-flight session credential
    /// back inside its error detail. The surfaced failure must no longer
    /// contain the credential value, while the benign carrier text survives.
    #[tokio::test]
    async fn envelope_echoing_a_credential_value_is_scrubbed() {
        const ECHOED_JWT: &str = "echo-jwt-value-0123456789abcdef";
        const ECHOED_ACCESS: &str = "echo-access-value-0123456789";
        let credentials = ResetCredentials {
            zcode_session_jwt: ZcodeSessionJwt(ECHOED_JWT.to_string()),
            zai_access_token: ZaiAccessToken(ECHOED_ACCESS.to_string()),
        };
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let source_addr = source.local_addr().unwrap();
        let body =
            format!("{{\"code\":1,\"msg\":\"request refused for bearer {ECHOED_JWT} (see logs)\"}}");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .unwrap();
        });
        let error = fetch_reset_status_from(
            http_client().unwrap(),
            &format!("http://{source_addr}/status"),
            &credentials,
        )
        .await
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code, "unexpected_response");
        assert!(!error.message.contains(ECHOED_JWT), "{}", error.message);
        assert!(!error.message.contains(ECHOED_ACCESS), "{}", error.message);
        assert!(
            error
                .message
                .contains("request refused for bearer [REDACTED] (see logs)"),
            "{}",
            error.message
        );
    }

    // ---------- retention (last-good for the observation) ----------

    fn observed_status(count: usize, observed_at: DateTime<Utc>) -> ZCodeResetStatus {
        ZCodeResetStatus {
            five_hour_cards: vec![ZCodeResetCard {
                target: ZCodeResetTarget::FiveHour,
                expires_at: Some("2026-10-16T08:00:00Z".to_string()),
            }],
            weekly_cards: (0..count)
                .map(|_| ZCodeResetCard {
                    target: ZCodeResetTarget::Weekly,
                    expires_at: None,
                })
                .collect(),
            observed_at,
        }
    }

    #[test]
    fn fresh_observation_replaces_the_cache_and_is_stamped() {
        let mut cache = None;
        let now = Utc::now();
        let observed = observed_status(1, now - chrono::Duration::minutes(5));
        let result =
            retain_observation(&mut cache, Ok(observed), now).unwrap();
        // The freshness authority re-stamps with the observation clock.
        assert_eq!(result.observed_at, now);
        assert_eq!(cache.as_ref().unwrap().observed_at, now);
        assert_eq!(result.five_hour_cards.len(), 1);
    }

    #[test]
    fn failure_within_the_ttl_serves_the_retained_observation() {
        let observed_at = Utc::now() - chrono::Duration::minutes(10);
        let mut cache = Some(observed_status(1, observed_at));
        let failure = Err(ProviderError::transient("network", "offline"));
        let served = retain_observation(&mut cache, failure, Utc::now()).unwrap();
        // The retained observation stands in, with its ORIGINAL stamp.
        assert_eq!(served.observed_at, observed_at);
        assert!(cache.is_some());
    }

    #[test]
    fn failure_after_the_ttl_propagates_and_drops_the_cache() {
        let observed_at = Utc::now() - chrono::Duration::seconds(OBSERVATION_TTL_SECS + 60);
        let mut cache = Some(observed_status(1, observed_at));
        let failure = Err(ProviderError::transient("network", "offline"));
        let error = retain_observation(&mut cache, failure, Utc::now()).unwrap_err();
        assert_eq!(error.code, "network");
        assert!(cache.is_none(), "an expired observation must not linger");
    }

    #[test]
    fn failure_without_any_cache_propagates() {
        let mut cache = None;
        let failure = Err(ProviderError::new("auth_invalid", "rejected"));
        let error = retain_observation(&mut cache, failure, Utc::now()).unwrap_err();
        assert_eq!(error.code, "auth_invalid");
        assert!(cache.is_none());
    }

    #[test]
    fn freshness_budget_mirrors_the_codex_convention() {
        let now = Utc::now();
        // Fresh: just observed, and right at the TTL edge.
        assert!(cards_fresh(&(now - chrono::Duration::seconds(1)), now));
        assert!(cards_fresh(
            &(now - chrono::Duration::seconds(OBSERVATION_TTL_SECS)),
            now
        ));
        // Stale: past the budget.
        assert!(!cards_fresh(
            &(now - chrono::Duration::seconds(OBSERVATION_TTL_SECS + 1)),
            now
        ));
        // Future-dated beyond the skew tolerance is not trusted either.
        assert!(!cards_fresh(
            &(now + chrono::Duration::seconds(OBSERVED_FUTURE_TOLERANCE_SECS + 1)),
            now
        ));
        assert!(cards_fresh(
            &(now + chrono::Duration::seconds(OBSERVED_FUTURE_TOLERANCE_SECS)),
            now
        ));
    }

    /// Live proof for the discovery run: exactly ONE GET against the status
    /// endpoint. Run manually under the credential-hash protocol (hash before,
    /// run, hash after) — never in CI, never in a loop.
    #[tokio::test]
    #[ignore = "live proof: exactly one GET /reset/status; run manually with the credential hash protocol"]
    async fn live_reset_status_observation() {
        let client = http_client().expect("http client");
        let (status, facts) = fetch_status_facts(
            client,
            RESET_STATUS_URL,
            &load_reset_credentials().expect("live credentials"),
        )
        .await
        .expect("live reset-status observation");
        // Safe metadata only — no tokens, no identifiers, no raw body.
        println!("reset/status observation:");
        println!("  http class: 2xx (success)");
        println!("  five-hour cards: {}", facts.five_hour_count);
        println!("  weekly cards: {}", facts.weekly_count);
        println!(
            "  five-hour expiries present: {}",
            facts.five_hour_expiries_present
        );
        println!("  weekly expiries present: {}", facts.weekly_expiries_present);
        println!("  history fields present: {}", facts.history_fields_present);
        assert_eq!(status.five_hour_cards.len(), facts.five_hour_count);
        assert_eq!(status.weekly_cards.len(), facts.weekly_count);
    }
}
