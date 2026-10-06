//! Grok Bot (X Premium+) weekly usage — passive supplemental observation.
//!
//! Observes the Grok Bot desktop client's weekly usage via the Cursor-served
//! ConnectRPC endpoint
//! `POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus`
//! (request shape verified 2026-10-06 from the installed Grok Bot app; the
//! checksum algorithm was verified from the installed bundle and live-proven
//! the same day, and is reproduced against the live endpoint through this
//! exact code path).
//!
//! This is a SIBLING of `grok` (the xAI billing client), not a replacement or
//! an extension of it: the weekly-credit and on-demand windows stay sourced
//! from the xAI plane with their own credential, history, and notification
//! identity. This module only adds the X Premium+ weekly usage the Grok Bot
//! client serves through Cursor's sand plane — a deliberately separate pool
//! that is never summed, merged, or compared with the xAI figures.
//!
//! Passive credential contract — Cursor (and the Grok Bot app) own login,
//! refresh, rotation, and every credential-store write; this module only ever
//! reads:
//! - reads the Cursor access token from
//!   `%APPDATA%\Cursor\User\globalStorage\state.vscdb` (key
//!   `cursorAuth/accessToken`) through a temporary file snapshot — the source
//!   database is never opened, never written, never checkpointed;
//! - the snapshot copies the database and its `-wal` sidecar (the `-shm` is a
//!   transient index that the copy rebuilds itself), so a live WAL is
//!   replayed and the freshest committed auth state is seen;
//! - the token's JWT `exp` claim is verified locally; a token that is past
//!   its expiry — or whose expiry cannot be verified — fails closed with no
//!   network traffic. No refresh, no retry loop: exactly one POST per
//!   invocation.
//!
//! The request reproduces the installed client's header family, with every
//! dynamic value derived locally and nothing persisted:
//! - `x-cursor-checksum` — the verified obfuscation chain over a 6-byte
//!   big-endian `floor(now_ms / 1_000_000)` bucket (initial key byte 165,
//!   XOR-then-add-index, key chained to the previous output), base64url
//!   without padding, suffixed with the machine id. LimitScope generates its
//!   own process-stable UUID instead of reading Grok Bot's persisted machine
//!   id (live-verified acceptance); it is written nowhere.
//! - `x-cursor-client-version` / `User-Agent: Grok Bot/<version>` — the
//!   installed app version, read from the Windows uninstall registry. When
//!   the version cannot be derived the observation fails closed rather than
//!   fabricating one.
//! - `x-request-id` — generated fresh per request (the app does the same).
//!
//! The module's ONLY upstream URL is the read-only usage POST. Raw response
//! bodies are never logged: body bytes flow only into the parser, and errors
//! carry at most a sanitized status or an authored sentence. Credential
//! material never crosses to the wire: the token is a newtype with a
//! redacted `Debug` and a failing `Serialize` (compile-time guard), and every
//! failure message passes the final-boundary value scrub.
//!
//! Wire exposure rides the Grok provider's snapshot entry
//! (`ProviderUsageDto::grok_bot`), refreshed by the ordinary runtime cycle —
//! no second scheduler, no tauri command of its own. A failed observation
//! never fails the Grok/xAI quota refresh, and a fresh observation is
//! retained (within the TTL below) when a later observation fails, mirroring
//! the ZCode plan-observation convention.
//!
//! Semantics note: the weekly percentage, reset, and plan label are parsed
//! exactly as the endpoint reports them. `usesPooledEnterpriseAllowance` is a
//! fail-closed configuration: pooled enterprise usage is not representable in
//! this supplemental shape, so the observation reports nothing rather than a
//! wrong figure. On-demand spend metadata (the separate current-period
//! endpoint) is deliberately out of scope for v1.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::provider_error::ProviderError;
use crate::zai::{epoch_ms_to_rfc3339, sanitize_display_label};

const SAND_USAGE_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Freshness budget for a retained Grok Bot observation. Mirrors
/// `zcode_reset::OBSERVATION_TTL_SECS` (and the Grok provider's own 15-minute
/// polling cadence): the endpoint is re-queried live every Grok poll, and an
/// observation older than roughly one poll cycle must stop reading as
/// current.
pub const OBSERVATION_TTL_SECS: i64 = 15 * 60;
/// Clock-skew tolerance for observation timestamps (5 minutes), matching the
/// sibling supplemental observations and the persisted last-good store.
const OBSERVED_FUTURE_TOLERANCE_SECS: i64 = 300;

/// The ItemTable key Cursor stores its (JWT) access token under.
const TOKEN_KEY: &str = "cursorAuth/accessToken";
/// A token within this margin of its locally decoded expiry is treated as
/// expired without any network traffic (mirrors `grok.rs`).
const EXPIRY_GUARD_SECS: i64 = 60;

/// The verified checksum seed: a 6-byte big-endian encoding of
/// `floor(now_ms / 1_000_000)` — the same ~16.7-minute bucket the installed
/// client uses.
const CHECKSUM_BUCKET_MS: i64 = 1_000_000;
/// The verified initial obfuscation key byte.
const CHECKSUM_INITIAL_KEY: u8 = 165;

// ---------- data returned to the WebView (camelCase on the wire) ----------

/// The Grok Bot weekly usage observation. Only endpoint fields that were
/// evidence-backed at discovery time and are required for the supplemental
/// presentation are carried; absolute weekly limits and remaining amounts are
/// deliberately absent (the endpoint does not report them in this shape).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokBotUsage {
    /// The Grok plan label as the endpoint reports it (e.g. `X Premium+`),
    /// display-sanitized. `None` when the upstream label is missing — the
    /// frontend renders the block without the plan suffix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    /// The stable plan id the endpoint reports (e.g. `x-premium-plus`).
    /// Diagnostics-grade; never rendered as the display label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    /// The underlying Cursor plan name the endpoint reports (e.g. `Free`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_plan_name: Option<String>,
    /// The weekly usage percentage, exactly as reported (clamped to 0..=100
    /// only for display safety). Never derived from anything else, never
    /// summed with the xAI weekly-credit pool.
    pub used_percent: f64,
    /// RFC 3339 start of the current weekly period, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_start: Option<String>,
    /// RFC 3339 UTC reset of the current weekly period, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
    /// The endpoint's own availability verdict, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_available_usage: Option<bool>,
    /// The endpoint's own on-demand `enabled` flag, when explicitly reported.
    /// Absent stays absent — never assumed true (a missing flag must not
    /// make on-demand spend read as enabled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_demand_enabled: Option<bool>,
    /// Internal observation stamp (never serialized): when this observation
    /// was actually observed upstream. Same contract as the sibling
    /// supplemental observations; the retention TTL and the runtime's
    /// stale-trim compare against it.
    #[serde(skip)]
    pub observed_at: DateTime<Utc>,
}

/// The safe facts a live proof records: presence booleans and the clamped
/// percentage only — no credential material, no account identifiers, no raw
/// response bodies. The production fetch carries them for the sibling
/// observers' parity; only the tests and the live proof read them.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SandUsageFacts {
    pub plan_label_present: bool,
    pub plan_id_present: bool,
    pub usage_percent: Option<f64>,
    pub reset_present: bool,
    pub period_start_present: bool,
    pub has_available_usage: Option<bool>,
    pub on_demand_enabled: Option<bool>,
}

// ---------- credentials (passive, backend-only) ----------

/// The Cursor JWT access token (`cursorAuth/accessToken` in Cursor's local
/// state). `Debug` is redacted through the member impl, and the manual
/// `Serialize` impl below always fails so no generic serde path can ever
/// carry credential material toward the wire.
pub(crate) struct CursorAccessToken(pub(crate) String);

impl std::fmt::Debug for CursorAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("CursorAccessToken").field(&"<redacted>").finish()
    }
}

impl Serialize for CursorAccessToken {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(<S::Error as serde::ser::Error>::custom(
            "Cursor credential material never crosses the wire",
        ))
    }
}

fn cursor_not_installed() -> ProviderError {
    ProviderError::new(
        "cursor_not_installed",
        "No Cursor local state with a signed-in session was found; the Grok Bot observation needs a signed-in Cursor install.",
    )
}

fn credential_missing() -> ProviderError {
    ProviderError::new(
        "credential_missing",
        "The Cursor local state holds no usable access token. Sign in to Cursor first.",
    )
}

fn credential_expired(reason: &str) -> ProviderError {
    ProviderError::new(
        "credential_expired",
        format!(
            "The cached Cursor access token {reason}. Re-authenticate in Cursor; LimitScope never refreshes Cursor auth."
        ),
    )
}

fn state_unreadable(reason: String) -> ProviderError {
    ProviderError::new("auth_unreadable", format!("Could not read the Cursor local state (read-only snapshot): {reason}"))
}

#[cfg(windows)]
fn cursor_state_db_path() -> Result<PathBuf, ProviderError> {
    let appdata = std::env::var("APPDATA").map_err(|_| {
        ProviderError::new(
            "credential_missing",
            "Could not locate the Cursor local-state directory (APPDATA is not set).",
        )
    })?;
    Ok(PathBuf::from(appdata)
        .join("Cursor")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb"))
}

#[cfg(not(windows))]
fn cursor_state_db_path() -> Result<PathBuf, ProviderError> {
    Err(cursor_not_installed())
}

/// Removes the snapshot directory whatever happens to the read.
struct SnapshotDirGuard {
    path: PathBuf,
}

impl Drop for SnapshotDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Copies the Cursor state database (plus its `-wal` sidecar when present)
/// into a fresh temporary directory. The source files are only ever opened
/// for reading by the copy call itself — never locked, never written, never
/// checkpointed. The `-shm` sidecar is deliberately not copied: it is a
/// transient shared-memory index, and the snapshot rebuilds its own from the
/// copied WAL.
fn snapshot_state_db(source: &Path) -> Result<SnapshotDirGuard, ProviderError> {
    let dir = std::env::temp_dir().join(format!(
        "limitscope-cursor-snapshot-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).map_err(|error| state_unreadable(format!("could not prepare a snapshot directory: {error}")))?;
    let guard = SnapshotDirGuard { path: dir };
    let mut wal_name = source.as_os_str().to_os_string();
    wal_name.push("-wal");
    let wal = PathBuf::from(&wal_name);

    fs::copy(source, guard.path.join("state.vscdb"))
        .map_err(|error| state_unreadable(format!("could not copy the database: {error}")))?;
    if wal.exists() {
        fs::copy(&wal, guard.path.join("state.vscdb-wal"))
            .map_err(|error| state_unreadable(format!("could not copy the database WAL: {error}")))?;
    }
    Ok(guard)
}

/// Opens the snapshot (the WAL, if copied, is replayed here) and reads the
/// token entry. The value is stored as TEXT or BLOB depending on the writer;
/// both decode.
fn read_token_from_snapshot(snapshot_dir: &Path) -> Result<String, ProviderError> {
    use rusqlite::OptionalExtension;
    let connection = rusqlite::Connection::open(snapshot_dir.join("state.vscdb"))
        .map_err(|error| state_unreadable(format!("could not open the snapshot: {error}")))?;
    let value: Option<rusqlite::types::Value> = connection
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            rusqlite::params![TOKEN_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| state_unreadable(format!("could not query the snapshot: {error}")))?;
    match value {
        Some(rusqlite::types::Value::Text(text)) => Ok(text),
        Some(rusqlite::types::Value::Blob(bytes)) => String::from_utf8(bytes)
            .map_err(|_| state_unreadable("the token entry is not valid UTF-8".to_string())),
        _ => Err(credential_missing()),
    }
}

/// One passive read of the Cursor access token: snapshot, query, and let the
/// guard clean the snapshot up. The source database is never mutated.
fn read_token_from_state_db(source: &Path) -> Result<String, ProviderError> {
    let guard = snapshot_state_db(source)?;
    read_token_from_snapshot(&guard.path)
}

/// The `exp` claim of the token's JWT payload (epoch seconds). Only the claim
/// is decoded; the token itself never leaves this module.
fn jwt_exp_epoch(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    // Tolerate padded input; JWT segments are unpadded by spec.
    let payload = payload.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    match value.get("exp")? {
        Value::Number(number) => number.as_f64().map(|seconds| seconds.floor() as i64),
        _ => None,
    }
}

/// Conservative local expiry verdict, mirroring `grok.rs`: an expiry that is
/// missing, malformed, or inside the guard margin fails closed — no network
/// traffic is ever spent on an unverifiable token.
fn verify_token_unexpired(token: &str) -> Result<(), ProviderError> {
    let Some(exp) = jwt_exp_epoch(token) else {
        return Err(credential_expired("could not be verified as unexpired"));
    };
    if Utc::now().timestamp() + EXPIRY_GUARD_SECS >= exp {
        return Err(credential_expired("has expired"));
    }
    Ok(())
}

/// Loads the Cursor access token: a read-only snapshot of the local state,
/// then a local JWT-expiry verdict. No refresh, no credential mutation.
fn load_cursor_token() -> Result<CursorAccessToken, ProviderError> {
    let path = cursor_state_db_path()?;
    if !path.exists() {
        return Err(cursor_not_installed());
    }
    let raw = read_token_from_state_db(&path)?;
    let token = raw.trim();
    if token.is_empty() {
        return Err(credential_missing());
    }
    verify_token_unexpired(token)?;
    Ok(CursorAccessToken(token.to_string()))
}

// ---------- installed client version (passive) ----------

/// A version-like sanity gate: the app's own registration is semver-shaped,
/// so a non-numeric head means the registry entry is not a usable version.
fn version_like(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let head = trimmed.split('.').next().unwrap_or_default();
    (!head.is_empty() && head.chars().all(|c| c.is_ascii_digit())).then(|| trimmed.to_string())
}

#[cfg(windows)]
mod win_registry {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, KEY_READ, REG_SZ,
    };

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
            RegQueryValueExW(key, wide.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut bytes)
        };
        if status != 0 || kind != REG_SZ || bytes == 0 || bytes > 32 * 1024 {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize + 1) / 2];
        let status = unsafe {
            RegQueryValueExW(key, wide.as_ptr(), std::ptr::null(), std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut bytes)
        };
        if status != 0 {
            return None;
        }
        let len = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
        Some(
            OsString::from_wide(&buffer[..len])
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn read_sz_at(root: HKEY, path: &str, name: &str) -> Option<String> {
        let key = open(root, path)?;
        let result = read_sz(key, name);
        unsafe { RegCloseKey(key) };
        result
    }

    /// `(display_name, display_version)` of every direct subkey of
    /// `root\path` whose `DisplayName` REG_SZ contains `contains`
    /// (case-insensitive). Strictly read-only.
    pub(super) fn display_versions_matching(
        root: HKEY,
        path: &str,
        contains: &str,
    ) -> Vec<(String, String)> {
        let Some(key) = open(root, path) else {
            return Vec::new();
        };
        let result = (|| {
            let contains_lower = contains.to_ascii_lowercase();
            let mut matches = Vec::new();
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
                    break;
                }
                let subkey = OsString::from_wide(&name_buffer[..name_len as usize])
                    .to_string_lossy()
                    .into_owned();
                let subkey_path = format!("{path}\\{subkey}");
                let Some(display) = read_sz_at(root, &subkey_path, "DisplayName") else {
                    continue;
                };
                let Some(version) = read_sz_at(root, &subkey_path, "DisplayVersion") else {
                    continue;
                };
                if display.to_ascii_lowercase().contains(&contains_lower) {
                    matches.push((display, version));
                }
            }
            matches
        })();
        unsafe { RegCloseKey(key) };
        result
    }

    pub(super) use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER as CURRENT_USER;
}

/// The installed Grok Bot app version — the value the client sends as
/// `x-cursor-client-version` — read from the app's own Windows uninstall
/// registration (`DisplayVersion` of the entry whose display name names the
/// Grok Bot app). `None` when it cannot be derived, in which case the
/// supplemental observation fails closed rather than fabricating a version.
#[cfg(windows)]
fn installed_client_version() -> Option<String> {
    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    let matches =
        win_registry::display_versions_matching(win_registry::CURRENT_USER, UNINSTALL, "grok");
    if matches.is_empty() {
        return None;
    }
    // Prefer the unambiguous "Grok Bot" entry over any other grok-named
    // entry; several non-matching candidates stay unresolved rather than
    // guessed at.
    let bots: Vec<&(String, String)> = matches
        .iter()
        .filter(|(display, _)| display.to_ascii_lowercase().contains("grok bot"))
        .collect();
    let chosen = match bots.len() {
        1 => Some(bots[0]),
        0 if matches.len() == 1 => Some(&matches[0]),
        _ => None,
    };
    chosen.and_then(|(_, version)| version_like(version))
}

#[cfg(not(windows))]
fn installed_client_version() -> Option<String> {
    // No passive local source for the installed Grok Bot version
    // off-Windows; the supplemental observation fails closed.
    None
}

// ---------- checksum (verified contract) ----------

/// The verified obfuscation chain: the initial key byte is 165, and for each
/// byte index `i` the output is `((input[i] XOR key) + (i mod 256)) mod 256`
/// with the key chained to the previous output byte.
fn checksum_obfuscate(input: &[u8]) -> Vec<u8> {
    let mut key: u8 = CHECKSUM_INITIAL_KEY;
    input
        .iter()
        .enumerate()
        .map(|(index, &byte)| {
            let output = (byte ^ key).wrapping_add((index % 256) as u8);
            key = output;
            output
        })
        .collect()
}

/// The 6-byte big-endian encoding of `floor(now_ms / 1_000_000)`. The bucket
/// value for any plausible epoch fits in 48 bits, so the low six bytes of the
/// i64 big-endian form are the whole value.
fn checksum_bucket(now_ms: i64) -> [u8; 6] {
    let bucket = (now_ms / CHECKSUM_BUCKET_MS).to_be_bytes();
    [bucket[2], bucket[3], bucket[4], bucket[5], bucket[6], bucket[7]]
}

/// The verified checksum: the base64url (unpadded) obfuscated timestamp
/// bucket suffixed with the machine id.
fn build_checksum(now_ms: i64, machine_id: &str) -> String {
    format!(
        "{}{}",
        URL_SAFE_NO_PAD.encode(checksum_obfuscate(&checksum_bucket(now_ms))),
        machine_id
    )
}

/// LimitScope's own machine id for the checksum: a process-stable UUID
/// generated locally, never read from and never written to Grok Bot's or
/// Cursor's persisted state (live-verified server acceptance).
fn machine_id() -> &'static str {
    static MACHINE_ID: OnceLock<String> = OnceLock::new();
    MACHINE_ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

// ---------- request context ----------

/// The full header set the installed Grok Bot client sends for the sand usage
/// request. The machine id rides inside `x-cursor-checksum`; the request id is
/// generated fresh per request (the app does the same) and never persisted.
fn request_headers(
    token: &CursorAccessToken,
    client_version: &str,
    now_ms: i64,
) -> Result<Vec<(String, String)>, ProviderError> {
    Ok(vec![
        (
            "authorization".to_string(),
            format!("Bearer {}", token.0.trim()),
        ),
        (
            "x-cursor-checksum".to_string(),
            build_checksum(now_ms, machine_id()),
        ),
        ("x-cursor-client-type".to_string(), "sand".to_string()),
        ("x-cursor-client-source".to_string(), "sand-desktop".to_string()),
        (
            "x-cursor-client-version".to_string(),
            client_version.to_string(),
        ),
        ("x-sand-box-namespace".to_string(), "prod".to_string()),
        ("x-cursor-client-os".to_string(), "CLIENT_OS_WINDOWS".to_string()),
        ("x-ghost-mode".to_string(), "true".to_string()),
        (
            "x-request-id".to_string(),
            uuid::Uuid::new_v4().to_string(),
        ),
        (
            "user-agent".to_string(),
            format!("Grok Bot/{client_version}"),
        ),
        ("connect-protocol-version".to_string(), "1".to_string()),
    ])
}

fn request_header_map(
    token: &CursorAccessToken,
    client_version: &str,
    now_ms: i64,
) -> Result<reqwest::header::HeaderMap, ProviderError> {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in request_headers(token, client_version, now_ms)? {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .expect("hardcoded header names are valid");
        let value = reqwest::header::HeaderValue::from_str(&value).map_err(|_| {
            ProviderError::new("unexpected", "Grok Bot request header material is not header-safe.")
        })?;
        map.insert(name, value);
    }
    Ok(map)
}

fn http_client() -> Result<&'static reqwest::Client, ProviderError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // A redirect would carry the Authorization header to whatever host
            // the redirect names. A 3xx surfaces as a plain HTTP failure.
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

// ---------- upstream response (only what we consume; extra ignored) ----------
//
// Wire shape verified live 2026-10-06 against
// GetSandUsageStatus (ConnectRPC unary JSON; the response body is the message
// itself). Only the fields the v1 presentation needs are parsed:
// {
//   currentPeriodStart: string | number,
//   nextResetTimestampUtc: string | number,
//   usagePercent: number,
//   hasAvailableUsage: boolean,
//   onDemandSettings: { enabled: boolean, ... } | null,
//   includedUsageSuperGrokPlan: string,     // e.g. "x-premium-plus"
//   grokPlanLabel: string,                  // e.g. "X Premium+"
//   cursorPlanName: string,                 // e.g. "Free"
//   billingBrand: string,
//   usesPooledEnterpriseAllowance: boolean
// }

/// Upstream numbers arrive as JSON numbers or numeric strings (drift
/// tolerance, mirroring `grok.rs`); anything else degrades to `None`.
fn finite_number(raw: Option<&Value>) -> Option<f64> {
    let value = raw?;
    if let Some(number) = value.as_f64() {
        return number.is_finite().then_some(number);
    }
    value.as_str()?.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

/// A display-safe provider-derived label: trimmed, non-empty, control-free,
/// length-capped (the shared display gate). `None` when nothing visible
/// remains.
fn display_label(raw: Option<&Value>) -> Option<String> {
    raw?.as_str().map(str::trim).filter(|text| !text.is_empty()).and_then(sanitize_display_label)
}

/// The period timestamps: an RFC 3339 string is preserved exactly as sent; a
/// numeric epoch (seconds or milliseconds) routes through the shared
/// plausibility-bounded converter. Anything else is no stamp at all.
fn timestamp_value(raw: Option<&Value>) -> Option<String> {
    match raw? {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()
                && DateTime::parse_from_rfc3339(trimmed).is_ok())
            .then(|| trimmed.to_string())
        }
        Value::Number(_) => epoch_ms_to_rfc3339(raw?)
            .map(|stamp| stamp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        _ => None,
    }
}

fn parse_sand_usage(body: &[u8]) -> Result<(GrokBotUsage, SandUsageFacts), ProviderError> {
    let parsed: Value = serde_json::from_slice(body).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "The Grok Bot usage response was not usable JSON (schema change).",
        )
    })?;
    // Fail closed on pooled enterprise allowances: the percentage of a pooled
    // allowance is not the user's own pool, so the observation reports
    // nothing rather than a wrong figure.
    if parsed.get("usesPooledEnterpriseAllowance").and_then(Value::as_bool) == Some(true) {
        return Err(ProviderError::new(
            "unsupported_configuration",
            "The Grok Bot account reports a pooled enterprise allowance; LimitScope does not represent pooled usage.",
        ));
    }
    let Some(percent) = finite_number(parsed.get("usagePercent")) else {
        return Err(ProviderError::new(
            "unexpected_response",
            "The Grok Bot usage response has no usable usage figure (schema change).",
        ));
    };
    let plan_name = display_label(parsed.get("grokPlanLabel"));
    let plan_id = display_label(parsed.get("includedUsageSuperGrokPlan"));
    let cursor_plan_name = display_label(parsed.get("cursorPlanName"));
    let period_start = timestamp_value(parsed.get("currentPeriodStart"));
    let reset_at = timestamp_value(parsed.get("nextResetTimestampUtc"));
    let has_available_usage = parsed.get("hasAvailableUsage").and_then(Value::as_bool);
    // Conservative on-demand verdict: only an explicit boolean `enabled`
    // counts; an absent or non-boolean flag is never assumed true.
    let on_demand_enabled = parsed
        .get("onDemandSettings")
        .and_then(|settings| settings.get("enabled"))
        .and_then(Value::as_bool);
    let usage = GrokBotUsage {
        plan_name,
        plan_id,
        cursor_plan_name,
        used_percent: percent.clamp(0.0, 100.0),
        period_start,
        reset_at,
        has_available_usage,
        on_demand_enabled,
        // Placeholder stamp; `retain_observation` (the freshness authority)
        // re-stamps with the observation clock.
        observed_at: Utc::now(),
    };
    let facts = SandUsageFacts {
        plan_label_present: usage.plan_name.is_some(),
        plan_id_present: usage.plan_id.is_some(),
        usage_percent: Some(usage.used_percent),
        reset_present: usage.reset_at.is_some(),
        period_start_present: usage.period_start.is_some(),
        has_available_usage: usage.has_available_usage,
        on_demand_enabled: usage.on_demand_enabled,
    };
    Ok((usage, facts))
}

// ---------- HTTP ----------

/// One passive observation: exactly one POST against the usage URL with the
/// locally derived header family. No retry, no refresh: on an auth refusal
/// the caller sees the error and Cursor remains the only writer of its own
/// credential store.
async fn fetch_observation(
    client: &reqwest::Client,
    url: &str,
    token: &CursorAccessToken,
    client_version: &str,
) -> Result<(GrokBotUsage, SandUsageFacts), ProviderError> {
    let now_ms = Utc::now().timestamp_millis();
    let response = client
        .post(url)
        .headers(request_header_map(token, client_version, now_ms)?)
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|error| {
            ProviderError::transient(
                "network",
                format!("Could not reach the Grok Bot usage endpoint: {error}"),
            )
            .with_transport_timeout(error.is_timeout())
        })?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::new(
            "auth_invalid",
            "Cursor rejected the credential for the Grok Bot usage endpoint. Re-authenticate in Cursor; LimitScope never refreshes Cursor auth.",
        ));
    }
    let body = response.bytes().await.map_err(|error| {
        ProviderError::transient(
            "network",
            format!("The Grok Bot usage response was cut short: {error}"),
        )
        .with_transport_timeout(error.is_timeout())
    })?;
    if !status.is_success() {
        return Err(ProviderError::http_failure(
            status,
            format!("The Grok Bot usage endpoint returned HTTP {status}."),
        ));
    }
    parse_sand_usage(&body)
}

/// Injectable-core fetch: one POST against `url` with `token`. Tests use it
/// to exercise transport behavior hermetically (redirect refusal, connection
/// refusal) without touching the real store, and the live proof runs this
/// exact code path against the production endpoint.
async fn fetch_observation_from(
    client: &reqwest::Client,
    url: &str,
    token: &CursorAccessToken,
    client_version: &str,
) -> Result<(GrokBotUsage, SandUsageFacts), ProviderError> {
    // Final-boundary value scrub (`secret_scrub`): the token is alive for
    // this whole fetch, so any exact occurrence of it in a failure message
    // is removed before the error leaves the adapter.
    fetch_observation(client, url, token, client_version)
        .await
        .map_err(|error| crate::secret_scrub::scrub_provider_error(error, &[token.0.as_str()]))
}

// ---------- retention (last-good for the observation) ----------

/// The last successful observation, retained so a transient observation
/// failure cannot erase fresh Grok Bot data while it is still within its
/// freshness budget. In-memory only, process-local, never persisted.
static LAST_OBSERVATION: OnceLock<Mutex<Option<GrokBotUsage>>> = OnceLock::new();

fn last_observation() -> &'static Mutex<Option<GrokBotUsage>> {
    LAST_OBSERVATION.get_or_init(|| Mutex::new(None))
}

/// True when an observation timestamp is still within its freshness budget:
/// not future-dated beyond clock-skew tolerance and not older than the TTL.
/// The same predicate the runtime's stale-trim applies to retained entries.
pub fn usage_fresh(observed_at: &DateTime<Utc>, now: DateTime<Utc>) -> bool {
    let age = now.signed_duration_since(*observed_at).num_seconds();
    (-OBSERVED_FUTURE_TOLERANCE_SECS..=OBSERVATION_TTL_SECS).contains(&age)
}

/// Retention core (pure, testable): a fresh observation replaces the cache
/// and passes through; a failure passes through unless a cached observation
/// is still within its freshness budget, which stands in as the last-good
/// result — a stale percentage can thus never linger, but a transient
/// observation failure also cannot erase data that was current moments ago.
fn retain_observation(
    cache: &mut Option<GrokBotUsage>,
    result: Result<GrokBotUsage, ProviderError>,
    now: DateTime<Utc>,
) -> Result<GrokBotUsage, ProviderError> {
    match result {
        Ok(mut usage) => {
            usage.observed_at = now;
            *cache = Some(usage.clone());
            Ok(usage)
        }
        Err(error) => {
            let fresh = cache
                .as_ref()
                .is_some_and(|observation| usage_fresh(&observation.observed_at, now));
            if fresh {
                Ok(cache.clone().expect("freshness checked above"))
            } else {
                *cache = None;
                Err(error)
            }
        }
    }
}

/// The production entry point, called once per Grok provider job inside the
/// ordinary runtime cycle: one passive credential read, one POST, then
/// retention. Never fails the caller's quota path — the runtime consumes
/// this `Result` independently of the xAI billing result.
pub async fn observe_grok_bot_usage() -> Result<GrokBotUsage, ProviderError> {
    let result = async {
        // The client version gates the request before any credential is
        // read: without a derivable installed version the observation fails
        // closed rather than fabricating one.
        let Some(client_version) = installed_client_version() else {
            return Err(ProviderError::new(
                "grok_bot_not_installed",
                "The installed Grok Bot app version could not be derived; the Grok Bot observation is unavailable.",
            ));
        };
        let token = load_cursor_token()?;
        fetch_observation_from(http_client()?, SAND_USAGE_URL, &token, &client_version)
            .await
            .map(|(usage, _)| usage)
    }
    .await;
    let mut cache = last_observation().lock().unwrap();
    retain_observation(&mut cache, result, Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zai::MAX_UPSTREAM_DETAIL_CHARS;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const TEST_TOKEN: &str = "cursor-jwt-value";

    fn test_token() -> CursorAccessToken {
        CursorAccessToken(TEST_TOKEN.to_string())
    }

    // Live-verified shape: X Premium+ weekly pool, ~17.66% used, reset Oct 11.
    fn sand_usage_body() -> Value {
        serde_json::json!({
            "currentPeriodStart": "2026-10-04T09:12:03.000Z",
            "nextResetTimestampUtc": "2026-10-11T09:12:03.000Z",
            "usagePercent": 17.66,
            "hasAvailableUsage": true,
            "hasNonZeroIncludedLimit": true,
            "onDemandSettings": { "enabled": false },
            "includedUsageSuperGrokPlan": "x-premium-plus",
            "grokPlanLabel": "X Premium+",
            "cursorPlanName": "Free",
            "billingBrand": "xai",
            "usesPooledEnterpriseAllowance": false
        })
    }

    // ---------- mutation boundary (source-level proof) ----------

    /// LimitScope observes only. The module source must not contain any
    /// upstream URL but the usage endpoint, any request method but POST (the
    /// verified ConnectRPC unary shape), or any logging of response bodies.
    /// The needle literals are assembled from fragments so this test cannot
    /// reintroduce the very strings it forbids.
    #[test]
    fn module_source_contains_only_the_read_only_usage_endpoint_and_no_logging() {
        let source = include_str!("cursor_grok_bot.rs");

        // Every absolute URL in the module must be the usage endpoint.
        let scheme_needle = format!("{}://", "https");
        for (offset, _) in source.match_indices(&scheme_needle) {
            assert!(
                source[offset..].starts_with(SAND_USAGE_URL),
                "unexpected upstream URL at byte {offset}: the usage endpoint is the only one allowed"
            );
        }

        assert!(
            !source.contains(&format!("Method::{}", "PUT")),
            "no PUT method may exist"
        );
        assert!(
            !source.contains(&format!("Method::{}", "DELETE")),
            "no DELETE method may exist"
        );
        assert!(
            !source.contains(&format!("Method::{}", "PATCH")),
            "no PATCH method may exist"
        );
        assert!(
            !source.contains(&format!(".{}(", "patch")),
            "no PATCH request may exist"
        );

        // Raw response bodies are never logged: no log macros exist (the
        // needle literals are assembled from fragments for the same reason).
        assert!(!source.contains(&format!("{}println!", "e")));
        assert!(!source.contains(&format!("{}::", "log")));
        assert!(!source.contains(&format!("{}::", "tracing")));
    }

    // ---------- checksum (verified contract) ----------

    #[test]
    fn checksum_obfuscation_matches_the_verified_chain() {
        // The expectation is computed independently of the implementation.
        fn expected(input: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            let mut key: u8 = 165;
            for (i, &b) in input.iter().enumerate() {
                let v = ((b ^ key) as u16 + (i % 256) as u16) % 256;
                out.push(v as u8);
                key = v as u8;
            }
            out
        }
        for input in [
            vec![0u8; 6],
            vec![255u8; 6],
            vec![0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc],
        ] {
            assert_eq!(checksum_obfuscate(&input), expected(&input));
        }
        // The chain is stateful: the first byte alone already differs from
        // the naive XOR, pinning the 165 seed.
        assert_eq!(checksum_obfuscate(&[0][0..1]), vec![165]);
    }

    #[test]
    fn checksum_bucket_encodes_six_big_endian_bytes_of_floor_millis() {
        // 2026-10-06T00:00:00Z = 1791312000000 ms; floor(/1e6) = 1791312.
        let now_ms = 1_791_312_000_000_i64;
        let bucket = checksum_bucket(now_ms);
        let recomposed = u64::from_be_bytes([0, 0, bucket[0], bucket[1], bucket[2], bucket[3], bucket[4], bucket[5]]);
        assert_eq!(recomposed, 1_791_312_u64);
        // Floor semantics: anything below the next bucket stays inside.
        assert_eq!(checksum_bucket(now_ms + 999_999), checksum_bucket(now_ms));
        assert_ne!(checksum_bucket(now_ms + 1_000_000), checksum_bucket(now_ms));
    }

    #[test]
    fn build_checksum_is_deterministic_and_base64url_plus_machine_id() {
        let machine = "0f8d5a2e-1111-4222-8333-9444aaaaaaaaaa";
        let first = build_checksum(1_791_312_000_000, machine);
        let second = build_checksum(1_791_312_000_000, machine);
        assert_eq!(first, second, "same bucket + machine id is deterministic");
        // 6 bytes base64url = exactly 8 unpadded characters, then the id.
        let prefix = &first[..first.len() - machine.len()];
        assert_eq!(prefix.len(), 8, "prefix: {prefix}");
        assert!(!prefix.contains('='), "no accidental padding: {prefix}");
        assert!(prefix.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
        }));
        assert!(first.ends_with(machine));
        // A different bucket changes the encoded prefix.
        assert_ne!(
            build_checksum(1_791_313_000_000, machine),
            first,
            "the timestamp bucket participates"
        );
        // A different machine id changes the suffix only.
        let other = build_checksum(1_791_312_000_000, "aaaa");
        assert_eq!(&other[..other.len() - 4], prefix);
    }

    #[test]
    fn machine_id_is_a_stable_process_local_uuid() {
        let first = machine_id();
        let second = machine_id();
        assert_eq!(first, second, "process-stable");
        assert!(uuid::Uuid::parse_str(first).is_ok(), "a valid UUID");
        // Not persisted anywhere: there is no write path in the module at all
        // (pinned by the source-level boundary test above).
    }

    // ---------- parser matrix ----------

    #[test]
    fn live_shape_parses_label_plan_and_weekly_window() {
        let (usage, facts) = parse_sand_usage(sand_usage_body().to_string().as_bytes()).unwrap();
        assert_eq!(usage.plan_name.as_deref(), Some("X Premium+"));
        assert_eq!(usage.plan_id.as_deref(), Some("x-premium-plus"));
        assert_eq!(usage.cursor_plan_name.as_deref(), Some("Free"));
        assert!((usage.used_percent - 17.66).abs() < 1e-9);
        assert_eq!(usage.period_start.as_deref(), Some("2026-10-04T09:12:03.000Z"));
        assert_eq!(usage.reset_at.as_deref(), Some("2026-10-11T09:12:03.000Z"));
        assert_eq!(usage.has_available_usage, Some(true));
        assert_eq!(usage.on_demand_enabled, Some(false));
        assert!(facts.plan_label_present);
        assert!(facts.plan_id_present);
        assert!(facts.reset_present);
        assert!(facts.period_start_present);
    }

    #[test]
    fn missing_plan_label_degrades_without_dropping_the_observation() {
        let mut body = sand_usage_body();
        body.as_object_mut().unwrap().remove("grokPlanLabel");
        let (usage, facts) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.plan_name, None);
        assert_eq!(usage.plan_id.as_deref(), Some("x-premium-plus"));
        assert!((usage.used_percent - 17.66).abs() < 1e-9);
        assert!(!facts.plan_label_present);
    }

    #[test]
    fn missing_usage_percent_fails_closed() {
        let mut body = sand_usage_body();
        body.as_object_mut().unwrap().remove("usagePercent");
        let error = parse_sand_usage(body.to_string().as_bytes()).unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        // A non-numeric figure is the same failure.
        let mut body = sand_usage_body();
        body["usagePercent"] = serde_json::json!("soon-ish");
        assert_eq!(
            parse_sand_usage(body.to_string().as_bytes()).unwrap_err().code,
            "unexpected_response"
        );
    }

    #[test]
    fn numeric_string_percent_is_accepted_and_percent_is_clamped() {
        let mut body = sand_usage_body();
        body["usagePercent"] = serde_json::json!("17.66");
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert!((usage.used_percent - 17.66).abs() < 1e-9);
        let mut body = sand_usage_body();
        body["usagePercent"] = serde_json::json!(142.0);
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.used_percent, 100.0);
        let mut body = sand_usage_body();
        body["usagePercent"] = serde_json::json!(-3.0);
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.used_percent, 0.0);
    }

    #[test]
    fn pooled_enterprise_allowance_fails_closed() {
        let mut body = sand_usage_body();
        body["usesPooledEnterpriseAllowance"] = serde_json::json!(true);
        let error = parse_sand_usage(body.to_string().as_bytes()).unwrap_err();
        assert_eq!(error.code, "unsupported_configuration");
        assert_eq!(error.transient, Some(false), "never retried");
        // Absent or false both stay supported.
        let mut body = sand_usage_body();
        body.as_object_mut().unwrap().remove("usesPooledEnterpriseAllowance");
        assert!(parse_sand_usage(body.to_string().as_bytes()).is_ok());
    }

    #[test]
    fn malformed_responses_are_schema_changes_not_panics() {
        let html = b"<html><body>Please sign in</body></html>";
        assert_eq!(
            parse_sand_usage(html).unwrap_err().code,
            "unexpected_response"
        );
        assert_eq!(
            parse_sand_usage(b"{}".as_slice()).unwrap_err().code,
            "unexpected_response"
        );
        assert_eq!(
            parse_sand_usage(serde_json::json!([1, 2]).to_string().as_bytes())
                .unwrap_err()
                .code,
            "unexpected_response"
        );
    }

    #[test]
    fn unknown_upstream_fields_are_ignored() {
        let mut body = sand_usage_body();
        body["billingBrand"] = serde_json::json!("xai");
        body["someFutureField"] = serde_json::json!({ "nested": [1, 2] });
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.plan_name.as_deref(), Some("X Premium+"));
    }

    #[test]
    fn timestamps_accept_rfc3339_strings_and_epoch_numbers_only() {
        let mut body = sand_usage_body();
        body["nextResetTimestampUtc"] = serde_json::json!(1_791_830_032_000_i64);
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.reset_at.as_deref(), Some("2026-10-12T18:33:52Z"));
        let mut body = sand_usage_body();
        body["nextResetTimestampUtc"] = serde_json::json!("not-a-timestamp");
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.reset_at, None);
        let mut body = sand_usage_body();
        body["nextResetTimestampUtc"] = serde_json::json!(null);
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.reset_at, None);
    }

    #[test]
    fn on_demand_flag_is_only_an_explicit_boolean() {
        // Absent settings stay absent — never assumed true.
        let mut body = sand_usage_body();
        body.as_object_mut().unwrap().remove("onDemandSettings");
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.on_demand_enabled, None);
        // A non-boolean flag is equally absent.
        let mut body = sand_usage_body();
        body["onDemandSettings"] = serde_json::json!({ "enabled": "yes" });
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.on_demand_enabled, None);
        // An explicit true is carried as-is.
        let mut body = sand_usage_body();
        body["onDemandSettings"] = serde_json::json!({ "enabled": true });
        let (usage, _) = parse_sand_usage(body.to_string().as_bytes()).unwrap();
        assert_eq!(usage.on_demand_enabled, Some(true));
    }

    #[test]
    fn wire_keeps_server_figures_but_never_the_internal_stamp_or_credentials() {
        let (usage, _) = parse_sand_usage(sand_usage_body().to_string().as_bytes()).unwrap();
        let wire = serde_json::to_string(&usage).unwrap();
        assert!(wire.contains("planName"), "wire: {wire}");
        assert!(wire.contains("usedPercent"), "wire: {wire}");
        assert!(wire.contains("resetAt"), "wire: {wire}");
        assert!(
            !wire.contains("observed_at") && !wire.contains("observedAt"),
            "internal stamp stays off the wire: {wire}"
        );
        assert!(
            !wire.contains(TEST_TOKEN),
            "the credential must never ride the wire: {wire}"
        );
    }

    #[test]
    fn credential_newtype_never_serializes_and_never_debugs() {
        // Serialize always fails (compile-time wire guard).
        assert!(serde_json::to_string(&test_token()).is_err());
        // Debug never shows the value.
        let debug = format!("{:?}", test_token());
        assert!(!debug.contains(TEST_TOKEN), "debug: {debug}");
        assert!(debug.contains("<redacted>"));
    }

    // ---------- JWT expiry verdict (fail closed) ----------

    fn jwt_with_exp(exp: Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn token_expiry_is_verified_locally_and_fails_closed() {
        // Valid: more than the guard margin ahead.
        let future = Utc::now().timestamp() + 3600;
        assert!(verify_token_unexpired(&jwt_with_exp(serde_json::json!(future))).is_ok());
        // Inside the guard margin counts as expired.
        let borderline = Utc::now().timestamp() + 30;
        assert!(verify_token_unexpired(&jwt_with_exp(serde_json::json!(borderline))).is_err());
        // Past expiry.
        let past = Utc::now().timestamp() - 60;
        assert!(verify_token_unexpired(&jwt_with_exp(serde_json::json!(past))).is_err());
        // Missing or malformed expiry must never read as valid.
        assert!(verify_token_unexpired(&jwt_with_exp(serde_json::json!("soon"))).is_err());
        let no_exp = format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#),
            URL_SAFE_NO_PAD.encode(r#"{"sub":"someone"}"#)
        );
        assert!(verify_token_unexpired(&no_exp).is_err());
        assert!(verify_token_unexpired("garbage").is_err());
        // The verdict errors are the documented codes.
        let error = verify_token_unexpired("garbage").unwrap_err();
        assert_eq!(error.code, "credential_expired");
    }

    // ---------- Cursor state DB (hermetic, real SQLite files) ----------

    mod state_db {
        use super::*;
        use rusqlite::Connection;
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        fn temp_dir(tag: &str) -> PathBuf {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "limitscope-cursor-test-{}-{}-{}",
                tag,
                std::process::id(),
                n
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            path
        }

        /// Builds a state.vscdb-shaped database in WAL mode with the token
        /// row, keeping the writer connection open so the newest insert is
        /// still uncheckpointed WAL data — the exact state a live Cursor
        /// presents to a passive reader.
        fn wal_db_fixture(dir: &Path, token: &str) -> Connection {
            let db = dir.join("state.vscdb");
            let connection = Connection::open(&db).unwrap();
            connection.pragma_update(None, "journal_mode", "WAL").unwrap();
            connection
                .execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)", [])
                .unwrap();
            connection
                .execute(
                    "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                    rusqlite::params![TOKEN_KEY, token],
                )
                .unwrap();
            connection
        }

        #[test]
        fn token_is_read_through_a_live_wal_without_mutating_the_source() {
            let dir = temp_dir("wal-visible");
            let db = dir.join("state.vscdb");
            let writer = wal_db_fixture(&dir, TEST_TOKEN);
            // The writer connection stays open with uncheckpointed WAL data.
            let before = fs::read(&db).unwrap_or_default();

            let read = read_token_from_state_db(&dir.join("state.vscdb")).unwrap();
            assert_eq!(read.trim(), TEST_TOKEN);

            // The WAL sidecar exists (the writer never checkpointed).
            assert!(dir.join("state.vscdb-wal").exists());
            // The source database bytes are unchanged by the read; the file
            // set is unchanged too.
            let after = fs::read(&db).unwrap();
            assert_eq!(before.len(), after.len(), "source db size unchanged");
            drop(writer);
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn checkpointed_database_is_read_the_same_way() {
            let dir = temp_dir("plain-db");
            let writer = wal_db_fixture(&dir, TEST_TOKEN);
            drop(writer); // closing checkpoints the WAL into the db
            let read = read_token_from_state_db(&dir.join("state.vscdb")).unwrap();
            assert_eq!(read.trim(), TEST_TOKEN);
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn blob_and_text_values_both_decode() {
            let dir = temp_dir("blob-value");
            let db = dir.join("state.vscdb");
            let connection = Connection::open(&db).unwrap();
            connection
                .execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)", [])
                .unwrap();
            connection
                .execute(
                    "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                    rusqlite::params![TOKEN_KEY, TEST_TOKEN.as_bytes()],
                )
                .unwrap();
            drop(connection);
            let read = read_token_from_state_db(&db).unwrap();
            assert_eq!(read.trim(), TEST_TOKEN);
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn missing_key_and_missing_database_fail_closed() {
            let dir = temp_dir("missing-key");
            let db = dir.join("state.vscdb");
            let connection = Connection::open(&db).unwrap();
            connection
                .execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)", [])
                .unwrap();
            connection
                .execute(
                    "INSERT INTO ItemTable (key, value) VALUES ('other/key', 'x')",
                    [],
                )
                .unwrap();
            drop(connection);
            let error = read_token_from_state_db(&db).unwrap_err();
            assert_eq!(error.code, "credential_missing");

            // A database path that does not exist at all.
            let error = read_token_from_state_db(&dir.join("absent.vscdb")).unwrap_err();
            assert_eq!(error.code, "auth_unreadable");
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn the_snapshot_never_touches_the_source_directory_beyond_reading() {
            let dir = temp_dir("snapshot-no-touch");
            let writer = wal_db_fixture(&dir, TEST_TOKEN);
            let entries_before: Vec<_> = fs::read_dir(&dir).unwrap().collect();
            let _ = read_token_from_state_db(&dir.join("state.vscdb")).unwrap();
            let entries_after: Vec<_> = fs::read_dir(&dir).unwrap().collect();
            assert_eq!(
                entries_before.len(),
                entries_after.len(),
                "no file is created, removed, or renamed in the source directory"
            );
            drop(writer);
            let _ = fs::remove_dir_all(&dir);
        }
    }

    // ---------- installed version gate ----------

    #[test]
    fn version_gate_accepts_only_semver_shaped_versions() {
        assert_eq!(version_like("0.66.0").as_deref(), Some("0.66.0"));
        assert_eq!(version_like(" 1.2.3 ").as_deref(), Some("1.2.3"));
        assert_eq!(version_like(""), None);
        assert_eq!(version_like("beta"), None);
        assert_eq!(version_like(".5"), None);
    }

    // ---------- transport behavior (hermetic, local sockets) ----------

    fn local_url(listener: &TcpListener) -> String {
        format!("http://{}/GetSandUsageStatus", listener.local_addr().unwrap())
    }

    /// The verified request shape: ConnectRPC unary JSON POST with the
    /// credential, checksum, client-type family, and a fresh request id.
    #[tokio::test]
    async fn request_shape_matches_the_verified_contract() {
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = local_url(&source);
        let body = sand_usage_body().to_string();
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
            String::from_utf8_lossy(&request).to_ascii_lowercase()
        });
        let (usage, _) = fetch_observation_from(
            http_client().unwrap(),
            &url,
            &test_token(),
            "0.66.0",
        )
        .await
        .unwrap();
        assert_eq!(usage.plan_name.as_deref(), Some("X Premium+"));
        assert!((usage.used_percent - 17.66).abs() < 1e-9);
        let request = server.join().unwrap();
        assert!(request.starts_with("post /getsandusagestatus"), "request line: {request}");
        assert!(request.contains("authorization: bearer cursor-jwt-value"));
        assert!(request.contains("x-cursor-checksum:"));
        assert!(request.contains("x-cursor-client-type: sand"));
        assert!(request.contains("x-cursor-client-source: sand-desktop"));
        assert!(request.contains("x-cursor-client-version: 0.66.0"));
        assert!(request.contains("x-sand-box-namespace: prod"));
        assert!(request.contains("x-cursor-client-os: client_os_windows"));
        assert!(request.contains("x-ghost-mode: true"));
        assert!(request.contains("x-request-id:"));
        assert!(request.contains("user-agent: grok bot/0.66.0"));
        assert!(request.contains("content-type: application/json"));
        // The unary body is exactly the empty object.
        let body_start = request.rfind("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        assert!(request[body_start..].starts_with("{}"));
        // The credential never leaks into the checksum header: its value is
        // exactly the 8-character base64url prefix plus the machine uuid.
        let checksum = request
            .split("x-cursor-checksum: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        assert_eq!(checksum.len(), 8 + machine_id().len(), "checksum: {checksum}");
        assert!(checksum.ends_with(machine_id()), "checksum: {checksum}");
        assert!(!checksum.contains(TEST_TOKEN));
    }

    /// A 401 is a deterministic auth failure — never retried, and the message
    /// never embeds credential material.
    #[tokio::test]
    async fn auth_rejection_is_deterministic_and_credential_free() {
        let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = local_url(&source);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        let error = fetch_observation_from(http_client().unwrap(), &url, &test_token(), "0.66.0")
            .await
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code, "auth_invalid");
        assert_eq!(error.transient, Some(false));
        assert!(!error.message.contains(TEST_TOKEN));
    }

    /// 429/5xx stay transient with the HTTP status carried.
    #[tokio::test]
    async fn rate_limit_and_server_errors_are_transient() {
        for (line, expected_status) in [
            ("429 Too Many Requests", Some(429u16)),
            ("500 Internal Server Error", Some(500u16)),
        ] {
            let source = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let url = local_url(&source);
            let server = std::thread::spawn(move || {
                let (mut stream, _) = source.accept().unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
            });
            let error = fetch_observation_from(http_client().unwrap(), &url, &test_token(), "0.66.0")
                .await
                .unwrap_err();
            server.join().unwrap();
            assert_eq!(error.code, "unexpected_response");
            assert_eq!(error.transient, Some(true));
            assert_eq!(error.http_status, expected_status);
            assert!(!error.message.contains(TEST_TOKEN));
        }
    }

    /// The shared client refuses redirects: a 302 from the usage endpoint is
    /// surfaced as-is, never followed, so the credential cannot be relayed
    /// elsewhere.
    #[tokio::test]
    async fn usage_redirect_is_returned_without_following() {
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
        });
        let error = fetch_observation_from(
            http_client().unwrap(),
            &format!("http://{source_addr}/GetSandUsageStatus"),
            &test_token(),
            "0.66.0",
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "unexpected_response");
        assert_eq!(error.http_status, Some(302));
        source_thread.join().unwrap();
        assert!(
            destination_thread.join().unwrap().unwrap_err().kind() == std::io::ErrorKind::WouldBlock,
            "the redirect target must never be contacted"
        );
    }

    /// A transport-level failure (connection refused) is a transient network
    /// error, never a credential verdict, and carries no upstream text.
    #[tokio::test]
    async fn transport_failure_is_a_transient_network_error() {
        let error = fetch_observation_from(
            http_client().unwrap(),
            "http://127.0.0.1:9/GetSandUsageStatus",
            &test_token(),
            "0.66.0",
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "network");
        assert_eq!(error.transient, Some(true));
    }

    /// End-to-end leak simulation: an upstream response whose error path
    /// echoes the in-flight credential back inside a transport message. The
    /// surfaced failure must no longer contain the token value.
    #[test]
    fn error_scrubbing_removes_the_token_from_failure_messages() {
        let error = ProviderError::transient(
            "network",
            format!("request refused for bearer {TEST_TOKEN} (see logs)"),
        );
        let scrubbed = crate::secret_scrub::scrub_provider_error(error, &[TEST_TOKEN]);
        assert!(!scrubbed.message.contains(TEST_TOKEN));
        assert!(scrubbed.message.len() <= MAX_UPSTREAM_DETAIL_CHARS + 200);
    }

    // ---------- retention ----------

    #[test]
    fn retention_replaces_the_cache_with_fresh_observations() {
        let mut cache: Option<GrokBotUsage> = None;
        let (usage, _) = parse_sand_usage(sand_usage_body().to_string().as_bytes()).unwrap();
        let now = Utc::now();
        let retained = retain_observation(&mut cache, Ok(usage), now).unwrap();
        assert_eq!(retained.observed_at, now);
        assert_eq!(cache.as_ref().unwrap().observed_at, now);
    }

    #[test]
    fn retention_serves_fresh_cached_data_across_a_transient_failure() {
        let mut cache: Option<GrokBotUsage> = None;
        let (usage, _) = parse_sand_usage(sand_usage_body().to_string().as_bytes()).unwrap();
        let now = Utc::now();
        retain_observation(&mut cache, Ok(usage), now).unwrap();

        let failure = ProviderError::transient("network", "offline");
        let served = retain_observation(&mut cache, Err(failure), now + chrono::Duration::seconds(60)).unwrap();
        assert!((served.used_percent - 17.66).abs() < 1e-9);
        assert!(cache.is_some());
    }

    #[test]
    fn retention_drops_the_cache_once_the_ttl_is_exhausted() {
        let mut cache: Option<GrokBotUsage> = None;
        let (usage, _) = parse_sand_usage(sand_usage_body().to_string().as_bytes()).unwrap();
        let now = Utc::now();
        retain_observation(&mut cache, Ok(usage), now).unwrap();

        let failure = ProviderError::transient("network", "offline");
        let error = retain_observation(
            &mut cache,
            Err(failure),
            now + chrono::Duration::seconds(OBSERVATION_TTL_SECS + 60),
        )
        .unwrap_err();
        assert_eq!(error.code, "network");
        assert!(cache.is_none(), "the stale observation must not linger");
    }

    #[test]
    fn freshness_window_matches_the_ttl_contract() {
        let now = Utc::now();
        assert!(usage_fresh(&now, now));
        assert!(usage_fresh(&(now - chrono::Duration::seconds(OBSERVATION_TTL_SECS)), now));
        assert!(!usage_fresh(
            &(now - chrono::Duration::seconds(OBSERVATION_TTL_SECS + 1)),
            now
        ));
        // Future stamps within the clock-skew tolerance stay fresh.
        assert!(usage_fresh(&(now + chrono::Duration::seconds(120)), now));
        assert!(!usage_fresh(
            &(now + chrono::Duration::seconds(OBSERVED_FUTURE_TOLERANCE_SECS + 60)),
            now
        ));
    }
}
