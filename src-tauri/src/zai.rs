//! Z.ai (GLM Coding Plan) provider backend.
//!
//! Reads the coding-plan API key from the local ZCode stores (read-only;
//! `ZAI_API_KEY` env override) and queries the Z.ai usage monitor endpoint
//! (`GET https://api.z.ai/api/monitor/usage/quota/limit`). The key stays in
//! this process: nothing beyond normalized windows is returned to the
//! WebView, and no auth material is ever logged.
//!
//! Live-verified 2026-09-27: the monitor endpoint accepts the GLM coding-plan
//! API key. ZCode keeps that same key in two places — in plaintext under the
//! enabled `builtin:zai-coding-plan` provider in `~/.zcode/v2/config.json`,
//! and encrypted (`enc:v1`, AES-256-GCM, app-derived key) under the
//! `account-provider:coding-plan:account:zai-*:api-key` entries of
//! `~/.zcode/v2/credentials.json`. The plaintext mirror is preferred (no
//! dependency on the app's encryption scheme); the store copy is decrypted
//! here, in this process, as a fallback. Auth failures arrive as HTTP 200
//! with a JSON body `{ code: 401, success: false }`, not as HTTP statuses.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use crate::provider_error::ProviderError;
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const USAGE_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";
const ZCODE_DIR: &str = ".zcode/v2";
const CREDENTIAL_SECRET_ENV: &str = "ZCODE_CREDENTIAL_SECRET";
const USER_AGENT: &str = concat!("rate-limits/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// `enc:v1` envelope (matches the ZCode app's createCredentialCipherProvider):
// "enc:v1:" + base64url(iv) + "." + base64url(tag) + "." + base64url(ciphertext)
pub(crate) const ENC_PREFIX: &str = "enc:v1:";
pub(crate) const ENC_IV_LEN: usize = 12;
pub(crate) const ENC_TAG_LEN: usize = 16;

// Plausibility bounds for `nextResetTime` epoch-millisecond timestamps
// (2100-01-01 UTC as the upper bound; smaller magnitudes are epoch seconds).
const MIN_RESET_MS: i64 = 100_000_000_000;
const MAX_RESET_MS: i64 = 4_102_444_800_000;

// ---------- data returned to the WebView (camelCase on the wire) ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZaiLimitWindow {
    pub label: String,
    pub used_percent: f64,
    /// RFC 3339 UTC timestamp derived from the upstream `nextResetTime`
    /// epoch-milliseconds value. `None` when upstream has no usable stamp
    /// (e.g. the 5-hour window before its first use).
    pub reset_at: Option<String>,
}

/// Masked attribution of the credential whose quota is shown: the last four
/// characters of the API key — the same tail convention the OpenCode Go card
/// masks with — so the user can tell which stored candidate answered without
/// any key material crossing the boundary.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ZaiAccount {
    pub key_hint: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZaiUsage {
    pub limits: Vec<ZaiLimitWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<ZaiAccount>,
}

pub(crate) fn zcode_not_installed() -> ProviderError {
    ProviderError::new(
        "zcode_not_installed",
        "ZCode is not installed (ZCode data directory not found).",
    )
}

pub(crate) fn credential_missing() -> ProviderError {
    ProviderError::new(
        "credential_missing",
        "No Z.ai coding-plan API key found in the ZCode stores. Sign in to ZCode first, or set ZAI_API_KEY.",
    )
}

pub(crate) fn auth_invalid() -> ProviderError {
    ProviderError::new(
        "auth_invalid",
        "The Z.ai API key was rejected. Re-authenticate ZCode, then refresh again.",
    )
}

pub(crate) fn not_entitled() -> ProviderError {
    ProviderError::new(
        "not_entitled",
        "The Z.ai usage endpoint refused access to this account's key.",
    )
}

/// This key was refused by the endpoint; the resolver may try the next
/// candidate key.
fn is_credential_rejected(error: &ProviderError) -> bool {
    matches!(error.code.as_str(), "auth_invalid" | "not_entitled")
}

/// Masked key hint: the last four characters of the stored key — the same
/// masking convention the OpenCode Go card uses (`…1234`). Deterministic per
/// credential, so the card and the failure attribution stay stable.
/// `None` when the key is too short to mask meaningfully or the tail is not
/// key material.
fn account_hint(key: &str) -> Option<String> {
    let tail = key.chars().rev().take(4).collect::<Vec<_>>();
    // Real plan keys are far longer; anything shorter than 12 would leak too
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
    Some(tail.into_iter().rev().collect())
}

/// The identity string the success attribution and the failure paths build
/// for one credential candidate, so the runtime's last-good guard compares
/// like-for-like. The hint identifies the *credential*, which is exactly
/// what the resolution loop attempts — candidate by candidate.
fn key_identity(key: &ZaiKey) -> Option<String> {
    account_hint(&key.0).map(|hint| format!("key:{hint}"))
}

// ---------- upstream response (only the fields we consume; extra ignored) ----------
//
// Live-verified 2026-09-27 (plan level "lite"): payload under `data`, plus a
// `{ code, msg, success }` envelope (code 200 = ok). `limits[]` entries carry
// `type` (CREDIT_LIMIT observed; TIME_LIMIT/TOKENS_LIMIT/… on other plan
// levels), `unit`/`number` (window duration), `percentage` (used, 0–100), and
// `nextResetTime` (epoch ms) — the latter absent for an untouched window.

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    data: Option<UsagePayload>,
    #[serde(flatten)]
    root: Value,
}

#[derive(Debug, Deserialize)]
struct UsagePayload {
    #[serde(default)]
    limits: Option<Vec<LimitEntry>>,
}

#[derive(Debug, Deserialize)]
struct LimitEntry {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    unit: Option<i64>,
    #[serde(default)]
    number: Option<i64>,
    #[serde(default)]
    percentage: Option<f64>,
    #[serde(default, rename = "nextResetTime")]
    next_reset_time: Option<Value>,
}

// ---------- normalization ----------

/// Dashboard label for one upstream window. GLM's coding plan documents
/// exactly two credit windows — a 5-hour pool and a weekly pool that resets
/// every 7 days from order time (docs.z.ai/devpack); there is no monthly
/// window. The live payload spells the weekly pool as unit 6/number 1 (its
/// next reset lands ~7 days out, matching the documented weekly cycle), and
/// the 5-hour window uses the same label as Codex and OpenCode Go.
fn label_for_limit(limit: &LimitEntry) -> String {
    match limit.r#type.as_deref() {
        Some("TIME_LIMIT") => return "5-hour".to_string(),
        Some("TOKENS_LIMIT") => return "Tokens".to_string(),
        Some("RATE_LIMIT") => return "Rate limit".to_string(),
        Some("TIMES_LIMIT") => return "Requests".to_string(),
        Some("SESSION_LIMIT") => return "Sessions".to_string(),
        _ => {}
    }
    match (limit.r#type.as_deref(), limit.unit, limit.number) {
        (Some("CREDIT_LIMIT"), Some(3), Some(5)) => "5-hour".to_string(),
        (Some("CREDIT_LIMIT"), Some(3), _) => "Hourly".to_string(),
        (Some("CREDIT_LIMIT"), Some(4), _) => "Daily".to_string(),
        (Some("CREDIT_LIMIT"), Some(5) | Some(6), _) => "Weekly".to_string(),
        _ => sanitize_fallback_label(limit.r#type.as_deref()),
    }
}

/// Conservative display cap for a provider-derived fallback label.
pub(crate) const MAX_FALLBACK_LABEL_CHARS: usize = 40;

/// Display-safety for provider-derived free text bound for a label: ANSI
/// escape sequences are stripped whole, control characters are removed,
/// whitespace runs collapse to single spaces, and the result is capped.
/// Ordinary text is unchanged. Returns `None` when nothing visible remains,
/// so the caller can degrade (to `"UNKNOWN"`, to omitting the label, ...) —
/// shared with the sibling ZCode observers that surface provider labels.
pub(crate) fn sanitize_display_label(raw: &str) -> Option<String> {
    let visible: String = strip_ansi_escapes(raw)
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let collapsed = visible
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let capped: String = collapsed.chars().take(MAX_FALLBACK_LABEL_CHARS).collect();
    (!capped.is_empty()).then_some(capped)
}

/// Display-safety for the fallback branch of `label_for_limit`: unknown
/// upstream `type` values are provider-derived free text, so they pass the
/// shared display gate; input with no visible text left degrades to
/// `"UNKNOWN"`, the same label a missing type uses. Known labels return
/// before this gate and are never altered.
fn sanitize_fallback_label(raw_type: Option<&str>) -> String {
    let Some(raw) = raw_type else {
        return "UNKNOWN".to_string();
    };
    sanitize_display_label(&raw.replace('_', " "))
        .unwrap_or_else(|| "UNKNOWN".to_string())
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
            Some('[') => {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7E}').contains(&next) {
                        break;
                    }
                }
            }
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
            Some(next) if ('\u{20}'..='\u{2F}').contains(next) => {
                while matches!(chars.peek(), Some(next) if ('\u{20}'..='\u{2F}').contains(next)) {
                    chars.next();
                }
                chars.next();
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Only windows that are actually present upstream are emitted — no invented
/// or defaulted limits. Order follows the upstream `limits[]` array.
fn normalize_usage(payload: &UsagePayload) -> Vec<ZaiLimitWindow> {
    let Some(limits) = &payload.limits else {
        return Vec::new();
    };
    limits
        .iter()
        .filter_map(|limit| {
            let used = limit.percentage?;
            let reset_at = limit
                .next_reset_time
                .as_ref()
                .and_then(epoch_ms_to_rfc3339)
                .map(|reset| reset.to_rfc3339_opts(SecondsFormat::Secs, true));
            Some(ZaiLimitWindow {
                label: label_for_limit(limit),
                used_percent: used.clamp(0.0, 100.0),
                reset_at,
            })
        })
        .collect()
}

/// `nextResetTime` arrives as epoch milliseconds (number; tolerate a string).
/// Implausible magnitudes — zero, negative, seconds-like, or post-2100 — are
/// rejected rather than rendered as a bogus date.
pub(crate) fn epoch_ms_to_rfc3339(raw: &Value) -> Option<DateTime<Utc>> {
    let mut ms = match raw {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }?;
    if ms <= 0 {
        return None;
    }
    if ms < MIN_RESET_MS {
        // Epoch seconds, not milliseconds; promote and re-check bounds.
        ms *= 1000;
    }
    if ms > MAX_RESET_MS {
        return None;
    }
    Utc.timestamp_millis_opt(ms).single()
}

/// Conservative display cap for provider-derived error detail embedded in a
/// user-visible message.
pub(crate) const MAX_UPSTREAM_DETAIL_CHARS: usize = 200;

/// Display-safety for upstream error text that would otherwise be embedded
/// verbatim in a user-visible message: ANSI escape sequences are stripped
/// whole, control characters are removed, whitespace runs collapse to single
/// spaces, and the result is capped. Ordinary text is unchanged. Returns
/// `None` when nothing visible remains, so the caller can fall back to an
/// authored sentence instead of rendering an empty detail.
pub(crate) fn sanitize_upstream_detail(raw: &str) -> Option<String> {
    let visible: String = strip_ansi_escapes(raw)
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let collapsed = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped: String = collapsed.chars().take(MAX_UPSTREAM_DETAIL_CHARS).collect();
    let trimmed = capped.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// The monitor endpoint answers auth failures with HTTP 200 and a JSON
/// envelope (`{ code: 401|403, msg, success: false }`); those must surface as
/// credential errors so the resolver can try its next candidate key.
///
/// The envelope `msg` is provider-controlled free text that is embedded in a
/// user-visible message, so it passes [`sanitize_upstream_detail`] first —
/// the same display-safety gate the fallback labels use — and degrades to an
/// authored sentence when nothing visible remains.
fn envelope_error(body: &Value) -> Option<ProviderError> {
    let code = body.get("code").and_then(Value::as_i64)?;
    if code == 200 {
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
        // Mirror the HTTP classification: an envelope code 429 or 5xx is a
        // server-side refusal worth one retry; anything else is deterministic.
        _ => ProviderError::new(
            "unexpected_response",
            format!("Z.ai usage endpoint rejected the request (code {code}): {detail}"),
        )
        .with_transient(code == 429 || (500..=599).contains(&code)),
    })
}

fn parse_windows(body: &[u8]) -> Result<Vec<ZaiLimitWindow>, ProviderError> {
    let parsed: Value = serde_json::from_slice(body).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "Z.ai usage response was not usable JSON (schema change).",
        )
    })?;
    if let Some(error) = envelope_error(&parsed) {
        return Err(error);
    }
    let response: UsageResponse = serde_json::from_value(parsed).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "Z.ai usage response format changed; no limits payload found.",
        )
    })?;
    let payload = response
        .data
        .or_else(|| serde_json::from_value(response.root).ok());
    let Some(payload) = payload else {
        return Err(ProviderError::new(
            "unexpected_response",
            "Z.ai usage response format changed; no limits payload found.",
        ));
    };
    let limits = normalize_usage(&payload);
    if limits.is_empty() {
        return Err(ProviderError::new(
            "unexpected_response",
            "Z.ai usage response format changed; no known windows found.",
        ));
    }
    Ok(limits)
}

// ---------- enc:v1 decryption (same scheme as the ZCode app) ----------

/// The secret the ZCode app derives its credential-encryption key from: the
/// `ZCODE_CREDENTIAL_SECRET` env var when set, else a platform/home/user string.
pub(crate) fn credential_secret() -> Result<String, ProviderError> {
    if let Ok(secret) = std::env::var(CREDENTIAL_SECRET_ENV) {
        if !secret.is_empty() {
            return Ok(secret);
        }
    }
    let platform = if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        // JS os.platform() reports "darwin" here; std reports "macos".
        "darwin"
    } else {
        std::env::consts::OS
    };
    let home = std::env::home_dir().ok_or_else(|| {
        ProviderError::new(
            "auth_unreadable",
            "Could not locate the user home directory to derive the credential key.",
        )
    })?;
    let home = home.to_string_lossy().to_string();
    // JS os.userInfo().username; USERNAME/USER covers the common cases, and
    // the home-dir basename is usually the same value as a further fallback.
    let username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            Path::new(&home)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    Ok(format!("zcode-credential-fallback:{platform}:{home}:{username}"))
}

/// Decrypts one `enc:v1` store value; non-prefixed values are plaintext.
pub(crate) fn decrypt_credential(value: &str, secret: &str) -> Result<String, ProviderError> {
    let Some(rest) = value.strip_prefix(ENC_PREFIX) else {
        return Ok(value.to_string());
    };
    let parts: Vec<&str> = rest.split('.').collect();
    let malformed = || {
        ProviderError::new(
            "auth_unreadable",
            "ZCode credential store holds a malformed encrypted value.",
        )
    };
    if parts.len() != 3 {
        return Err(malformed());
    }
    let iv = URL_SAFE_NO_PAD.decode(parts[0]).map_err(|_| malformed())?;
    let tag = URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| malformed())?;
    let ciphertext = URL_SAFE_NO_PAD.decode(parts[2]).map_err(|_| malformed())?;
    if iv.len() != ENC_IV_LEN || tag.len() != ENC_TAG_LEN {
        return Err(malformed());
    }
    let key = Sha256::digest(secret.as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| malformed())?;
    // aes-gcm's one-shot API expects the tag appended to the ciphertext.
    let mut sealed = ciphertext;
    sealed.extend_from_slice(&tag);
    let plain = cipher
        .decrypt(Nonce::from_slice(&iv), sealed.as_ref())
        .map_err(|_| {
            ProviderError::new(
                "auth_unreadable",
                "ZCode credential could not be decrypted on this machine.",
            )
        })?;
    String::from_utf8(plain).map_err(|_| malformed())
}

// ---------- local auth state (read-only) ----------

// Manual impl so the key can never reach logs through Debug formatting.
pub(crate) struct ZaiKey(pub(crate) String);

impl std::fmt::Debug for ZaiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ZaiKey").field(&"<redacted>").finish()
    }
}

pub(crate) fn zcode_dir() -> Result<PathBuf, ProviderError> {
    let home = std::env::home_dir().ok_or_else(zcode_not_installed)?;
    Ok(home.join(ZCODE_DIR))
}

/// Credential candidates, most preferred first:
/// 1. `ZAI_API_KEY` — explicit user override (community-tooling convention).
/// 2. Enabled providers in `~/.zcode/v2/config.json` whose base URL is the
///    Z.ai API the monitor endpoint lives on. This plaintext key is the one
///    the endpoint accepts (live-verified).
/// 3. The same plan keys, decrypted from the `enc:v1` entries in
///    `~/.zcode/v2/credentials.json` (individual before team) — kept as a
///    fallback for setups where the config mirror is absent.
/// The `builtin:zai-start-plan` entry (a zcode.z.ai session token) and any
/// other provider are never sent to api.z.ai.
fn load_key_candidates() -> Result<Vec<ZaiKey>, ProviderError> {
    let env_key = std::env::var("ZAI_API_KEY").ok();
    let mut keys = merge_env_and_store(env_key, load_config_keys())?;
    if keys.is_empty() {
        keys = load_credential_store_keys()?;
    }
    if keys.is_empty() {
        return Err(credential_missing());
    }
    Ok(keys)
}

/// A non-empty `ZAI_API_KEY` replaces the stores entirely; an empty/absent
/// value falls through to the store result.
fn merge_env_and_store(
    env_key: Option<String>,
    store: Result<Vec<ZaiKey>, ProviderError>,
) -> Result<Vec<ZaiKey>, ProviderError> {
    if let Some(env_key) = env_key.as_deref().map(str::trim).filter(|key| !key.is_empty()) {
        return Ok(vec![ZaiKey(env_key.to_string())]);
    }
    store
}

fn load_config_keys() -> Result<Vec<ZaiKey>, ProviderError> {
    let path = zcode_dir()?.join("config.json");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new())
        }
        Err(error) => {
            return Err(ProviderError::new(
                "auth_unreadable",
                format!("Could not read the ZCode configuration: {error}"),
            ))
        }
    };
    parse_config_keys(&raw)
}

/// Only providers whose base URL is `https://api.z.ai` qualify — a provider
/// key must never be sent to a host it was not minted for (e.g. the
/// `builtin:zai-start-plan` session token belongs to zcode.z.ai, not
/// api.z.ai), and the `https` scheme is required so a misconfigured plain-HTTP
/// base URL cannot downgrade the request.
fn provider_base_url_is_zai(provider: &Value) -> bool {
    let Some(base) = provider
        .pointer("/options/baseURL")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|base| !base.is_empty())
    else {
        return false;
    };
    let Some((scheme, host_rest)) = base.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    host_rest
        .split('/')
        .next()
        .unwrap_or_default()
        .eq_ignore_ascii_case("api.z.ai")
}

fn parse_config_keys(raw: &str) -> Result<Vec<ZaiKey>, ProviderError> {
    let value: Value = serde_json::from_str(raw).map_err(|_| {
        ProviderError::new(
            "auth_unreadable",
            "ZCode configuration (config.json) is not valid JSON.",
        )
    })?;
    let Some(providers) = value.get("provider").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut candidates: Vec<(usize, String, String)> = providers
        .iter()
        .filter_map(|(name, provider)| {
            if !provider_base_url_is_zai(provider) {
                return None;
            }
            let enabled = provider.get("enabled").and_then(Value::as_bool);
            let key = provider
                .pointer("/options/apiKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|key| !key.is_empty());
            // Disabled providers keep stale keys; only enabled ones are trusted.
            let usable = key.filter(|_| enabled != Some(false))?;
            // Z.ai coding plans first, other z.ai providers last.
            let rank = if name.contains("coding-plan") { 0 } else { 1 };
            Some((rank, name.clone(), usable.to_string()))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(candidates
        .into_iter()
        .map(|(_, _, key)| ZaiKey(key))
        .collect())
}

fn load_credential_store_keys() -> Result<Vec<ZaiKey>, ProviderError> {
    let path = zcode_dir()?.join("credentials.json");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(zcode_not_installed())
        }
        Err(error) => {
            return Err(ProviderError::new(
                "auth_unreadable",
                format!("Could not read the ZCode credential store: {error}"),
            ))
        }
    };
    parse_credentials(&raw, &credential_secret()?)
}

/// Extracts and decrypts the Z.ai plan API keys from a credential-store
/// document. Only keys whose *name* marks them as Z.ai coding-plan keys are
/// considered, so no other provider's credential can end up at api.z.ai.
pub(crate) fn parse_credentials(raw: &str, secret: &str) -> Result<Vec<ZaiKey>, ProviderError> {
    let value: Value = serde_json::from_str(raw).map_err(|_| {
        ProviderError::new(
            "auth_unreadable",
            "ZCode credential store (credentials.json) is not valid JSON.",
        )
    })?;
    let Value::Object(entries) = value else {
        return Err(credential_missing());
    };
    let mut candidates: Vec<(usize, String, String)> = entries
        .into_iter()
        .filter_map(|(name, value)| {
            let is_zai_plan_key = name.contains("coding-plan")
                && name.contains(":zai-")
                && name.ends_with(":api-key");
            let key = if is_zai_plan_key {
                value.as_str().map(str::trim).filter(|key| !key.is_empty())
            } else {
                None
            }?;
            // Individual plan first, then team, then everything else
            // alphabetically.
            let rank = if name.contains("individual") {
                0
            } else if name.contains("team") {
                1
            } else {
                2
            };
            Some((rank, name, key.to_string()))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    if candidates.is_empty() {
        return Err(credential_missing());
    }
    // Skip candidates that cannot be decrypted (e.g. written by another
    // machine profile); if none decrypt, the store is unusable.
    let decrypted: Vec<ZaiKey> = candidates
        .into_iter()
        .filter_map(|(_, _, key)| decrypt_credential(&key, secret).ok())
        .map(ZaiKey)
        .collect();
    if decrypted.is_empty() {
        return Err(ProviderError::new(
            "auth_unreadable",
            "None of the Z.ai credentials could be decrypted on this machine.",
        ));
    }
    Ok(decrypted)
}

// ---------- HTTP ----------

pub(crate) fn http_client() -> Result<&'static reqwest::Client, ProviderError> {
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

/// One authenticated GET against the monitor endpoint. `Err` codes
/// `auth_invalid` / `not_entitled` mean this key was refused and another
/// candidate (if any) may be tried.
async fn fetch_with_key(
    client: &reqwest::Client,
    key: &ZaiKey,
) -> Result<Vec<ZaiLimitWindow>, ProviderError> {
    let response = client
        .get(USAGE_URL)
        .bearer_auth(&key.0)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|error| {
            crate::provider_error::transport_failure(
                "network",
                format!("Could not reach the Z.ai usage endpoint: {error}"),
                &error,
            )
        })?;

    let status = response.status();
    let retry_after = crate::provider_error::retry_after_header(response.headers()).map(String::from);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(auth_invalid());
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(not_entitled());
    }
    let body = response.bytes().await.map_err(|error| {
        crate::provider_error::transport_failure(
            "network",
            format!("Z.ai usage response was cut short: {error}"),
            &error,
        )
    })?;
    if !status.is_success() {
        return Err(ProviderError::http_failure_with_retry_after(
            status,
            format!("Z.ai usage endpoint returned HTTP {status}."),
            retry_after.as_deref(),
            Utc::now(),
        ));
    }

    parse_windows(&body)
}

async fn fetch_zai_usage() -> Result<ZaiUsage, ProviderError> {
    let client = http_client()?;
    let keys = load_key_candidates()?;
    resolve_usage(&keys, |key| Box::pin(fetch_with_key(client, key))).await
}

/// Tries the credential candidates in order and attributes every outcome to
/// the credential ACTUALLY attempted: a success carries the winning key's
/// masked hint, a refusal (`auth_invalid`/`not_entitled`) is stamped with the
/// refused candidate's identity and advances, and a non-rejected failure
/// (network, schema) carries the identity of the candidate that was in
/// flight — never a different one.
async fn resolve_usage(
    keys: &[ZaiKey],
    fetch_one: impl for<'a> Fn(
        &'a ZaiKey,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<ZaiLimitWindow>, ProviderError>> + Send + 'a>,
    >,
) -> Result<ZaiUsage, ProviderError> {
    let mut last_error: Option<ProviderError> = None;
    for key in keys {
        let identity = key_identity(key);
        match fetch_one(key).await {
            Ok(limits) => {
                return Ok(ZaiUsage {
                    limits,
                    account: account_hint(&key.0).map(|key_hint| ZaiAccount { key_hint }),
                });
            }
            // This key was refused; try the next candidate (e.g. the team
            // plan key when the personal one has no monitor access). The
            // refusal keeps the refused key's identity so a final failure can
            // never be attributed to a candidate that was never tried.
            Err(error) if is_credential_rejected(&error) => {
                last_error = Some(scrub_candidate_failure(error, key).with_identity_hint(identity));
            }
            Err(error) => {
                return Err(scrub_candidate_failure(error, key).with_identity_hint(identity))
            }
        }
    }
    Err(last_error.unwrap_or_else(credential_missing))
}

/// Final-boundary value scrub (`secret_scrub`): the candidate key is in scope
/// for its own attempt, so any exact occurrence of it in the failure message
/// — for example an upstream echoing the authenticated request's own
/// credential back inside an error detail — is removed before the failure
/// leaves the candidate loop.
fn scrub_candidate_failure(error: ProviderError, key: &ZaiKey) -> ProviderError {
    crate::secret_scrub::scrub_provider_error(error, &[key.0.as_str()])
}

#[tauri::command]
pub async fn get_zai_usage() -> Result<ZaiUsage, ProviderError> {
    fetch_zai_usage().await
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    const TEST_SECRET: &str = "unit-test-secret";

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
        let client = http_client().unwrap();
        let response = client
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

    #[tokio::test]
    async fn same_origin_redirect_is_not_followed() {
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let source_addr = source.local_addr().unwrap();
        let source_thread = thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    format!("HTTP/1.1 302 Found\r\nLocation: http://{source_addr}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
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
    }

    /// Seals a plaintext the same way the ZCode app does, for decrypt tests.
    fn seal(plaintext: &str, iv: &[u8]) -> String {
        let key = Sha256::digest(TEST_SECRET.as_bytes());
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

    // Shape observed live from GET /api/monitor/usage/quota/limit on
    // 2026-09-27 with a GLM coding-plan key (plan level "lite").
    fn current_shape_body() -> Value {
        serde_json::json!({
            "code": 200,
            "msg": "Operation successful",
            "success": true,
            "data": {
                "level": "lite",
                "limits": [
                    {
                        "type": "CREDIT_LIMIT",
                        "unit": 3,
                        "number": 5,
                        "usage": 2000,
                        "currentValue": 0,
                        "remaining": 2000,
                        "percentage": 0
                    },
                    {
                        "type": "CREDIT_LIMIT",
                        "unit": 6,
                        "number": 1,
                        "usage": 10000,
                        "currentValue": 6495,
                        "remaining": 3504,
                        "percentage": 64,
                        "nextResetTime": 1790586723999_i64
                    }
                ]
            }
        })
    }

    #[test]
    fn decrypt_roundtrips_encrypted_and_plaintext_values() {
        let sealed = seal("plan-key-value", &[9u8; ENC_IV_LEN]);
        assert_eq!(
            decrypt_credential(&sealed, TEST_SECRET).unwrap(),
            "plan-key-value"
        );
        // Values without the prefix are stored as plaintext and pass through.
        assert_eq!(
            decrypt_credential("plain-key", TEST_SECRET).unwrap(),
            "plain-key"
        );
    }

    #[test]
    fn decrypt_rejects_tampered_or_malformed_values() {
        let sealed = seal("plan-key-value", &[9u8; ENC_IV_LEN]);
        let mut tampered = sealed.clone();
        tampered.replace_range(ENC_PREFIX.len() + 2..ENC_PREFIX.len() + 3, "A");
        assert_eq!(
            decrypt_credential(&tampered, TEST_SECRET).unwrap_err().code,
            "auth_unreadable"
        );
        assert!(decrypt_credential(&sealed, "wrong-secret").is_err());
        assert_eq!(
            decrypt_credential("enc:v1:not-an-envelope", TEST_SECRET)
                .unwrap_err()
                .code,
            "auth_unreadable"
        );
        assert_eq!(
            decrypt_credential("enc:v1:a.b.c", TEST_SECRET).unwrap_err().code,
            "auth_unreadable"
        );
    }

    #[test]
    fn parse_config_keys_prefers_enabled_zai_coding_plans() {
        let raw = serde_json::json!({
            "provider": {
                "builtin:zai": {
                    "kind": "anthropic",
                    "enabled": true,
                    "options": { "baseURL": "https://api.z.ai/api/anthropic", "apiKey": "" }
                },
                "builtin:zai-coding-plan": {
                    "kind": "anthropic",
                    "enabled": true,
                    "options": { "baseURL": "https://api.z.ai/api/anthropic", "apiKey": " config-key " }
                },
                "builtin:zai-start-plan": {
                    "kind": "anthropic",
                    "enabled": true,
                    "options": { "baseURL": "https://zcode.z.ai/api/v1/zcode-plan/anthropic", "apiKey": "zcode-plane-token" }
                },
                "builtin:bigmodel-coding-plan": {
                    "kind": "anthropic",
                    "enabled": true,
                    "options": { "baseURL": "https://open.bigmodel.cn/api/anthropic", "apiKey": "bigmodel-key" }
                },
                "custom-openai": {
                    "kind": "openai",
                    "enabled": true,
                    "options": { "apiKey": "other-key" }
                }
            }
        })
        .to_string();
        let keys = parse_config_keys(&raw).unwrap();
        // Only keys minted for api.z.ai qualify; the zcode.z.ai session token,
        // the bigmodel key, and the baseURL-less provider are never sent to
        // api.z.ai.
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, "config-key");
    }

    #[test]
    fn parse_config_keys_skips_missing_or_disabled_keys() {
        let raw = serde_json::json!({
            "provider": {
                "builtin:zai-coding-plan": {
                    "enabled": true,
                    "options": { "baseURL": "https://api.z.ai/api/anthropic", "apiKey": "   " }
                }
            }
        })
        .to_string();
        assert!(parse_config_keys(&raw).unwrap().is_empty());
        let disabled = serde_json::json!({
            "provider": {
                "builtin:zai-coding-plan": {
                    "enabled": false,
                    "options": { "baseURL": "https://api.z.ai/api/anthropic", "apiKey": "x" }
                }
            }
        })
        .to_string();
        assert!(parse_config_keys(&disabled).unwrap().is_empty());
        assert!(parse_config_keys(r#"{"provider":{}}"#).unwrap().is_empty());
        assert!(parse_config_keys(r#"{"other":1}"#).unwrap().is_empty());
        assert_eq!(parse_config_keys("not json").unwrap_err().code, "auth_unreadable");
    }

    #[test]
    fn provider_base_url_requires_https_scheme_on_api_z_ai() {
        let keys_for = |base: &str| {
            let raw = serde_json::json!({
                "provider": {
                    "p": {
                        "enabled": true,
                        "options": { "baseURL": base, "apiKey": "key" }
                    }
                }
            })
            .to_string();
            parse_config_keys(&raw).unwrap()
        };
        // The pinned scheme+host pair qualifies.
        assert_eq!(keys_for("https://api.z.ai/api/anthropic").len(), 1);
        assert_eq!(keys_for("HTTPS://API.Z.AI/api/anthropic").len(), 1);
        assert_eq!(keys_for("  https://api.z.ai  ").len(), 1);
        // Plain HTTP and unknown schemes are downgrades: never sent to api.z.ai.
        assert!(keys_for("http://api.z.ai/api/anthropic").is_empty());
        assert!(keys_for("ftp://api.z.ai/api/anthropic").is_empty());
        // Scheme-less bases cannot be scheme-checked and are rejected.
        assert!(keys_for("api.z.ai/api/anthropic").is_empty());
        assert!(keys_for(" ").is_empty());
        // Right scheme, wrong host: still excluded.
        assert!(keys_for("https://zcode.z.ai/api/v1/zcode-plan/anthropic").is_empty());
        assert!(keys_for("https://api.z.ai.evil.test/api/anthropic").is_empty());
    }

    #[test]
    fn parse_credentials_prefers_individual_then_team() {
        let raw = serde_json::json!({
            "oauth:zai:access_token": "jwt-token-value",
            "oauth:active_provider": "enc:v1:whatever",
            "account-provider:coding-plan:account:zai-team-coding-plan:account:uuid1:api-key": " team-key ",
            "account-provider:coding-plan:account:zai-individual-coding-plan:account:uuid1:api-key": "individual-key",
            "account-provider:coding-plan:account:bigmodel-coding-plan:account:uuid1:api-key": "bigmodel-key",
            "web-remote-control:external-relay:pass_hash": "hash"
        })
        .to_string();
        let keys = parse_credentials(&raw, TEST_SECRET).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].0, "individual-key");
        assert_eq!(keys[1].0, "team-key");
        // A non-Z.ai plan key must never become a candidate for api.z.ai.
        assert!(!keys.iter().any(|key| key.0 == "bigmodel-key"));
    }

    #[test]
    fn parse_credentials_decrypts_store_values() {
        let raw = serde_json::json!({
            "account-provider:coding-plan:account:zai-individual-coding-plan:account:uuid:api-key":
                seal("decrypted-plan-key", &[1u8; ENC_IV_LEN])
        })
        .to_string();
        let keys = parse_credentials(&raw, TEST_SECRET).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, "decrypted-plan-key");
    }

    #[test]
    fn parse_credentials_rejects_missing_or_undecryptable_credentials() {
        assert_eq!(
            parse_credentials(r#"{"oauth:zai:access_token":"jwt"}"#, TEST_SECRET)
                .unwrap_err()
                .code,
            "credential_missing"
        );
        assert_eq!(
            parse_credentials(
                r#"{"account-provider:coding-plan:account:zai-individual-coding-plan:account:uuid:api-key":"   "}"#,
                TEST_SECRET
            )
            .unwrap_err()
            .code,
            "credential_missing"
        );
        assert_eq!(
            parse_credentials(r#"{"some-api-key":"x"}"#, TEST_SECRET)
                .unwrap_err()
                .code,
            "credential_missing"
        );
        assert_eq!(
            parse_credentials("not json", TEST_SECRET)
                .unwrap_err()
                .code,
            "auth_unreadable"
        );
        assert_eq!(
            parse_credentials("[]", TEST_SECRET).unwrap_err().code,
            "credential_missing"
        );
        // Encrypted values that cannot be decrypted here leave no candidates.
        assert_eq!(
            parse_credentials(
                r#"{"account-provider:coding-plan:account:zai-individual-coding-plan:account:uuid:api-key":"enc:v1:aGVsbG8.AAAA.AAAA"}"#,
                TEST_SECRET
            )
            .unwrap_err()
            .code,
            "auth_unreadable"
        );
    }

    #[test]
    fn env_override_replaces_the_credential_store() {
        let store_raw = r#"{"account-provider:coding-plan:account:zai-individual-coding-plan:account:uuid:api-key":"store-key"}"#;
        let store = || parse_credentials(store_raw, TEST_SECRET);
        // A non-empty env key wins over the store.
        let merged = merge_env_and_store(Some("  env-key  ".to_string()), store()).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0, "env-key");
        // Blank/absent env keys fall through to the store.
        assert_eq!(
            merge_env_and_store(Some("   ".to_string()), store()).unwrap()[0].0,
            "store-key"
        );
        assert_eq!(merge_env_and_store(None, store()).unwrap()[0].0, "store-key");
        // Without an override, store errors pass through untouched.
        assert_eq!(
            merge_env_and_store(None, Err(zcode_not_installed()))
                .unwrap_err()
                .code,
            "zcode_not_installed"
        );
    }

    #[test]
    fn current_shape_parses_windows_and_derives_resets() {
        let body = current_shape_body().to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(
            limits
                .iter()
                .map(|limit| limit.label.as_str())
                .collect::<Vec<_>>(),
            ["5-hour", "Weekly"]
        );
        assert_eq!(limits[0].used_percent, 0.0);
        assert_eq!(limits[1].used_percent, 64.0);
        assert_eq!(limits[0].reset_at, None);
        // 1790586723999 ms == 2026-09-28T09:12:03Z (the UI renders local time)
        assert_eq!(limits[1].reset_at.as_deref(), Some("2026-09-28T09:12:03Z"));
    }

    #[test]
    fn legacy_reference_shape_still_parses() {
        // Shape documented by the zai-quota reference (other plan levels).
        let body = serde_json::json!({
            "success": true,
            "data": {
                "level": "standard",
                "limits": [
                    { "type": "TIME_LIMIT", "percentage": 42.5, "nextResetTime": 1790535600000_i64 },
                    { "type": "TOKENS_LIMIT", "percentage": 8.0 }
                ]
            }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(
            limits
                .iter()
                .map(|limit| limit.label.as_str())
                .collect::<Vec<_>>(),
            ["5-hour", "Tokens"]
        );
        assert_eq!(limits[0].used_percent, 42.5);
        // 1790535600000 ms == 2026-09-27T19:00:00Z
        assert_eq!(limits[0].reset_at.as_deref(), Some("2026-09-27T19:00:00Z"));
        assert_eq!(limits[1].reset_at, None);
    }

    #[test]
    fn root_payload_fallback_when_data_wrapper_missing() {
        let body = serde_json::json!({
            "limits": [
                { "type": "TIME_LIMIT", "percentage": 12.0, "nextResetTime": 1790535600000_i64 }
            ]
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].label, "5-hour");
        assert_eq!(limits[0].used_percent, 12.0);
    }

    #[test]
    fn logical_auth_failures_map_to_credential_errors() {
        // Live-observed: HTTP 200 + { code: 401, success: false }.
        let rejected = serde_json::json!({
            "code": 401,
            "msg": "token expired or incorrect",
            "success": false
        })
        .to_string();
        assert_eq!(
            parse_windows(rejected.as_bytes())
                .unwrap_err()
                .code,
            "auth_invalid"
        );

        let forbidden = serde_json::json!({ "code": 403, "msg": "forbidden", "success": false })
            .to_string();
        assert_eq!(
            parse_windows(forbidden.as_bytes())
                .unwrap_err()
                .code,
            "not_entitled"
        );

        let other = serde_json::json!({ "code": 500, "msg": "boom", "success": false })
            .to_string();
        assert_eq!(
            parse_windows(other.as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
    }

    #[test]
    fn windows_without_percentage_are_skipped_and_percent_clamps() {
        let body = serde_json::json!({
            "data": {
                "limits": [
                    { "type": "CREDIT_LIMIT", "percentage": 150.0 },
                    { "type": "CREDIT_LIMIT", "percentage": -5.0 },
                    { "type": "RATE_LIMIT" }
                ]
            }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].used_percent, 100.0);
        assert_eq!(limits[1].used_percent, 0.0);
    }

    #[test]
    fn unknown_window_kinds_get_readable_labels() {
        let body = serde_json::json!({
            "data": {
                "limits": [
                    { "type": "TOOL_CALLS_LIMIT", "percentage": 4.0 },
                    { "type": "CREDIT_LIMIT", "unit": 4, "number": 2, "percentage": 7.0 },
                    { "percentage": 9.0 }
                ]
            }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(limits[0].label, "TOOL CALLS LIMIT");
        assert_eq!(limits[1].label, "Daily");
        assert_eq!(limits[2].label, "UNKNOWN");
    }

    #[test]
    fn hostile_fallback_labels_are_sanitized_not_rendered() {
        // Ordinary unknown text still passes through readably — the gate
        // must not mangle benign labels.
        let body = serde_json::json!({
            "data": { "limits": [{ "type": "CUSTOM_GPU_LIMIT", "percentage": 4.0 }] }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(limits[0].label, "CUSTOM GPU LIMIT");

        // ANSI escapes disappear whole (no "[31m" residue), control
        // characters are removed, and whitespace runs collapse.
        let body = serde_json::json!({
            "data": { "limits": [{ "type": "\u{1b}[31mEVIL\u{1b}[0m  TOOL\u{0}CALLS", "percentage": 4.0 }] }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(limits[0].label, "EVIL TOOLCALLS");
        assert!(!limits[0].label.contains('\u{1b}'));
        assert!(!limits[0].label.chars().any(char::is_control));

        // An escape-only type carries no visible text and degrades to the
        // same label a missing type uses.
        let body = serde_json::json!({
            "data": { "limits": [{ "type": "\u{1b}[2J", "percentage": 1.0 }] }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(limits[0].label, "UNKNOWN");

        // Overlong input is capped deterministically.
        let long = "x".repeat(60);
        let body = serde_json::json!({
            "data": { "limits": [{ "type": long, "percentage": 1.0 }] }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(limits[0].label.chars().count(), MAX_FALLBACK_LABEL_CHARS);
    }

    #[test]
    fn failed_envelope_without_a_code_is_a_schema_error() {
        // `{ success: false }` with no numeric code is neither a usable
        // payload nor a classifiable refusal: it must surface as a
        // deterministic schema error, never as an empty success.
        let body = serde_json::json!({ "success": false }).to_string();
        let error = parse_windows(body.as_bytes()).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.transient, Some(false));
    }

    #[test]
    fn reset_timestamps_reject_implausible_magnitudes() {
        let body = serde_json::json!({
            "data": {
                "limits": [
                    // epoch seconds are promoted to milliseconds
                    { "percentage": 1.0, "nextResetTime": 1790535600 },
                    // post-2100 or pre-1973 magnitudes are not usable dates
                    { "percentage": 1.0, "nextResetTime": 4_200_000_000_000_i64 },
                    { "percentage": 1.0, "nextResetTime": 99_999_999_999_i64 },
                    { "percentage": 1.0, "nextResetTime": -1 }
                ]
            }
        })
        .to_string();
        let limits = parse_windows(body.as_bytes()).unwrap();
        assert_eq!(limits[0].reset_at.as_deref(), Some("2026-09-27T19:00:00Z"));
        assert_eq!(limits[1].reset_at, None);
        assert_eq!(limits[2].reset_at, None);
        assert_eq!(limits[3].reset_at, None);
    }

    #[test]
    fn html_body_and_empty_limits_are_rejected() {
        let html = b"<html><body>Please sign in</body></html>";
        assert_eq!(
            parse_windows(html).unwrap_err().code,
            "unexpected_response"
        );

        let empty = serde_json::json!({ "data": { "limits": [] } }).to_string();
        assert_eq!(
            parse_windows(empty.as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );

        let no_limits = serde_json::json!({ "data": { "level": "standard" } }).to_string();
        assert_eq!(
            parse_windows(no_limits.as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
    }

    #[test]
    fn credential_rejection_codes_trigger_fallback() {
        assert!(is_credential_rejected(&auth_invalid()));
        assert!(is_credential_rejected(&not_entitled()));
        assert!(!is_credential_rejected(&ProviderError::new("network", "boom")));
        assert!(!is_credential_rejected(&ProviderError::new(
            "unexpected_response",
            "boom"
        )));
    }

    /// The structured retry verdict (`transient`/`httpStatus`) must mirror
    /// what the frontend used to infer from message wording.
    #[test]
    fn error_metadata_matches_the_retry_contract() {
        assert_eq!(ProviderError::transient("network", "offline").transient, Some(true));
        assert_eq!(auth_invalid().transient, Some(false));
        assert_eq!(not_entitled().transient, Some(false));
        assert_eq!(credential_missing().transient, Some(false));
        assert_eq!(
            ProviderError::http_failure(
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                "Z.ai usage endpoint returned HTTP 429 Too Many Requests.",
            )
            .transient,
            Some(true)
        );
        assert_eq!(
            ProviderError::http_failure(
                reqwest::StatusCode::BAD_GATEWAY,
                "Z.ai usage endpoint returned HTTP 502 Bad Gateway.",
            )
            .http_status,
            Some(502)
        );
        assert_eq!(
            ProviderError::http_failure(
                reqwest::StatusCode::NOT_FOUND,
                "Z.ai usage endpoint returned HTTP 404 Not Found.",
            )
            .transient,
            Some(false)
        );
    }

    #[test]
    fn envelope_errors_classify_transiency_by_code() {
        let envelope = |code: i64| {
            serde_json::json!({ "code": code, "msg": "boom", "success": false }).to_string()
        };
        // 429/5xx envelope refusals are transient; other codes are not.
        assert_eq!(
            parse_windows(envelope(429).as_bytes()).unwrap_err().transient,
            Some(true)
        );
        assert_eq!(
            parse_windows(envelope(500).as_bytes()).unwrap_err().transient,
            Some(true)
        );
        assert_eq!(
            parse_windows(envelope(400).as_bytes()).unwrap_err().transient,
            Some(false)
        );
        // Auth envelope refusals stay deterministic (key fallback handles them).
        assert_eq!(
            parse_windows(envelope(401).as_bytes()).unwrap_err().transient,
            Some(false)
        );
    }

    /// The envelope `msg` is provider-controlled free text that reaches a
    /// user-visible message, so it must pass the same display-safety gate the
    /// fallback labels use: hostile text is stripped and capped, benign text
    /// is unchanged, and the retry classification is unaffected.
    #[test]
    fn envelope_upstream_message_is_sanitized_before_display() {
        let envelope = |code: i64, msg: &str| {
            serde_json::json!({ "code": code, "msg": msg, "success": false }).to_string()
        };

        // Ordinary text still passes through readably.
        let error =
            parse_windows(envelope(400, "quota exceeded for this plan").as_bytes()).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(
            error.message,
            "Z.ai usage endpoint rejected the request (code 400): quota exceeded for this plan"
        );

        // ANSI escapes disappear whole (no "[31m" residue) and control
        // characters are removed.
        let hostile = "\u{1b}[31mEVIL\u{1b}[0m  injected\u{0}text";
        let error = parse_windows(envelope(400, hostile).as_bytes()).unwrap_err();
        assert_eq!(
            error.message,
            "Z.ai usage endpoint rejected the request (code 400): EVIL injectedtext"
        );
        assert!(!error.message.contains('\u{1b}'));
        assert!(!error.message.chars().any(char::is_control));

        // Overlong upstream text is capped deterministically.
        let long = "x".repeat(MAX_UPSTREAM_DETAIL_CHARS + 50);
        let error = parse_windows(envelope(400, &long).as_bytes()).unwrap_err();
        let detail_len = error.message.split(": ").last().unwrap().chars().count();
        assert_eq!(detail_len, MAX_UPSTREAM_DETAIL_CHARS);

        // Empty or escape-only text degrades to the authored fallback, never
        // an empty tail.
        let escape_only = parse_windows(envelope(400, "\u{1b}[2J").as_bytes()).unwrap_err();
        assert!(
            escape_only.message.ends_with(": request failed"),
            "{}",
            escape_only.message
        );
        let absent = parse_windows(br#"{"code":400,"success":false}"#).unwrap_err();
        assert!(
            absent.message.ends_with(": request failed"),
            "{}",
            absent.message
        );

        // Sanitizing the detail never changes the retry verdict.
        assert_eq!(
            parse_windows(envelope(500, hostile).as_bytes())
                .unwrap_err()
                .transient,
            Some(true)
        );
        assert_eq!(
            parse_windows(envelope(400, hostile).as_bytes())
                .unwrap_err()
                .transient,
            Some(false)
        );
    }

    #[test]
    fn error_wire_format_is_camel_case_and_omits_absent_metadata() {
        let wire = serde_json::to_string(&ProviderError::http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Z.ai usage endpoint returned HTTP 429 Too Many Requests.",
        ))
        .unwrap();
        assert!(wire.contains("\"httpStatus\":429"), "wire: {wire}");
        assert!(wire.contains("\"transient\":true"), "wire: {wire}");

        let wire = serde_json::to_string(&credential_missing()).unwrap();
        assert!(wire.contains("\"transient\":false"), "wire: {wire}");
        assert!(!wire.contains("httpStatus"), "wire: {wire}");
        // Error messages must not embed the API key.
        assert!(!wire.contains("config-key") && !wire.contains("individual-key"), "wire: {wire}");
    }

    #[test]
    fn wire_format_is_camel_case_and_never_contains_the_key() {
        // The attribution is derived from a real full-key sentinel so the
        // negative assertions below discriminate: the key demonstrably
        // existed and its masked hint is the only credential-derived
        // material allowed on the wire.
        let key = "config-key-12345678";
        let usage = ZaiUsage {
            limits: parse_windows(current_shape_body().to_string().as_bytes())
                .unwrap(),
            account: account_hint(key).map(|key_hint| ZaiAccount { key_hint }),
        };
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(wire.contains("usedPercent"), "wire format must be camelCase: {wire}");
        assert!(wire.contains("resetAt"), "wire format must be camelCase: {wire}");
        assert!(!wire.contains("used_percent"));
        // The masked hint is on the wire; the full key never is.
        assert!(wire.contains("\"keyHint\":\"5678\""), "wire: {wire}");
        // The credential must never travel toward the WebView.
        assert!(!wire.contains("config-key"));
        assert!(!wire.contains("individual-key"));
    }

    #[test]
    fn key_hint_masks_the_key_tail() {
        assert_eq!(account_hint("abcd1234efgh5678").as_deref(), Some("5678"));
        // Deterministic per credential: the same key derives the same hint.
        assert_eq!(account_hint("abcd1234efgh5678"), account_hint("abcd1234efgh5678"));
        // Too short to mask meaningfully — no attribution rather than a hint
        // that exposes most of the key.
        assert_eq!(account_hint("short-key"), None);
        assert_eq!(account_hint(""), None);
        assert_eq!(account_hint("abcd1234efg"), None);
        // Whitespace or punctuation in the tail is not key material.
        assert_eq!(account_hint("abcd1234efgh56 8"), None);
        assert_eq!(account_hint("abcd1234efgh56.8"), None);
        // Dashes/underscores are common in key alphabets and stay usable.
        assert_eq!(account_hint("abcd1234efgh5_-8").as_deref(), Some("5_-8"));
    }

    // ---------- candidate attribution (identity hardening) ----------
    //
    // The resolution loop tries stored credential candidates in order; every
    // outcome must be attributed to the credential ACTUALLY attempted, so the
    // runtime's last-good guard can tell a failure of key A apart from a
    // failure that arrived while key B was in flight.

    fn window() -> ZaiLimitWindow {
        ZaiLimitWindow {
            label: "5-hour".to_string(),
            used_percent: 0.0,
            reset_at: None,
        }
    }

    fn keys() -> Vec<ZaiKey> {
        vec![
            ZaiKey("key-one-abcd1234".to_string()),
            ZaiKey("key-two-wxyz9012".to_string()),
        ]
    }

    /// Builds a candidate fetcher from per-key outcomes (key value → result).
    fn outcomes(
        per_key: Vec<(&'static str, Result<Vec<ZaiLimitWindow>, ProviderError>)>,
    ) -> impl for<'a> Fn(
        &'a ZaiKey,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ZaiLimitWindow>, ProviderError>> + Send + 'a>>
    {
        move |key: &ZaiKey| {
            let result = per_key
                .iter()
                .find(|(value, _)| *value == key.0)
                .map(|(_, result)| result.clone())
                .unwrap_or_else(|| Err(credential_missing()));
            Box::pin(async move { result })
        }
    }

    #[tokio::test]
    async fn winning_candidate_becomes_the_account_attribution() {
        // The first key is refused; the second answers. The card names the
        // credential that actually produced the windows.
        let usage = resolve_usage(
            &keys(),
            outcomes(vec![
                ("key-one-abcd1234", Err(auth_invalid())),
                ("key-two-wxyz9012", Ok(vec![window()])),
            ]),
        )
        .await
        .unwrap();
        assert_eq!(usage.account.as_ref().unwrap().key_hint, "9012");

        // Single winning candidate: same derivation.
        let usage = resolve_usage(
            &[ZaiKey("key-one-abcd1234".to_string())],
            outcomes(vec![("key-one-abcd1234", Ok(vec![window()]))]),
        )
        .await
        .unwrap();
        assert_eq!(usage.account.as_ref().unwrap().key_hint, "1234");
    }

    #[tokio::test]
    async fn final_failure_after_rejections_names_the_last_attempted_candidate() {
        // Both candidates refused: the surfaced error is the LAST candidate's
        // (its verdict is the one the endpoint gave for the key in flight).
        let error = resolve_usage(
            &keys(),
            outcomes(vec![
                ("key-one-abcd1234", Err(auth_invalid())),
                ("key-two-wxyz9012", Err(not_entitled())),
            ]),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "not_entitled");
        assert_eq!(error.identity_hint.as_deref(), Some("key:9012"));

        // Refusal followed by a non-credential failure: the in-flight
        // candidate is attributed, not the refused one.
        let error = resolve_usage(
            &keys(),
            outcomes(vec![
                ("key-one-abcd1234", Err(auth_invalid())),
                ("key-two-wxyz9012", Err(ProviderError::transient("network", "offline"))),
            ]),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "network");
        assert_eq!(error.identity_hint.as_deref(), Some("key:9012"));
    }

    #[tokio::test]
    async fn candidate_failure_message_is_scrubbed_against_the_live_key() {
        // Leak simulation: an upstream echoing the in-flight candidate's own
        // credential back inside its error detail. The surfaced message must
        // no longer contain the key value, while the benign carrier text and
        // the identity attribution survive untouched.
        const ECHOED: &str = "echo-key-value-0123456789";
        let error = resolve_usage(
            &[ZaiKey(ECHOED.to_string())],
            outcomes(vec![(
                ECHOED,
                Err(ProviderError::new(
                    "unexpected_response",
                    format!("endpoint refused the request (token {ECHOED}): try again"),
                )),
            )]),
        )
        .await
        .unwrap_err();
        assert!(!error.message.contains(ECHOED), "{}", error.message);
        assert!(
            error
                .message
                .contains("endpoint refused the request (token [REDACTED]): try again"),
            "{}",
            error.message
        );
        assert_eq!(error.identity_hint.as_deref(), Some("key:6789"));
    }

    #[tokio::test]
    async fn failure_without_a_rejection_stops_on_the_first_candidate() {
        // A network failure on the first candidate never advances; its
        // identity must be the first candidate's.
        let error = resolve_usage(
            &keys(),
            outcomes(vec![(
                "key-one-abcd1234",
                Err(ProviderError::transient("network", "offline")),
            )]),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "network");
        assert_eq!(error.identity_hint.as_deref(), Some("key:1234"));
    }

    #[tokio::test]
    async fn unmaskable_credentials_stay_unattributed() {
        // A key too short to mask safely derives no identity anywhere: the
        // runtime keeps its conservative retention behavior.
        let error = resolve_usage(
            &[ZaiKey("short-key".to_string())],
            outcomes(vec![("short-key", Err(auth_invalid()))]),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "auth_invalid");
        assert_eq!(error.identity_hint, None);

        let usage = resolve_usage(
            &[ZaiKey("short-key".to_string())],
            outcomes(vec![("short-key", Ok(vec![window()]))]),
        )
        .await
        .unwrap();
        assert_eq!(usage.account, None);
    }

    /// The identity string the failure paths stamp must match the identity
    /// the success attribution carries (the runtime guard compares them).
    #[test]
    fn identity_format_matches_the_success_attribution_input() {
        assert_eq!(
            key_identity(&ZaiKey("abcd1234efgh5678".to_string())),
            Some("key:5678".to_string())
        );
        assert_eq!(key_identity(&ZaiKey("short-key".to_string())), None);
    }

    /// The usage wire carries only the masked hint — never the key.
    #[test]
    fn usage_attribution_carries_only_the_key_tail() {
        let usage = ZaiUsage {
            limits: vec![window()],
            account: Some(ZaiAccount {
                key_hint: "5678".to_string(),
            }),
        };
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(wire.contains("\"account\":{\"keyHint\":\"5678\"}"), "wire: {wire}");
        assert!(!wire.contains("abcd1234efgh5678"), "wire: {wire}");
    }

    #[test]
    fn key_debug_never_leaks_the_secret() {
        let key = ZaiKey("super-secret-zai-key".to_string());
        let debug = format!("{key:?}");
        // The Debug rendering must actually happen — an empty rendering
        // would make the leak check below vacuous.
        assert!(debug.contains("ZaiKey"), "debug must render the type: {debug}");
        assert!(!debug.contains("super-secret-zai-key"));
    }

    #[test]
    #[ignore = "live test: requires a real local Z.ai plan key and network access"]
    fn live_fetch_returns_windows() {
        let usage = tauri::async_runtime::block_on(fetch_zai_usage()).expect("live fetch");
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
