//! ZCode plan/balance observations — passive supplemental read (production).
//!
//! Observes the promotional/temporary ZCode plans (e.g. Start Plan / ZCode
//! Trust Build packages) via
//! `GET https://zcode.z.ai/api/v1/zcode-plan/billing/balance?app_version=<v>`
//! — the endpoint the ZCode app itself calls for its Start Plan usage view
//! (request shape verified 2026-10-06 from the installed app's
//! `fetchZaiStartPlanBalanceEnvelope` / `buildZCodeSourceHeaders` /
//! `withRequestIdHeader` bundle code; the same day reproduced against the
//! live endpoint through this exact code path).
//!
//! This is a SIBLING of `zcode_reset` (the free reset-card observer), not a
//! replacement: the ordinary Z.ai coding-plan windows stay sourced from the
//! `api.z.ai` monitor endpoint, and this module only adds plan-grouped
//! absolute balances that the monitor endpoint does not carry.
//!
//! Passive credential contract — identical to `zcode_reset`: ZCode owns
//! login, refresh, rotation, and all credential-store writes; this module
//! only ever reads. The endpoint authenticates with the ZCode session JWT
//! alone (`zcodejwttoken`, live-verified byte-identical to the app's
//! `builtin:zai-start-plan` API key on this machine) — loaded through the
//! shared `zcode_reset::load_session_jwt` path, decrypted in this process
//! with the app-derived `enc:v1` key. No refresh, no rotation, no retry
//! loop: exactly one GET per invocation.
//!
//! The non-secret static request headers reproduce the installed app's
//! `buildZCodeSourceHeaders` family, with every device-derived value read
//! from local state (never guessed):
//! - `User-Agent: ZCode/<version>` + `X-ZCode-App-Version` — the installed
//!   app version, read from the Windows uninstall registry entry;
//! - `X-Device-Mid` — the `deviceMid` of `~/.zcode/v2/telemetry-state.json`;
//! - `X-Os-Version` — the Windows release from the registry;
//! - `X-Platform` / `X-Os-Category` — derived from `std::env::consts`;
//! - `X-Client-Language` / `X-Client-Timezone` — the OS locale and IANA
//!   timezone;
//! - `X-Release-Channel` — `ZCODE_ENV` or `stable`;
//! - `X-Request-Id` — generated fresh per request (the app does the same).
//! A value that cannot be derived locally is omitted, exactly like the
//! app's own builder degrades.
//!
//! The module's ONLY upstream URL is the read-only balance GET. Raw
//! response bodies are never logged: body bytes flow only into the parser,
//! and errors carry at most a sanitized envelope message or an HTTP status.
//!
//! Wire exposure rides the Z.ai provider's snapshot entry
//! (`ProviderUsageDto::zcode_plans`), refreshed by the ordinary runtime
//! cycle — no second scheduler, no tauri command of its own. A failed
//! observation never fails the Z.ai quota refresh, and a fresh observation
//! is retained (within the TTL below) when a later observation fails,
//! mirroring the reset-card freshness convention.
//!
//! Semantics note: plans are grouped exactly as upstream reports them.
//! Balance rows are attributed to a plan by `user_plan_id` (falling back to
//! `plan_id`), mirroring the app's own mapping. Buckets that cannot be
//! attributed to an active plan are dropped — never merged across plans,
//! never summed across units. Absolute `total/used/remaining` values are
//! preserved as reported; nothing is forced into the monitor endpoint's
//! percentage-only shape.

use std::fs;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::provider_error::ProviderError;
use crate::zai::{
    auth_invalid, epoch_ms_to_rfc3339, http_client, not_entitled, sanitize_display_label,
    sanitize_upstream_detail,
};
use crate::zcode_reset::{bearer_session_value, load_session_jwt};

const PLAN_BALANCE_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/balance";

/// The app's own endpoint origin constant (`Po` in the installed bundle):
/// the referer it sends and the origin its source headers are bound to.
const ZCODE_ENDPOINT_ORIGIN: &str = "https://zcode.z.ai";

/// Freshness budget for a retained plan/balance observation. Mirrors
/// `zcode_reset::OBSERVATION_TTL_SECS` (and `codex::RESET_CREDITS_TTL_SECS`):
/// the endpoint is re-queried live every refresh cycle (default 5 minutes),
/// and a consumed bucket is visible only on the next fetch. An observation
/// older than roughly three default cycles must stop reading as current.
pub const OBSERVATION_TTL_SECS: i64 = 15 * 60;
/// Clock-skew tolerance for observation timestamps (5 minutes), matching the
/// reset-card observation and the persisted last-good store.
const OBSERVED_FUTURE_TOLERANCE_SECS: i64 = 300;

// ---------- data returned to the WebView (camelCase on the wire) ----------

/// One balance bucket under one active plan: an absolute pool for one meter
/// (e.g. tokens for one model, or credits) with its own period/expiry.
/// Absolute upstream values are preserved — `limit`/`used`/`remaining` are
/// raw counts, never percentages, and are never summed across units.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodePlanBalance {
    /// Stable upstream identifiers (never rendered by the normal UI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_plan_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entitlement_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_id: Option<String>,
    /// Display model for the bucket: the `model:`-prefixed capability when
    /// upstream names one, else the bucket's `show_name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Upstream meter classification (`meter`), when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meter: Option<String>,
    /// Upstream unit type (`unit_type`, e.g. `token` / `credit`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Absolute pool size (`total_units`), as reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    /// Absolute consumption (`used_units`), as reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    /// Absolute remainder (`remaining_units`), as reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    /// Upstream period of the bucket (`period`, else the owning plan
    /// entitlement's period — e.g. `one_time`, `daily`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    /// RFC 3339 UTC end of the bucket's current period, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_end: Option<String>,
    /// RFC 3339 UTC expiry of the bucket, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// One active ZCode plan/package, with the balance buckets attributed to it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodePlan {
    /// Stable upstream identifier (never rendered by the normal UI).
    pub plan_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_plan_id: Option<String>,
    /// Upstream display name, sanitized for the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The raw upstream status that qualified this plan (only active plans
    /// are carried — classification is upstream's, never a name heuristic).
    pub status: String,
    /// RFC 3339 UTC expiry of the plan (`ends_at`), when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<String>,
    pub balances: Vec<ZCodePlanBalance>,
}

/// The full supplemental observation: every currently active ZCode plan with
/// its plan-grouped balances.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodePlansObservation {
    pub plans: Vec<ZCodePlan>,
    /// Internal observation stamp (never serialized): when this observation
    /// was actually observed upstream. Same contract as `zcode_reset`.
    #[serde(skip)]
    pub observed_at: DateTime<Utc>,
}

/// The safe facts the live proof records: counts, sanitized plan names, and
/// unit kinds only — no credential material, no account identifiers, no raw
/// response bodies.
#[derive(Debug, Clone)]
pub struct PlanBalanceFacts {
    pub active_plan_count: usize,
    /// Balance rows attributed to an active plan (the rows that display).
    pub mapped_bucket_count: usize,
    /// Balance rows with no matching active plan upstream — dropped, never
    /// guessed onto a plan.
    pub orphan_bucket_count: usize,
    /// Sanitized display names of the active plans, upstream order.
    pub active_plan_names: Vec<String>,
    /// Distinct unit types observed (first-seen order), e.g. ["token"].
    pub unit_types: Vec<String>,
}

// ---------- request context (app-faithful, locally derived) ----------

/// The app's own value normalizer (`normalizeZCodeSourceHeaderValue`): trim,
/// keep only non-empty printable-ASCII values, drop everything else so a
/// derived value can never inject into a header.
fn normalize_source_value(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return None;
    }
    Some(trimmed.to_string())
}

/// `process.platform` as the app reports it to the endpoint.
fn platform_value() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// The app's `normalizeOsCategory` mapping for `X-Os-Category`.
fn os_category(platform: &str) -> &'static str {
    match platform {
        "darwin" => "macos",
        "win32" => "windows",
        _ => "linux",
    }
}

/// `process.arch` equivalent for `X-Platform`.
fn arch_value() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        std::env::consts::ARCH
    }
}

/// `X-Release-Channel`: the app derives the channel from `ZCODE_ENV` when
/// set, else `stable`. Pure core for tests.
fn release_channel_from(env: Option<&str>) -> String {
    env.and_then(|value| normalize_source_value(Some(value)))
        .unwrap_or_else(|| "stable".to_string())
}

fn release_channel() -> String {
    release_channel_from(std::env::var("ZCODE_ENV").ok().as_deref())
}

/// `X-Client-Language`: the OS locale, as the app resolves it via `Intl`.
fn client_language() -> String {
    sys_locale::get_locale()
        .and_then(|locale| normalize_source_value(Some(&locale)))
        .unwrap_or_else(|| "unknown".to_string())
}

/// `X-Client-Timezone`: the IANA timezone, as the app resolves it via `Intl`.
fn client_timezone() -> String {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|tz| normalize_source_value(Some(&tz)))
        .unwrap_or_else(|| "unknown".to_string())
}

/// `X-Device-Mid`: the device identifier the app itself persists in
/// `~/.zcode/v2/telemetry-state.json` (read-only; the app is the only
/// writer). Absent from the file means absent from the request, exactly
/// like the app's builder degrades.
fn device_mid() -> Option<String> {
    let path = crate::zai::zcode_dir().ok()?.join("telemetry-state.json");
    let raw = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    normalize_source_value(value.get("deviceMid").and_then(Value::as_str))
}

#[cfg(windows)]
mod win_registry {
    use super::normalize_source_value;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, KEY_READ, REG_SZ,
    };

    fn read_value(root: HKEY, path: &str, name: &str) -> Option<String> {
        let key = open(root, path)?;
        let result = read_sz(key, name);
        unsafe { RegCloseKey(key) };
        result
    }

    fn open(root: HKEY, path: &str) -> Option<HKEY> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut key: HKEY = std::ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(root, wide.as_ptr(), 0, KEY_READ, &mut key) };
        (status == 0).then_some(key)
    }

    fn read_sz(key: HKEY, name: &str) -> Option<String> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut kind: u32 = 0;
        let mut bytes: u32 = 0;
        let status = unsafe {
            RegQueryValueExW(
                key,
                wide.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut bytes,
            )
        };
        if status != 0 || kind != REG_SZ || bytes == 0 || bytes > 32 * 1024 {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize + 1) / 2];
        let status = unsafe {
            RegQueryValueExW(
                key,
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != 0 {
            return None;
        }
        let len = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        let text = OsString::from_wide(&buffer[..len])
            .to_string_lossy()
            .into_owned();
        normalize_source_value(Some(&text))
    }

    /// The `name = value` REG_SZ under `root\path`, read-only.
    pub(super) fn value(root: HKEY, path: &str, name: &str) -> Option<String> {
        read_value(root, path, name)
    }

    /// The first REG_SZ `value_name` across the direct subkeys of
    /// `root\path` whose `filter_key` REG_SZ contains `contains`
    /// (case-insensitive).
    pub(super) fn first_subkey_value_matching(
        root: HKEY,
        path: &str,
        filter_key: &str,
        contains: &str,
        value_name: &str,
    ) -> Option<String> {
        let key = open(root, path)?;
        let result = (|| {
            let contains_lower = contains.to_ascii_lowercase();
            for index in 0..4096u32 {
                let mut name_buffer = [0u16; 256];
                let mut name_len = name_buffer.len() as u32;
                let status = unsafe {
                    RegEnumKeyExW(
                        key,
                        index,
                        name_buffer.as_mut_ptr(),
                        &mut name_len,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if status != 0 {
                    // ERROR_NO_MORE_ITEMS or an unexpected failure: enumeration ends.
                    return None;
                }
                let subkey = OsString::from_wide(&name_buffer[..name_len as usize])
                    .to_string_lossy()
                    .into_owned();
                let subkey_path = format!("{path}\\{subkey}");
                if let Some(display) = read_value(CURRENT_USER, &subkey_path, filter_key) {
                    if display.to_ascii_lowercase().contains(&contains_lower) {
                        return read_value(CURRENT_USER, &subkey_path, value_name);
                    }
                }
            }
            None
        })();
        unsafe { RegCloseKey(key) };
        result
    }

    pub(super) use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER as CURRENT_USER, HKEY_LOCAL_MACHINE as LOCAL_MACHINE,
    };
}

/// The installed ZCode app version — the value the app sends as
/// `User-Agent: ZCode/<version>`, `X-ZCode-App-Version`, and the
/// `app_version` query parameter. Read from the app's own Windows uninstall
/// registration (`DisplayVersion` of the ZCode entry); `None` when it cannot
/// be derived, in which case the request degrades like the app's own builder
/// (`User-Agent: ZCode/unknown`, no `X-ZCode-App-Version`, no query param).
#[cfg(windows)]
fn app_release_version() -> Option<String> {
    use win_registry::{first_subkey_value_matching, CURRENT_USER};
    let version = first_subkey_value_matching(
        CURRENT_USER,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        "DisplayName",
        "zcode",
        "DisplayVersion",
    )?;
    // A version-like sanity gate: the app's own constant is semver-shaped.
    let head = version.split('.').next().unwrap_or_default();
    (!head.is_empty() && head.chars().all(|c| c.is_ascii_digit())).then_some(version)
}

#[cfg(not(windows))]
fn app_release_version() -> Option<String> {
    // No passive local source for the installed app version off-Windows; the
    // request degrades exactly like the app's own unknown-version builder.
    None
}

/// `X-Os-Version`, mirroring the app's `os.release()` value on Windows
/// (`major.minor.build`, e.g. `10.0.26300`), read from the registry.
#[cfg(windows)]
fn os_version() -> Option<String> {
    use win_registry::{value, LOCAL_MACHINE};
    let current = value(
        LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentVersion",
    )?;
    let build = value(
        LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuildNumber",
    )?;
    let version = format!("{current}.{build}");
    normalize_source_value(Some(&version))
}

#[cfg(not(windows))]
fn os_version() -> Option<String> {
    None
}

/// One static source header, assembled from locally derived context. Every
/// value passes the app's printable-ASCII normalizer first. A missing value
/// omits its header — the app's builder degrades the same way; nothing is
/// ever guessed.
fn build_source_headers(context: &SourceHeaderContext) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "user-agent".to_string(),
            format!(
                "ZCode/{}",
                context.app_version.as_deref().unwrap_or("unknown")
            ),
        ),
        (
            "http-referer".to_string(),
            ZCODE_ENDPOINT_ORIGIN.to_string(),
        ),
        ("x-title".to_string(), "Z Code@electron".to_string()),
    ];
    if let Some(version) = &context.app_version {
        headers.push(("x-zcode-app-version".to_string(), version.clone()));
    }
    // X-Platform = `<platform>-<arch>` (both static; always present here).
    headers.push((
        "x-platform".to_string(),
        format!("{}-{}", platform_value(), arch_value()),
    ));
    headers.push(("x-release-channel".to_string(), release_channel()));
    headers.push(("x-client-language".to_string(), client_language()));
    headers.push(("x-client-timezone".to_string(), client_timezone()));
    headers.push((
        "x-os-category".to_string(),
        os_category(platform_value()).to_string(),
    ));
    if let Some(os_version) = &context.os_version {
        headers.push(("x-os-version".to_string(), os_version.clone()));
    }
    if let Some(device_mid) = &context.device_mid {
        headers.push(("x-device-mid".to_string(), device_mid.clone()));
    }
    headers
}

/// The locally derived values that vary per machine; injected in tests so
/// the assembly stays pure.
struct SourceHeaderContext {
    app_version: Option<String>,
    os_version: Option<String>,
    device_mid: Option<String>,
}

fn current_source_context() -> SourceHeaderContext {
    SourceHeaderContext {
        app_version: app_release_version(),
        os_version: os_version(),
        device_mid: device_mid(),
    }
}

/// The full request header set for one balance GET: the app's static source
/// headers, then the session credential and request-scoped identity.
fn request_headers(
    credentials: &crate::zcode_reset::ZcodeSessionJwt,
) -> Result<Vec<(String, String)>, ProviderError> {
    let jwt = credentials.0.trim();
    if jwt.is_empty() {
        return Err(crate::zai::credential_missing());
    }
    let mut headers = build_source_headers(&current_source_context());
    headers.push(("authorization".to_string(), bearer_session_value(jwt)));
    headers.push(("accept".to_string(), "application/json".to_string()));
    // Request-scoped identity, generated fresh per request (the app does
    // the same via `withRequestIdHeader`); never persisted, never logged.
    headers.push(("x-request-id".to_string(), uuid::Uuid::new_v4().to_string()));
    Ok(headers)
}

fn request_header_map(
    credentials: &crate::zcode_reset::ZcodeSessionJwt,
) -> Result<reqwest::header::HeaderMap, ProviderError> {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in request_headers(credentials)? {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .expect("hardcoded header names are valid");
        let value = reqwest::header::HeaderValue::from_str(&value).map_err(|_| {
            ProviderError::new(
                "auth_unreadable",
                "ZCode request header material is not header-safe.",
            )
        })?;
        map.insert(name, value);
    }
    Ok(map)
}

/// The balance URL with the app's `app_version` query parameter. The app
/// always sends its bundled version; when this machine's version cannot be
/// derived, the parameter is omitted rather than fabricated.
fn balance_url(app_version: Option<&str>) -> String {
    match app_version {
        Some(version) => format!("{PLAN_BALANCE_URL}?app_version={version}"),
        None => PLAN_BALANCE_URL.to_string(),
    }
}

// ---------- upstream response (only what we consume; extra ignored) ----------
//
// Wire shape verified 2026-10-06 from the installed app's own zod schema and
// normalizer (`normalizeZaiStartPlanBalanceLimits`): envelope
// `{ code, msg?, data? }` with code 0 = success, and `data`:
// {
//   server_time: number,                  // unix seconds
//   plans: [{
//     plan_id, user_plan_id?, name?, description?, priority?, status?,
//     starts_at?, ends_at?,               // unix seconds
//     entitlements?: [{ entitlement_id?, show_name?, period?, effective_at? }]
//   }],
//   balances: [{
//     plan_id?, user_plan_id?, entitlement_id?, bucket_id?, show_name?,
//     capabilities?: ["model:GLM-5.3-Flash", ...], meter?, unit_type?,
//     total_units?, used_units?, remaining_units?, available_units?,
//     reserved_units?,                    // unix seconds below
//     period?, period_start?, period_end?, expires_at?
//   }]
// }

#[derive(Debug, serde::Deserialize)]
struct PlanBalancePayload {
    #[serde(default)]
    plans: Option<Vec<PlanEntry>>,
    #[serde(default)]
    balances: Option<Vec<BalanceEntry>>,
    // Unknown upstream fields (future additions) are tolerated, never parsed.
    #[serde(flatten)]
    _extra: Value,
}

#[derive(Debug, serde::Deserialize)]
struct PlanEntry {
    #[serde(default, rename = "plan_id")]
    plan_id: Option<Value>,
    #[serde(default, rename = "user_plan_id")]
    user_plan_id: Option<Value>,
    #[serde(default)]
    name: Option<Value>,
    #[serde(default)]
    status: Option<Value>,
    #[serde(default, rename = "ends_at")]
    ends_at: Option<Value>,
    #[serde(default)]
    entitlements: Option<Vec<EntitlementEntry>>,
    #[serde(flatten)]
    _extra: Value,
}

#[derive(Debug, serde::Deserialize)]
struct EntitlementEntry {
    #[serde(default, rename = "entitlement_id")]
    entitlement_id: Option<Value>,
    #[serde(default)]
    period: Option<Value>,
    #[serde(flatten)]
    _extra: Value,
}

#[derive(Debug, serde::Deserialize)]
struct BalanceEntry {
    #[serde(default, rename = "plan_id")]
    plan_id: Option<Value>,
    #[serde(default, rename = "user_plan_id")]
    user_plan_id: Option<Value>,
    #[serde(default, rename = "entitlement_id")]
    entitlement_id: Option<Value>,
    #[serde(default, rename = "bucket_id")]
    bucket_id: Option<Value>,
    #[serde(default, rename = "show_name")]
    show_name: Option<Value>,
    #[serde(default)]
    capabilities: Option<Value>,
    #[serde(default)]
    meter: Option<Value>,
    #[serde(default, rename = "unit_type")]
    unit_type: Option<Value>,
    #[serde(default, rename = "total_units")]
    total_units: Option<Value>,
    #[serde(default, rename = "used_units")]
    used_units: Option<Value>,
    #[serde(default, rename = "remaining_units")]
    remaining_units: Option<Value>,
    #[serde(default)]
    period: Option<Value>,
    #[serde(default, rename = "period_end")]
    period_end: Option<Value>,
    #[serde(default, rename = "expires_at")]
    expires_at: Option<Value>,
    #[serde(flatten)]
    _extra: Value,
}

/// A display-bound upstream string: trimmed, non-empty, WITHOUT the
/// printable-ASCII gate — hostile text is the display gate's job, and a
/// legitimate non-ASCII display name must not be dropped.
fn display_text(raw: &Option<Value>) -> Option<String> {
    match raw {
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        _ => None,
    }
}

/// String-or-number id/text as a trimmed string; empty degrades to `None`
/// (the app drops empty strings too).
fn text_value(raw: &Option<Value>) -> Option<String> {
    match raw {
        Some(Value::String(text)) => normalize_source_value(Some(text)),
        Some(Value::Number(number)) => normalize_source_value(Some(&number.to_string())),
        _ => None,
    }
}

/// Upstream numbers arrive as JSON numbers or numeric strings (the app's
/// `parseNumber`); anything else degrades to `None`.
fn number_value(raw: &Option<Value>) -> Option<f64> {
    let text = match raw {
        Some(Value::Number(number)) => return number.as_f64().filter(|value| value.is_finite()),
        Some(Value::String(text)) => text.trim().to_string(),
        _ => return None,
    };
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// Upstream stamps are unix SECONDS (the app multiplies by 1000). Routed
/// through the shared `epoch_ms_to_rfc3339`, which promotes seconds and
/// rejects implausible magnitudes.
fn unix_seconds_to_rfc3339(raw: &Option<Value>) -> Option<String> {
    epoch_ms_to_rfc3339(raw.as_ref()?)
        .map(|reset| reset.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// The `model:`-prefixed capability list, trimmed per entry.
fn capability_models(raw: &Option<Value>) -> Vec<String> {
    raw.as_ref()
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(|entry| {
                    let stripped = entry
                        .strip_prefix("model:")
                        .unwrap_or(entry)
                        .trim()
                        .to_string();
                    stripped
                })
                .filter(|entry| !entry.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A display-safe bucket label: the first named model capability, else the
/// upstream `show_name`. Provider-derived free text, so it passes the shared
/// display gate; `None` when nothing visible remains.
fn balance_model_label(entry: &BalanceEntry) -> Option<String> {
    capability_models(&entry.capabilities)
        .into_iter()
        .next()
        .or_else(|| display_text(&entry.show_name))
        .and_then(|raw| sanitize_display_label(&raw))
}

/// A display-safe plan name (same gate, `None` when empty).
fn plan_display_name(entry: &PlanEntry) -> Option<String> {
    display_text(&entry.name).and_then(|raw| sanitize_display_label(&raw))
}

/// The app's own qualification: only the upstream `active` status counts.
/// No plan-name heuristics — a promotional package qualifies exactly when
/// upstream says it is active.
fn plan_status_is_active(status: Option<&str>) -> bool {
    status
        .map(str::trim)
        .is_some_and(|status| status.eq_ignore_ascii_case("active"))
}

/// The zcode.z.ai envelope answers business failures with HTTP 200 and a
/// non-zero `code` (0 = success) — the same pattern the reset-status
/// endpoint uses. The envelope `msg` is provider-controlled free text that
/// would be embedded in a user-visible message, so it passes the same
/// display-safety gate and degrades to an authored sentence when nothing
/// visible remains.
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
            format!("ZCode plan-balance endpoint rejected the request (code {code}): {detail}"),
        )
        .with_transient(code == 429 || (500..=599).contains(&code)),
    })
}

/// Groups balance rows under their active plans exactly as the app does:
/// a row belongs to the plan with its `user_plan_id`, falling back to
/// `plan_id` equality when no `user_plan_id` was reported. Rows that cannot
/// be attributed to an active plan are counted and dropped — never guessed
/// onto a plan, never merged across plans, never summed across units.
fn assemble_observation(payload: &PlanBalancePayload) -> (ZCodePlansObservation, PlanBalanceFacts) {
    let active_plans: Vec<(&PlanEntry, String)> = payload
        .plans
        .iter()
        .flatten()
        .filter_map(|entry| {
            let plan_id = text_value(&entry.plan_id)?;
            plan_status_is_active(entry.status.as_ref().and_then(Value::as_str))
                .then_some((entry, plan_id))
        })
        .collect();

    let mut plans: Vec<ZCodePlan> = active_plans
        .iter()
        .map(|(entry, plan_id)| ZCodePlan {
            plan_id: plan_id.clone(),
            user_plan_id: text_value(&entry.user_plan_id),
            name: plan_display_name(entry),
            status: entry
                .status
                .as_ref()
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("active")
                .to_string(),
            ends_at: unix_seconds_to_rfc3339(&entry.ends_at),
            balances: Vec::new(),
        })
        .collect();

    let mut mapped = 0usize;
    let mut orphans = 0usize;
    for entry in payload.balances.iter().flatten() {
        let row_plan_id = text_value(&entry.plan_id);
        let row_user_plan_id = text_value(&entry.user_plan_id);
        // The app's own per-row predicate: match by `user_plan_id` when
        // both the row and the plan carry one, else by `plan_id`.
        let index = plans.iter().position(|plan| {
            if row_user_plan_id.is_some() && plan.user_plan_id.is_some() {
                plan.user_plan_id.as_deref() == row_user_plan_id.as_deref()
            } else {
                row_plan_id.as_deref() == Some(plan.plan_id.as_str())
            }
        });
        let Some(index) = index else {
            orphans += 1;
            continue;
        };
        let plan_entry = active_plans[index].0;
        // The period rides the plan's entitlements upstream; resolve it once
        // here so the wire row is self-contained.
        let entitlement_period = plan_entry
            .entitlements
            .iter()
            .flatten()
            .find(|entitlement| {
                text_value(&entitlement.entitlement_id).is_some_and(|id| {
                    text_value(&entry.entitlement_id).as_deref() == Some(id.as_str())
                })
            })
            .and_then(|entitlement| text_value(&entitlement.period));
        let period = text_value(&entry.period).or(entitlement_period);
        plans[index].balances.push(ZCodePlanBalance {
            user_plan_id: row_user_plan_id,
            entitlement_id: text_value(&entry.entitlement_id),
            bucket_id: text_value(&entry.bucket_id),
            model: balance_model_label(entry),
            meter: text_value(&entry.meter),
            unit: text_value(&entry.unit_type),
            limit: number_value(&entry.total_units),
            used: number_value(&entry.used_units),
            remaining: number_value(&entry.remaining_units),
            period,
            period_end: unix_seconds_to_rfc3339(&entry.period_end),
            expires_at: unix_seconds_to_rfc3339(&entry.expires_at),
        });
        mapped += 1;
    }

    let facts = PlanBalanceFacts {
        active_plan_count: plans.len(),
        mapped_bucket_count: mapped,
        orphan_bucket_count: orphans,
        active_plan_names: plans.iter().filter_map(|plan| plan.name.clone()).collect(),
        // Deterministic first-seen order; units are never summed.
        unit_types: {
            let mut unit_types: Vec<String> = Vec::new();
            for balance in plans.iter().flat_map(|plan| plan.balances.iter()) {
                if let Some(unit) = &balance.unit {
                    if !unit_types.contains(unit) {
                        unit_types.push(unit.clone());
                    }
                }
            }
            unit_types
        },
    };
    (
        ZCodePlansObservation {
            plans,
            // Placeholder stamp; `retain_observation` (the freshness
            // authority) re-stamps with the observation clock.
            observed_at: Utc::now(),
        },
        facts,
    )
}

fn parse_plan_balances(
    body: &[u8],
) -> Result<(ZCodePlansObservation, PlanBalanceFacts), ProviderError> {
    let parsed: Value = serde_json::from_slice(body).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "ZCode plan-balance response was not usable JSON (schema change).",
        )
    })?;
    let Some(code) = parsed.get("code").and_then(Value::as_i64) else {
        return Err(ProviderError::new(
            "unexpected_response",
            "ZCode plan-balance response has no envelope code (schema change).",
        ));
    };
    if let Some(error) = envelope_error(code, &parsed) {
        return Err(error);
    }
    let Some(data) = parsed.get("data") else {
        return Err(ProviderError::new(
            "unexpected_response",
            "ZCode plan-balance response has no data payload (schema change).",
        ));
    };
    let payload: PlanBalancePayload = serde_json::from_value(data.clone()).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "ZCode plan-balance response format changed; plans/balances are not lists.",
        )
    })?;
    Ok(assemble_observation(&payload))
}

// ---------- HTTP ----------

/// One passive observation: read the session credential from the store, then
/// exactly one GET against the balance URL. No retry, no refresh: on an auth
/// refusal the caller sees the error and ZCode remains the only writer of
/// the credential store.
async fn fetch_plan_facts(
    client: &reqwest::Client,
    url: &str,
    credentials: &crate::zcode_reset::ZcodeSessionJwt,
) -> Result<(ZCodePlansObservation, PlanBalanceFacts), ProviderError> {
    let response = client
        .get(url)
        .headers(request_header_map(credentials)?)
        .send()
        .await
        .map_err(|error| {
            ProviderError::transient(
                "network",
                format!("Could not reach the ZCode plan-balance endpoint: {error}"),
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
            format!("ZCode plan-balance response was cut short: {error}"),
        )
    })?;
    if !status.is_success() {
        return Err(ProviderError::http_failure(
            status,
            format!("ZCode plan-balance endpoint returned HTTP {status}."),
        ));
    }
    parse_plan_balances(&body)
}

/// Injectable-core fetch: one GET against `url` with `credentials`. Tests
/// use it to exercise transport behavior hermetically (redirect refusal,
/// connection refusal) without touching the real store.
async fn fetch_plans_from(
    client: &reqwest::Client,
    url: &str,
    credentials: &crate::zcode_reset::ZcodeSessionJwt,
) -> Result<ZCodePlansObservation, ProviderError> {
    // Final-boundary value scrub (`secret_scrub`): the session JWT is alive
    // for this whole fetch, so any exact occurrence of it in a failure
    // message (e.g. an upstream envelope echoing it back) is removed before
    // the error leaves the adapter.
    fetch_plan_facts(client, url, credentials)
        .await
        .map(|(plans, _)| plans)
        .map_err(|error| {
            crate::secret_scrub::scrub_provider_error(error, &[credentials.0.as_str()])
        })
}

// ---------- retention (last-good for the observation) ----------

/// The last successful observation, retained so a transient observation
/// failure cannot erase fresh plan data while it is still within its
/// freshness budget. In-memory only, process-local, never persisted.
static LAST_OBSERVATION: OnceLock<Mutex<Option<ZCodePlansObservation>>> = OnceLock::new();

fn last_observation() -> &'static Mutex<Option<ZCodePlansObservation>> {
    LAST_OBSERVATION.get_or_init(|| Mutex::new(None))
}

/// True when an observation timestamp is still within its freshness budget:
/// not future-dated beyond clock-skew tolerance and not older than the TTL.
/// The same predicate the runtime's stale-trim applies to retained entries.
pub fn plans_fresh(observed_at: &DateTime<Utc>, now: DateTime<Utc>) -> bool {
    let age = now.signed_duration_since(*observed_at).num_seconds();
    (-OBSERVED_FUTURE_TOLERANCE_SECS..=OBSERVATION_TTL_SECS).contains(&age)
}

/// Retention core (pure, testable): a fresh observation replaces the cache
/// and passes through; a failure passes through unless a cached observation
/// is still within its freshness budget, which stands in as the last-good
/// result — a consumed bucket can thus never read as available, but a
/// transient observation failure also cannot erase data that was current
/// moments ago.
fn retain_observation(
    cache: &mut Option<ZCodePlansObservation>,
    result: Result<ZCodePlansObservation, ProviderError>,
    now: DateTime<Utc>,
) -> Result<ZCodePlansObservation, ProviderError> {
    match result {
        Ok(mut plans) => {
            plans.observed_at = now;
            *cache = Some(plans.clone());
            Ok(plans)
        }
        Err(error) => {
            let fresh = cache
                .as_ref()
                .is_some_and(|observation| plans_fresh(&observation.observed_at, now));
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
pub async fn fetch_plan_balances() -> Result<ZCodePlansObservation, ProviderError> {
    let result = match load_session_jwt() {
        Ok(credentials) => {
            fetch_plans_from(
                http_client()?,
                &balance_url(app_release_version().as_deref()),
                &credentials,
            )
            .await
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
    use crate::zai::MAX_UPSTREAM_DETAIL_CHARS;
    use crate::zcode_reset::ZcodeSessionJwt;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const TEST_JWT: &str = "jwt-value";

    fn test_credentials() -> ZcodeSessionJwt {
        ZcodeSessionJwt(TEST_JWT.to_string())
    }

    /// Live-verified shape: one active Trust Build package, one token bucket.
    fn trust_build_body() -> Value {
        serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": {
                "server_time": 1791379200_i64,
                "plans": [{
                    "plan_id": "plan-trust-build",
                    "user_plan_id": "up-1",
                    "name": "ZCode Trust Build",
                    "description": "Promotional package",
                    "priority": 10,
                    "status": "active",
                    "starts_at": 1789000000_i64,
                    "ends_at": 1793000000_i64,
                    "entitlements": [{
                        "entitlement_id": "ent-token",
                        "show_name": "GLM-5.3-Flash tokens",
                        "period": "one_time"
                    }]
                }],
                "balances": [{
                    "plan_id": "plan-trust-build",
                    "user_plan_id": "up-1",
                    "entitlement_id": "ent-token",
                    "bucket_id": "bucket-1",
                    "show_name": "GLM-5.3-Flash",
                    "capabilities": ["model:GLM-5.3-Flash"],
                    "meter": "token_usage",
                    "unit_type": "token",
                    "total_units": 100000000,
                    "used_units": 5200000,
                    "remaining_units": 94800000,
                    "available_units": 94800000,
                    "reserved_units": 0,
                    "period": null,
                    "period_start": null,
                    "period_end": null,
                    "expires_at": 1793000000_i64
                }]
            }
        })
    }

    // ---------- mutation boundary (source-level proof) ----------

    /// LimitScope observes only. The module source must not contain any
    /// mutating sibling, any request but GET, any upstream URL but the
    /// balance endpoint, or any logging of response bodies. The needle
    /// literals are assembled from fragments so this test cannot
    /// reintroduce the very strings it forbids.
    #[test]
    fn module_source_contains_no_mutating_endpoints_or_logging() {
        let source = include_str!("zcode_plans.rs");

        let forbidden = [
            format!("billing/{}", "claim"),
            format!("billing/{}", "preview"),
            format!("reset/{}", "use"),
            format!("reset/{}", "opportunity"),
        ];
        for path in forbidden {
            assert!(
                !source.contains(&path),
                "mutating endpoint path must be absent from the module: {path}"
            );
        }

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

        // Every absolute URL in the module must be the read-only balance
        // endpoint (the constant and the origin constant it is built from).
        let scheme_needle = format!("{}://", "https");
        for (offset, _) in source.match_indices(&scheme_needle) {
            assert!(
                source[offset..].starts_with(PLAN_BALANCE_URL)
                    || source[offset..].starts_with(ZCODE_ENDPOINT_ORIGIN),
                "unexpected upstream URL at byte {offset}: the balance endpoint is the only one allowed"
            );
        }

        // Raw response bodies are never logged: no log macros exist (the
        // needle literals are assembled from fragments for the same reason).
        assert!(!source.contains(&format!("{}println!", "e")));
        assert!(!source.contains(&format!("{}::", "log")));
        assert!(!source.contains(&format!("{}::", "tracing")));
    }

    // ---------- parser matrix ----------

    // 1. one active Trust Build plan (live shape)
    #[test]
    fn trust_build_shape_parses_plan_and_absolute_bucket() {
        let (observation, facts) =
            parse_plan_balances(trust_build_body().to_string().as_bytes()).unwrap();
        assert_eq!(observation.plans.len(), 1);
        let plan = &observation.plans[0];
        assert_eq!(plan.plan_id, "plan-trust-build");
        assert_eq!(plan.user_plan_id.as_deref(), Some("up-1"));
        assert_eq!(plan.name.as_deref(), Some("ZCode Trust Build"));
        assert_eq!(plan.status, "active");
        // 1793000000 s == 2026-10-26T07:33:20Z
        assert_eq!(plan.ends_at.as_deref(), Some("2026-10-26T07:33:20Z"));
        assert_eq!(plan.balances.len(), 1);
        let balance = &plan.balances[0];
        // Absolute values are preserved, never forced into percentages.
        assert_eq!(balance.limit, Some(100_000_000.0));
        assert_eq!(balance.used, Some(5_200_000.0));
        assert_eq!(balance.remaining, Some(94_800_000.0));
        assert_eq!(balance.model.as_deref(), Some("GLM-5.3-Flash"));
        assert_eq!(balance.unit.as_deref(), Some("token"));
        assert_eq!(balance.meter.as_deref(), Some("token_usage"));
        // The entitlement period resolves onto the balance row.
        assert_eq!(balance.period.as_deref(), Some("one_time"));
        assert_eq!(balance.expires_at.as_deref(), Some("2026-10-26T07:33:20Z"));
        assert_eq!(balance.entitlement_id.as_deref(), Some("ent-token"));
        assert_eq!(balance.bucket_id.as_deref(), Some("bucket-1"));
        assert_eq!(facts.active_plan_count, 1);
        assert_eq!(facts.mapped_bucket_count, 1);
        assert_eq!(facts.orphan_bucket_count, 0);
        assert_eq!(facts.active_plan_names, vec!["ZCode Trust Build"]);
        assert_eq!(facts.unit_types, vec!["token".to_string()]);
    }

    // 2. multiple active plans
    #[test]
    fn multiple_active_plans_stay_separate() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "plans": [
                    { "plan_id": "plan-a", "user_plan_id": "up-a", "name": "Plan A", "status": "active" },
                    { "plan_id": "plan-b", "user_plan_id": "up-b", "name": "Plan B", "status": "active" }
                ],
                "balances": [
                    { "plan_id": "plan-a", "user_plan_id": "up-a", "unit_type": "token",
                      "total_units": 100, "used_units": 10, "remaining_units": 90 },
                    { "plan_id": "plan-b", "user_plan_id": "up-b", "unit_type": "token",
                      "total_units": 200, "used_units": 20, "remaining_units": 180 }
                ]
            }
        })
        .to_string();
        let (observation, facts) = parse_plan_balances(body.as_bytes()).unwrap();
        assert_eq!(observation.plans.len(), 2);
        assert_eq!(observation.plans[0].plan_id, "plan-a");
        assert_eq!(observation.plans[1].plan_id, "plan-b");
        assert_eq!(observation.plans[0].balances.len(), 1);
        assert_eq!(observation.plans[1].balances.len(), 1);
        // Separate pools are never summed across plans.
        assert_eq!(observation.plans[0].balances[0].remaining, Some(90.0));
        assert_eq!(observation.plans[1].balances[0].remaining, Some(180.0));
        assert_eq!(facts.active_plan_count, 2);
        assert_eq!(facts.mapped_bucket_count, 2);
    }

    // 3. balances map by user_plan_id first, plan_id second
    #[test]
    fn balances_map_by_user_plan_id_then_plan_id() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "plans": [
                    { "plan_id": "shared-plan", "user_plan_id": "up-1", "name": "One", "status": "active" },
                    { "plan_id": "shared-plan", "user_plan_id": "up-2", "name": "Two", "status": "active" }
                ],
                "balances": [
                    { "plan_id": "shared-plan", "user_plan_id": "up-2", "unit_type": "credit",
                      "total_units": 5, "used_units": 1, "remaining_units": 4 },
                    { "plan_id": "shared-plan", "unit_type": "credit",
                      "total_units": 9, "used_units": 0, "remaining_units": 9 }
                ]
            }
        })
        .to_string();
        let (observation, facts) = parse_plan_balances(body.as_bytes()).unwrap();
        // The user_plan_id rows went to their exact plans; the id-less row
        // fell back to the first plan matching the bare plan_id.
        assert_eq!(observation.plans[0].balances.len(), 1);
        assert_eq!(observation.plans[0].balances[0].remaining, Some(9.0));
        assert_eq!(observation.plans[1].balances.len(), 1);
        assert_eq!(observation.plans[1].balances[0].remaining, Some(4.0));
        assert_eq!(facts.mapped_bucket_count, 2);
        assert_eq!(facts.orphan_bucket_count, 0);
    }

    // 4. expired plans are excluded from active display
    #[test]
    fn expired_and_other_statuses_are_excluded() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "plans": [
                    { "plan_id": "plan-expired", "name": "Old", "status": "expired" },
                    { "plan_id": "plan-pending", "name": "Soon", "status": "pending" },
                    { "plan_id": "plan-active", "name": "Live", "status": "Active " },
                    { "plan_id": "plan-missing-status", "name": "Mystery" }
                ],
                "balances": [
                    { "plan_id": "plan-expired", "unit_type": "token", "total_units": 1 },
                    { "plan_id": "plan-active", "unit_type": "token", "total_units": 2 },
                    { "plan_id": "plan-unknown", "unit_type": "token", "total_units": 3 }
                ]
            }
        })
        .to_string();
        let (observation, facts) = parse_plan_balances(body.as_bytes()).unwrap();
        // Only the active plan is carried (case-insensitive, trimmed);
        // a missing status is NOT active.
        assert_eq!(observation.plans.len(), 1);
        assert_eq!(observation.plans[0].plan_id, "plan-active");
        assert_eq!(observation.plans[0].status, "Active");
        assert_eq!(observation.plans[0].balances.len(), 1);
        // Buckets of excluded plans are orphans, never displayed.
        assert_eq!(facts.orphan_bucket_count, 2);
        assert_eq!(facts.mapped_bucket_count, 1);
    }

    // 5. unknown/missing optional fields degrade defensively
    #[test]
    fn missing_optional_fields_degrade_without_dropping_the_plan() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "plans": [{ "plan_id": "plan-bare", "status": "active" }],
                "balances": [{
                    "plan_id": "plan-bare",
                    "capabilities": [],
                    "total_units": "not-a-number",
                    "used_units": null,
                    "expires_at": "whenever"
                }]
            }
        })
        .to_string();
        let (observation, _) = parse_plan_balances(body.as_bytes()).unwrap();
        let plan = &observation.plans[0];
        assert_eq!(plan.plan_id, "plan-bare");
        assert_eq!(plan.name, None);
        assert_eq!(plan.ends_at, None);
        assert_eq!(plan.balances.len(), 1);
        let balance = &plan.balances[0];
        assert_eq!(balance.model, None);
        assert_eq!(balance.limit, None);
        assert_eq!(balance.used, None);
        assert_eq!(balance.remaining, None);
        assert_eq!(balance.unit, None);
        assert_eq!(balance.period, None);
        assert_eq!(balance.expires_at, None);
    }

    // 6. token vs credit units stay distinct
    #[test]
    fn token_and_credit_buckets_never_merge() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "plans": [{ "plan_id": "plan-mixed", "name": "Mixed", "status": "active" }],
                "balances": [
                    { "plan_id": "plan-mixed", "unit_type": "token",
                      "total_units": 100000000, "used_units": 5200000, "remaining_units": 94800000 },
                    { "plan_id": "plan-mixed", "unit_type": "credit",
                      "total_units": 6, "used_units": 1, "remaining_units": 5 }
                ]
            }
        })
        .to_string();
        let (observation, facts) = parse_plan_balances(body.as_bytes()).unwrap();
        let balances = &observation.plans[0].balances;
        assert_eq!(balances.len(), 2);
        assert_eq!(balances[0].unit.as_deref(), Some("token"));
        assert_eq!(balances[1].unit.as_deref(), Some("credit"));
        // Both rows keep their own absolute values; no summed row exists.
        assert_eq!(balances[0].remaining, Some(94_800_000.0));
        assert_eq!(balances[1].remaining, Some(5.0));
        assert_eq!(balances[0].bucket_id, None);
        assert_eq!(facts.unit_types.len(), 2);
    }

    // 7. one_time vs daily period
    #[test]
    fn one_time_and_daily_periods_resolve_from_upstream() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "plans": [{
                    "plan_id": "plan-periods", "name": "Periods", "status": "active",
                    "entitlements": [
                        { "entitlement_id": "ent-once", "period": "one_time" },
                        { "entitlement_id": "ent-day", "period": "daily" }
                    ]
                }],
                "balances": [
                    { "plan_id": "plan-periods", "entitlement_id": "ent-once",
                      "unit_type": "token", "total_units": 100, "remaining_units": 100,
                      "expires_at": 1793000000_i64 },
                    { "plan_id": "plan-periods", "entitlement_id": "ent-day",
                      "unit_type": "token", "total_units": 10, "remaining_units": 9,
                      "period": "daily", "period_end": 1791400000_i64 }
                ]
            }
        })
        .to_string();
        let (observation, _) = parse_plan_balances(body.as_bytes()).unwrap();
        let balances = &observation.plans[0].balances;
        // one_time: the period comes from the plan entitlement, the expiry
        // from the bucket.
        assert_eq!(balances[0].period.as_deref(), Some("one_time"));
        assert_eq!(
            balances[0].expires_at.as_deref(),
            Some("2026-10-26T07:33:20Z")
        );
        assert_eq!(balances[0].period_end, None);
        // daily: the bucket's own period wins, the period end is the reset.
        assert_eq!(balances[1].period.as_deref(), Some("daily"));
        // 1791400000 s == 2026-10-07T19:06:40Z
        assert_eq!(
            balances[1].period_end.as_deref(),
            Some("2026-10-07T19:06:40Z")
        );
    }

    // 8. malformed response envelope
    #[test]
    fn malformed_envelopes_are_schema_changes_not_panics() {
        let html = b"<html><body>Please sign in</body></html>";
        assert_eq!(
            parse_plan_balances(html).unwrap_err().code,
            "unexpected_response"
        );
        assert_eq!(
            parse_plan_balances(serde_json::json!({ "data": {} }).to_string().as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
        assert_eq!(
            parse_plan_balances(serde_json::json!({ "code": 0 }).to_string().as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
        assert_eq!(
            parse_plan_balances(
                serde_json::json!({ "code": 0, "data": { "plans": "nope" } })
                    .to_string()
                    .as_bytes()
            )
            .unwrap_err()
            .code,
            "unexpected_response"
        );
        assert_eq!(
            parse_plan_balances(
                serde_json::json!({ "code": 0, "data": { "balances": [1, 2] } })
                    .to_string()
                    .as_bytes()
            )
            .unwrap_err()
            .code,
            "unexpected_response"
        );
    }

    #[test]
    fn empty_plans_yield_a_valid_empty_observation() {
        let body = serde_json::json!({
            "code": 0,
            "data": { "plans": [], "balances": [] }
        })
        .to_string();
        let (observation, facts) = parse_plan_balances(body.as_bytes()).unwrap();
        assert!(observation.plans.is_empty());
        assert_eq!(facts.active_plan_count, 0);
        assert_eq!(facts.mapped_bucket_count, 0);
    }

    #[test]
    fn unknown_upstream_fields_are_ignored() {
        let body = serde_json::json!({
            "code": 0,
            "msg": "ok",
            "success": true,
            "data": {
                "server_time": 1791379200_i64,
                "plans": [{
                    "plan_id": "plan-x", "name": "X", "status": "active",
                    "priority": 99, "some_future_field": { "nested": [1] }
                }],
                "balances": [{
                    "plan_id": "plan-x", "unit_type": "token",
                    "total_units": 100, "remaining_units": 100,
                    "reserved_units": 0, "available_units": 100
                }],
                "trace_id": "whatever"
            }
        })
        .to_string();
        let (observation, _) = parse_plan_balances(body.as_bytes()).unwrap();
        assert_eq!(observation.plans.len(), 1);
        assert_eq!(observation.plans[0].balances[0].limit, Some(100.0));
    }

    // ---------- envelope / HTTP classification ----------

    #[test]
    fn envelope_zero_is_success_and_business_codes_map_to_credential_errors() {
        assert!(envelope_error(0, &serde_json::json!({})).is_none());
        assert_eq!(
            parse_plan_balances(
                serde_json::json!({
                    "code": 401, "msg": "token expired or incorrect"
                })
                .to_string()
                .as_bytes()
            )
            .unwrap_err()
            .code,
            "auth_invalid"
        );
        assert_eq!(
            parse_plan_balances(
                serde_json::json!({
                    "code": 403, "msg": "forbidden"
                })
                .to_string()
                .as_bytes()
            )
            .unwrap_err()
            .code,
            "not_entitled"
        );
    }

    #[test]
    fn request_shape_errors_classify_as_deterministic() {
        // The discovery-time failure mode: HTTP 400 + code 3001 (parameter
        // error) is a request-shape refusal — surfaced, not retried.
        let error = parse_plan_balances(
            serde_json::json!({ "code": 3001, "msg": "parameter error" })
                .to_string()
                .as_bytes(),
        )
        .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.transient, Some(false));
        assert!(error.message.contains("3001"));
        assert!(error.message.contains("parameter error"));

        // Server-side refusals stay transient.
        assert_eq!(
            parse_plan_balances(
                serde_json::json!({ "code": 429, "msg": "slow down" })
                    .to_string()
                    .as_bytes()
            )
            .unwrap_err()
            .transient,
            Some(true)
        );
    }

    /// The envelope `msg` is provider-controlled free text that reaches a
    /// user-visible message, so it passes the same display-safety gate the
    /// Z.ai monitor errors use.
    #[test]
    fn envelope_upstream_message_is_sanitized_before_display() {
        let hostile = "\u{1b}[31mEVIL\u{1b}[0m  injected\u{0}text";
        let error = parse_plan_balances(
            serde_json::json!({ "code": 400, "msg": hostile })
                .to_string()
                .as_bytes(),
        )
        .unwrap_err();
        assert!(!error.message.contains('\u{1b}'));
        assert!(!error.message.chars().any(char::is_control));

        let long = "x".repeat(MAX_UPSTREAM_DETAIL_CHARS + 50);
        let error = parse_plan_balances(
            serde_json::json!({ "code": 400, "msg": long })
                .to_string()
                .as_bytes(),
        )
        .unwrap_err();
        let detail_len = error.message.split(": ").last().unwrap().chars().count();
        assert_eq!(detail_len, MAX_UPSTREAM_DETAIL_CHARS);

        let absent = parse_plan_balances(br#"{"code":400}"#).unwrap_err();
        assert!(absent.message.ends_with(": request failed"));
    }

    // ---------- display gates ----------

    #[test]
    fn plan_names_and_bucket_labels_pass_the_display_gate() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "plans": [{
                    "plan_id": "plan-hostile",
                    "name": "\u{1b}[31mEVIL\u{1b}[0m plan\u{0}name",
                    "status": "active"
                }],
                "balances": [{
                    "plan_id": "plan-hostile",
                    "show_name": "\u{1b}[0m  hostile   label",
                    "unit_type": "token",
                    "total_units": 1
                }]
            }
        })
        .to_string();
        let (observation, facts) = parse_plan_balances(body.as_bytes()).unwrap();
        let plan = &observation.plans[0];
        assert_eq!(plan.name.as_deref(), Some("EVIL planname"));
        assert_eq!(plan.balances[0].model.as_deref(), Some("hostile label"));
        assert_eq!(facts.active_plan_names, vec!["EVIL planname".to_string()]);
    }

    #[test]
    fn release_channel_defaults_to_stable_without_an_env_value() {
        assert_eq!(release_channel_from(Some("production")), "production");
        assert_eq!(release_channel_from(Some("  preview  ")), "preview");
        assert_eq!(release_channel_from(Some("   ")), "stable");
        assert_eq!(release_channel_from(None), "stable");
    }

    #[test]
    fn normalizer_source_value_rejects_non_printable_and_blank() {
        assert_eq!(normalize_source_value(Some("  ok ")).as_deref(), Some("ok"));
        assert_eq!(normalize_source_value(Some("a\u{0}b")), None);
        assert_eq!(normalize_source_value(Some("a\u{e9}b")), None);
        assert_eq!(normalize_source_value(Some("   ")), None);
        assert_eq!(normalize_source_value(None), None);
    }

    // ---------- request shape (app-faithful headers) ----------

    #[test]
    fn request_headers_match_the_apps_header_family() {
        let headers = request_headers(&test_credentials()).unwrap();
        let get = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        // The credential rides as the app sends it (Bearer-prefixed).
        assert_eq!(get("authorization").as_deref(), Some("Bearer jwt-value"));
        // Static source headers mirror buildZCodeSourceHeadersFromContext.
        assert_eq!(
            get("user-agent").is_some_and(|value| value.starts_with("ZCode/")),
            true
        );
        assert_eq!(get("http-referer").as_deref(), Some("https://zcode.z.ai"));
        assert_eq!(get("x-title").as_deref(), Some("Z Code@electron"));
        assert_eq!(
            get("x-platform"),
            Some(format!("{}-{}", platform_value(), arch_value()))
        );
        assert_eq!(
            get("x-os-category"),
            Some(os_category(platform_value()).to_string())
        );
        // The channel is derived (ZCODE_ENV when set, else "stable") — the
        // assertion is self-consistent with the ambient environment.
        assert_eq!(
            get("x-release-channel").as_deref(),
            Some(release_channel().as_str())
        );
        assert!(!release_channel().is_empty());
        assert_eq!(get("x-client-language").is_some(), true);
        assert_eq!(get("x-client-timezone").is_some(), true);
        assert_eq!(get("accept").as_deref(), Some("application/json"));
        // Request-scoped identity: present, unique per request.
        let first = get("x-request-id").expect("request id present");
        let second = request_headers(&test_credentials()).unwrap();
        let second_id = second
            .iter()
            .find(|(key, _)| key == "x-request-id")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert_ne!(first, second_id, "request ids are generated per request");
    }

    #[test]
    fn header_injection_context_omits_underivable_headers() {
        let empty = SourceHeaderContext {
            app_version: None,
            os_version: None,
            device_mid: None,
        };
        let headers = build_source_headers(&empty);
        let names: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
        assert!(!names.contains(&"x-zcode-app-version"));
        assert!(!names.contains(&"x-os-version"));
        assert!(!names.contains(&"x-device-mid"));
        // The degraded user agent mirrors the app's own unknown-version form.
        assert!(headers
            .iter()
            .any(|(name, value)| name == "user-agent" && value == "ZCode/unknown"));

        // The URL degrades the same way: no fabricated query parameter.
        assert_eq!(balance_url(None), PLAN_BALANCE_URL);
        assert_eq!(
            balance_url(Some("3.14.4")),
            format!("{PLAN_BALANCE_URL}?app_version=3.14.4")
        );
    }

    #[test]
    fn wire_keeps_identifiers_but_never_credential_material() {
        let (observation, _) =
            parse_plan_balances(trust_build_body().to_string().as_bytes()).unwrap();
        let wire = serde_json::to_string(&observation).unwrap();
        // Stable identifiers survive the wire (for diagnostics), camelCased.
        assert!(wire.contains("planId"), "wire: {wire}");
        assert!(wire.contains("userPlanId"), "wire: {wire}");
        assert!(wire.contains("entitlementId"), "wire: {wire}");
        assert!(wire.contains("bucketId"), "wire: {wire}");
        // Absolute balances ride the wire; the internal stamp does not.
        assert!(wire.contains("94800000.0"), "wire: {wire}");
        assert!(
            !wire.contains("observed_at"),
            "internal stamp stays off the wire: {wire}"
        );
        // The session credential never appears. (The credential newtype has
        // no Serialize impl at all — a compile-time, not runtime, guarantee.)
        assert!(!wire.contains(TEST_JWT), "wire: {wire}");
    }

    // ---------- transport behavior (hermetic, local sockets) ----------

    /// The shared client refuses redirects: a 302 from the balance endpoint
    /// is surfaced as-is, never followed, so the session credential cannot
    /// be relayed elsewhere.
    #[tokio::test]
    async fn plan_balance_redirect_is_returned_without_following() {
        let destination = TcpListener::bind(("127.0.0.1", 0)).unwrap();
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
        let response = fetch_plans_from(
            http_client().unwrap(),
            &format!("http://{source_addr}/balance"),
            &test_credentials(),
        )
        .await
        .unwrap_err();
        assert_eq!(response.code, "unexpected_response");
        assert_eq!(response.http_status, Some(302));
        let request = source_thread.join().unwrap();
        // Always-present request family: the credential, the platform
        // identity, and the per-request id. Device/version headers are
        // machine-derived and asserted in the pure assembly tests.
        assert!(request.contains("authorization: bearer jwt-value"));
        assert!(request.contains("x-platform:"));
        assert!(request.contains("x-request-id:"));
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
        let error = fetch_plans_from(
            http_client().unwrap(),
            "http://127.0.0.1:9/balance",
            &test_credentials(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "network");
        assert_eq!(error.transient, Some(true));
    }

    /// End-to-end leak simulation: an upstream envelope whose provider-
    /// controlled `msg` echoes the in-flight session credential back inside
    /// its error detail. The surfaced failure must no longer contain the
    /// credential value, while the benign carrier text survives.
    #[tokio::test]
    async fn envelope_echoing_the_session_credential_is_scrubbed() {
        const ECHOED_JWT: &str = "echo-jwt-value-0123456789abcdef";
        let credentials = ZcodeSessionJwt(ECHOED_JWT.to_string());
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let source_addr = source.local_addr().unwrap();
        let body = format!(
            "{{\"code\":1,\"msg\":\"request refused for bearer {ECHOED_JWT} (see logs)\"}}"
        );
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
        let error = fetch_plans_from(
            http_client().unwrap(),
            &format!("http://{source_addr}/balance"),
            &credentials,
        )
        .await
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code, "unexpected_response");
        assert!(!error.message.contains(ECHOED_JWT), "{}", error.message);
        assert!(
            error
                .message
                .contains("request refused for bearer [REDACTED] (see logs)"),
            "{}",
            error.message
        );
    }

    // ---------- retention (last-good for the observation) ----------

    fn observed_plans(count: usize, observed_at: DateTime<Utc>) -> ZCodePlansObservation {
        ZCodePlansObservation {
            plans: (0..count)
                .map(|index| ZCodePlan {
                    plan_id: format!("plan-{index}"),
                    user_plan_id: None,
                    name: Some("Plan".to_string()),
                    status: "active".to_string(),
                    ends_at: None,
                    balances: vec![ZCodePlanBalance {
                        user_plan_id: None,
                        entitlement_id: None,
                        bucket_id: None,
                        model: Some("GLM-5.3-Flash".to_string()),
                        meter: None,
                        unit: Some("token".to_string()),
                        limit: Some(100.0),
                        used: None,
                        remaining: Some(94.8),
                        period: Some("one_time".to_string()),
                        period_end: None,
                        expires_at: None,
                    }],
                })
                .collect(),
            observed_at,
        }
    }

    #[test]
    fn fresh_observation_replaces_the_cache_and_is_stamped() {
        let mut cache = None;
        let now = Utc::now();
        let observed = observed_plans(1, now - chrono::Duration::minutes(5));
        let result = retain_observation(&mut cache, Ok(observed), now).unwrap();
        // The freshness authority re-stamps with the observation clock.
        assert_eq!(result.observed_at, now);
        assert_eq!(cache.as_ref().unwrap().observed_at, now);
        assert_eq!(result.plans.len(), 1);
    }

    #[test]
    fn failure_within_the_ttl_serves_the_retained_observation() {
        let observed_at = Utc::now() - chrono::Duration::minutes(10);
        let mut cache = Some(observed_plans(1, observed_at));
        let failure = Err(ProviderError::transient("network", "offline"));
        let served = retain_observation(&mut cache, failure, Utc::now()).unwrap();
        // The retained observation stands in, with its ORIGINAL stamp.
        assert_eq!(served.observed_at, observed_at);
        assert!(cache.is_some());
    }

    #[test]
    fn failure_after_the_ttl_propagates_and_drops_the_cache() {
        let observed_at = Utc::now() - chrono::Duration::seconds(OBSERVATION_TTL_SECS + 60);
        let mut cache = Some(observed_plans(1, observed_at));
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
    fn freshness_budget_mirrors_the_reset_card_convention() {
        let now = Utc::now();
        assert!(plans_fresh(&(now - chrono::Duration::seconds(1)), now));
        assert!(plans_fresh(
            &(now - chrono::Duration::seconds(OBSERVATION_TTL_SECS)),
            now
        ));
        assert!(!plans_fresh(
            &(now - chrono::Duration::seconds(OBSERVATION_TTL_SECS + 1)),
            now
        ));
        assert!(!plans_fresh(
            &(now + chrono::Duration::seconds(OBSERVED_FUTURE_TOLERANCE_SECS + 1)),
            now
        ));
        assert!(plans_fresh(
            &(now + chrono::Duration::seconds(OBSERVED_FUTURE_TOLERANCE_SECS)),
            now
        ));
    }

    /// Live proof for the discovery run: exactly ONE GET against the balance
    /// endpoint. Run manually under the credential-hash protocol (hash
    /// before, run, hash after) — never in CI, never in a loop.
    #[tokio::test]
    #[ignore = "live proof: exactly one GET /billing/balance; run manually with the credential hash protocol"]
    async fn live_plan_balance_observation() {
        let client = http_client().expect("http client");
        let credentials = load_session_jwt().expect("live session jwt");
        let url = balance_url(app_release_version().as_deref());
        let (observation, facts) = fetch_plan_facts(client, &url, &credentials)
            .await
            .expect("live plan-balance observation");
        // Safe metadata only — no tokens, no identifiers, no raw body.
        println!("billing/balance observation:");
        println!("  http class: 2xx (success)");
        println!("  active plans: {}", facts.active_plan_count);
        for name in &facts.active_plan_names {
            println!("  plan: {name}");
        }
        println!("  mapped buckets: {}", facts.mapped_bucket_count);
        println!("  orphan buckets: {}", facts.orphan_bucket_count);
        println!("  unit types: {}", facts.unit_types.join(", "));
        assert_eq!(observation.plans.len(), facts.active_plan_count);
        let mapped: usize = observation
            .plans
            .iter()
            .map(|plan| plan.balances.len())
            .sum();
        assert_eq!(mapped, facts.mapped_bucket_count);
    }
}
