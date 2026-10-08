//! Grok Bot desktop usage — manual, read-only Windows UI Automation read.
//!
//! Grok Bot (the xAI desktop app) exposes its Usage & Billing screen through
//! the standard Windows accessibility tree: a semantic settings-panel anchor
//! (`sand-settings-panel-usage`), a Weekly usage progress bar carrying an
//! exact `RangeValuePattern` percentage, and day-granularity reset text such
//! as "Resets in 3 days" (verified live, PoC B1, on app versions 0.66.0 and
//! 0.68.1; reads succeed foreground and background at roughly 26–41 ms).
//!
//! Contract — manual and read-only, by design:
//! - the ONLY entry point is the `refresh_grok_bot_usage` command, invoked
//!   exclusively by an explicit user action in the Grok detail card. There is
//!   no scheduler, no timer, and no hook into the shared runtime cycle: this
//!   module never runs unless the user asked for it;
//! - enumeration is scoped strictly to processes whose image name matches
//!   the Grok Bot candidates below and to that process's own top-level
//!   windows. No other application is ever touched;
//! - the read uses only Windows accessibility interfaces. It never launches,
//!   navigates, clicks, types into, or restarts Grok Bot; no input
//!   synthesis, no pattern that mutates provider state, no window manager
//!   commands;
//! - the panel is located through stable semantic anchors only (the settings
//!   panel AutomationId, the weekly section text, progress-bar and text
//!   control types). Generated React AutomationIds (`base-ui-_r_*`) are
//!   never used as anchors;
//! - the result is a typed, sanitized summary: used percentage, the reset
//!   countdown text verbatim, an observation stamp, and the app version.
//!   No other accessibility text is returned, logged, or persisted;
//! - no credentials, cookies, or tokens of any kind are read — this module
//!   touches no Grok Bot files and makes no network requests;
//! - the scan runs on a dedicated background thread with a hard watchdog
//!   (`SCAN_TIMEOUT_MS`), never on the UI thread, and the async runtime is
//!   never blocked by it;
//! - the last successful reading is retained in memory (process-local,
//!   never persisted) and returned alongside a failed attempt, so the UI can
//!   show it clearly marked as last known. A failed attempt never overwrites
//!   it.
//!
//! The user keeps ownership of navigation: Grok Bot must already be running
//! with Settings → Usage & Billing open for a read to succeed. When it is
//! not, the command answers `not_running` / `screen_not_visible` instead of
//! reaching for the app.

use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

/// RFC 3339 UTC, second precision — the repo's wire convention for stamps.
fn wire_stamp(moment: DateTime<Utc>) -> String {
    moment.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Hard upper bound for one accessibility scan, wall-clock. A healthy read
/// takes tens of milliseconds; anything past this is treated as a failed
/// attempt (`unknown`) rather than blocking the caller.
#[cfg(windows)]
const SCAN_TIMEOUT_MS: u64 = 2_500;

/// Process image names (case-insensitive, without path) accepted as Grok Bot.
/// A name match alone reads nothing — every read still requires the semantic
/// settings-panel anchor inside that process's own window.
const GROK_BOT_EXE_NAMES: &[&str] = &["grok.exe", "grok bot.exe", "grokbot.exe"];

/// The semantic AutomationId of the Usage & Billing settings panel. Stable
/// across the verified app versions, unlike generated `base-ui-_r_*` ids.
const USAGE_PANEL_AUTOMATION_ID: &str = "sand-settings-panel-usage";

/// At most this many progress bars are inspected inside the usage panel.
const MAX_PROGRESS_BARS: usize = 16;
/// At most this many text elements are inspected for the reset countdown.
const MAX_TEXT_ELEMENTS: usize = 64;
/// At most this many of the process's windows are probed for the panel.
const MAX_WINDOWS: usize = 4;

// ---------- wire contract (camelCase on the wire, status snake_case) ----------

/// Outcome of one observation attempt. `ok` means the panel was found and at
/// least one of the weekly values was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrokBotStatus {
    Ok,
    NotRunning,
    ScreenNotVisible,
    Unknown,
}

/// One successful reading. `observed_at` is when the scan actually ran (RFC
/// 3339 UTC) — the UI's "Last updated" line shows exactly this stamp.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokBotSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub observed_at: String,
}

/// The command's answer: the fresh attempt (top-level fields) plus, when one
/// exists, the retained last successful reading. The command never returns an
/// error envelope — every failure mode is a typed status.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokBotRefresh {
    pub status: GrokBotStatus,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_known: Option<GrokBotSnapshot>,
}

/// What the platform scan produced, before retention is applied.
#[derive(Debug, Clone, PartialEq)]
struct ScanOutcome {
    status: GrokBotStatus,
    /// Present only for a successful (`ok`) scan that read at least one
    /// value — the candidate for the last-known cache.
    snapshot: Option<GrokBotSnapshot>,
}

impl ScanOutcome {
    fn not_running() -> Self {
        Self {
            status: GrokBotStatus::NotRunning,
            snapshot: None,
        }
    }

    fn screen_not_visible() -> Self {
        Self {
            status: GrokBotStatus::ScreenNotVisible,
            snapshot: None,
        }
    }

    fn unknown() -> Self {
        Self {
            status: GrokBotStatus::Unknown,
            snapshot: None,
        }
    }

    fn ok(snapshot: GrokBotSnapshot) -> Self {
        Self {
            status: GrokBotStatus::Ok,
            snapshot: Some(snapshot),
        }
    }
}

// ---------- sanitization (pure, hermetically testable) ----------

/// Normalizes a raw pattern value into a used percentage and validates it:
/// scaled by the pattern's own minimum/maximum when the range is meaningful,
/// then required to be finite and within 0–100. Anything else is discarded —
/// missing data stays missing, never zero.
pub fn sanitize_used_percent(value: f64, minimum: f64, maximum: f64) -> Option<f64> {
    let span = maximum - minimum;
    let percent = if span.is_finite() && span > f64::EPSILON {
        (value - minimum) / span * 100.0
    } else {
        value
    };
    if percent.is_finite() && (0.0..=100.0).contains(&percent) {
        Some((percent * 100.0).round() / 100.0)
    } else {
        None
    }
}

/// Accessible-name fallback: accepts the name only when it carries exactly
/// one percentage-shaped token, so a name with several numbers can never be
/// misread as a quota. A validated single match outside 0–100 is rejected.
pub fn parse_percent_from_name(name: &str) -> Option<f64> {
    let mut matches: Vec<f64> = Vec::new();
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'%' {
                if let Ok(value) = name[start..i].parse::<f64>() {
                    matches.push(value);
                }
            }
        } else {
            i += 1;
        }
    }
    if matches.len() == 1 {
        sanitize_used_percent(matches[0], 0.0, 100.0)
    } else {
        None
    }
}

/// Accepts a reset countdown line only in the verified source shape — a
/// "Resets in …" / "Reset in …" prefix — and returns it trimmed, with control
/// characters stripped, capped at a sane length. The text is returned
/// verbatim (never re-interpreted into an absolute timestamp); anything that
/// is not clearly that line is dropped rather than guessed at.
pub fn sanitize_reset_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();
    if !(lower.starts_with("resets in") || lower.starts_with("reset in")) {
        return None;
    }
    let cleaned: String = trimmed
        .chars()
        // Control characters and invisible format marks (zero-width spaces,
        // BOM) never belong in the displayed line; everything else is kept
        // verbatim.
        .filter(|c| !c.is_control() && !matches!(*c as u32, 0x200b..=0x200f | 0xfeff))
        .take(80)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Keeps a file-version string only when it is version-shaped: digits and
/// separators, at most 32 characters.
pub fn sanitize_app_version(version: &str) -> Option<String> {
    let cleaned: String = version
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .take(32)
        .collect();
    if cleaned.chars().any(|c| c.is_ascii_digit()) {
        Some(cleaned)
    } else {
        None
    }
}

// ---------- retention (last-known for the reading) ----------

/// The last successful reading, retained so a failed refresh cannot erase
/// it from the UI. In-memory only, process-local, never persisted, and there
/// is deliberately no TTL: manual refresh means the cadence is the user's,
/// and the card always shows the reading's own observation stamp.
static LAST_KNOWN: OnceLock<Mutex<Option<GrokBotSnapshot>>> = OnceLock::new();

fn last_known() -> &'static Mutex<Option<GrokBotSnapshot>> {
    LAST_KNOWN.get_or_init(|| Mutex::new(None))
}

/// Retention core (pure, testable): a successful reading replaces the cache;
/// a failed attempt leaves it untouched. The cache is cleared only when a
/// successful scan reads nothing at all, which cannot happen today (`ok`
/// requires at least one value) — kept explicit so the invariant survives
/// future edits.
fn retain_reading(
    cache: &mut Option<GrokBotSnapshot>,
    outcome: &ScanOutcome,
) -> Option<GrokBotSnapshot> {
    if outcome.status == GrokBotStatus::Ok {
        if let Some(snapshot) = outcome.snapshot.clone() {
            *cache = Some(snapshot);
        }
    }
    cache.clone()
}

// ---------- the one command: explicit, user-triggered only ----------

/// Reads Grok Bot's Usage & Billing values once, through Windows UI
/// Automation, and returns the attempt plus the retained last-known reading.
/// Callers must invoke this only on explicit user action — nothing in this
/// module ever schedules itself.
#[tauri::command]
pub async fn refresh_grok_bot_usage() -> GrokBotRefresh {
    let observed_at = wire_stamp(Utc::now());
    let outcome = tauri::async_runtime::spawn_blocking(perform_scan)
        .await
        .unwrap_or_else(|_| ScanOutcome::unknown());

    let snapshot = outcome.snapshot.clone().unwrap_or(GrokBotSnapshot {
        used_percent: None,
        reset_text: None,
        app_version: None,
        observed_at: observed_at.clone(),
    });
    let last_known = {
        let mut cache = last_known().lock().unwrap();
        retain_reading(&mut cache, &outcome)
    };

    GrokBotRefresh {
        status: outcome.status,
        observed_at,
        used_percent: snapshot.used_percent,
        reset_text: snapshot.reset_text,
        app_version: snapshot.app_version,
        last_known,
    }
}

/// Dispatches to the platform reader. Off-Windows builds answer `unknown`:
/// the read is a Windows accessibility contract and has no fallback source.
#[cfg(windows)]
fn perform_scan() -> ScanOutcome {
    native::scan()
}

#[cfg(not(windows))]
fn perform_scan() -> ScanOutcome {
    ScanOutcome::unknown()
}

// ---------- platform reader ----------

#[cfg(windows)]
mod native {
    use std::mem::ManuallyDrop;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::{
        parse_percent_from_name, sanitize_app_version, sanitize_reset_text, sanitize_used_percent,
        wire_stamp, GrokBotSnapshot, ScanOutcome, GROK_BOT_EXE_NAMES, MAX_PROGRESS_BARS,
        MAX_TEXT_ELEMENTS, MAX_WINDOWS, SCAN_TIMEOUT_MS, USAGE_PANEL_AUTOMATION_ID,
    };
    use chrono::Utc;
    use windows::core::{BSTR, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoExW, GetFileVersionInfoSizeExW, VerQueryValueW, FILE_VER_GET_NEUTRAL,
        VS_FIXEDFILEINFO,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::System::Variant::{VariantClear, VariantInit, VT_BSTR, VT_I4};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement,
        IUIAutomationElementArray, IUIAutomationRangeValuePattern, TreeScope_Descendants,
        UIA_AutomationIdPropertyId, UIA_ControlTypePropertyId, UIA_ProgressBarControlTypeId,
        UIA_RangeValuePatternId, UIA_TextControlTypeId, UIA_PROPERTY_ID,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
    };

    /// The scan runs on its own short-lived thread so COM apartment state,
    /// UIA handles, and any stall stay contained; the caller waits under the
    /// hard watchdog and answers `unknown` if the thread misses it.
    pub(super) fn scan() -> ScanOutcome {
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("grok-bot-uia-read".to_string())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(run_scan))
                    .unwrap_or_else(|_| ScanOutcome::unknown());
                let _ = tx.send(outcome);
            });
        match spawned {
            Ok(_) => match rx.recv_timeout(Duration::from_millis(SCAN_TIMEOUT_MS)) {
                Ok(outcome) => outcome,
                // Detached: the thread finishes on its own and its result is
                // dropped — the caller already answered `unknown`.
                Err(_) => ScanOutcome::unknown(),
            },
            Err(_) => ScanOutcome::unknown(),
        }
    }

    fn run_scan() -> ScanOutcome {
        unsafe {
            let com = CoInitializeEx(None, COINIT_MULTITHREADED);
            if com.is_err() {
                // A fresh thread has no prior apartment: failure here means
                // COM itself is unavailable, not a mode conflict to retry.
                return ScanOutcome::unknown();
            }
            let outcome = read_grok_bot();
            CoUninitialize();
            outcome
        }
    }

    fn read_grok_bot() -> ScanOutcome {
        unsafe {
            let windows = find_grok_bot_windows();
            if windows.is_empty() {
                return ScanOutcome::not_running();
            }

            let uia: IUIAutomation =
                match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                    Ok(uia) => uia,
                    Err(_) => return ScanOutcome::unknown(),
                };

            let mut probed = 0usize;
            for window in windows.iter().take(MAX_WINDOWS) {
                probed += 1;
                let root = match uia.ElementFromHandle(window.hwnd) {
                    Ok(root) => root,
                    // This window exposes no accessibility tree — keep it out
                    // of the "settings not open" verdict, try the next one.
                    Err(_) => continue,
                };
                let Some(panel) = find_usage_panel(&uia, &root) else {
                    continue;
                };
                return read_usage_panel(&uia, &panel, &window.exe_path);
            }
            if probed == 0 {
                return ScanOutcome::unknown();
            }
            // Grok Bot is running, but none of its windows shows the Usage &
            // Billing screen.
            ScanOutcome::screen_not_visible()
        }
    }

    // ----- window discovery: strictly the Grok Bot process -----

    struct WindowProbe {
        entries: Vec<WindowEntry>,
    }

    struct WindowEntry {
        hwnd: HWND,
        exe_path: String,
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        if !IsWindowVisible(hwnd).as_bool() {
            return windows::core::BOOL(1);
        }
        // Zero-length titles belong to shell/tool windows, not an app's main
        // window — cheapest filter before touching any process state.
        if GetWindowTextLengthW(hwnd) == 0 {
            return windows::core::BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        if pid == 0 {
            return windows::core::BOOL(1);
        }
        if let Some(exe_path) = process_image_path(pid) {
            if is_grok_bot_exe(&exe_path) {
                let probe = &mut *(lparam.0 as *mut WindowProbe);
                if probe.entries.len() < MAX_WINDOWS {
                    probe.entries.push(WindowEntry { hwnd, exe_path });
                }
            }
        }
        windows::core::BOOL(1)
    }

    fn is_grok_bot_exe(exe_path: &str) -> bool {
        let file_name = exe_path.rsplit(['\\', '/']).next().unwrap_or(exe_path);
        let lowered = file_name.to_lowercase();
        GROK_BOT_EXE_NAMES
            .iter()
            .any(|candidate| lowered == **candidate)
    }

    fn process_image_path(pid: u32) -> Option<String> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = [0u16; 1024];
            let mut length = buffer.len() as u32;
            let status = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            );
            let _ = CloseHandle(process);
            if status.is_err() || length == 0 {
                return None;
            }
            Some(String::from_utf16_lossy(&buffer[..length as usize]))
        }
    }

    fn find_grok_bot_windows() -> Vec<WindowEntry> {
        let mut probe = WindowProbe {
            entries: Vec::new(),
        };
        unsafe {
            // Enumeration errors yield whatever was collected — never a
            // broader scan.
            let _ = EnumWindows(
                Some(enum_windows_proc),
                LPARAM(&mut probe as *mut WindowProbe as isize),
            );
        }
        probe.entries
    }

    // ----- semantic anchors inside the app's own window -----

    fn property_condition_bstr(
        uia: &IUIAutomation,
        property: UIA_PROPERTY_ID,
        value: &str,
    ) -> windows::core::Result<IUIAutomationCondition> {
        unsafe {
            let mut variant = VariantInit();
            (*variant.Anonymous.Anonymous).vt = VT_BSTR;
            (*variant.Anonymous.Anonymous).Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(value));
            let condition = uia.CreatePropertyCondition(property, &variant);
            let _ = VariantClear(&mut variant);
            condition
        }
    }

    fn property_condition_i32(
        uia: &IUIAutomation,
        property: UIA_PROPERTY_ID,
        value: i32,
    ) -> windows::core::Result<IUIAutomationCondition> {
        unsafe {
            let mut variant = VariantInit();
            (*variant.Anonymous.Anonymous).vt = VT_I4;
            (*variant.Anonymous.Anonymous).Anonymous.lVal = value;
            let condition = uia.CreatePropertyCondition(property, &variant);
            let _ = VariantClear(&mut variant);
            condition
        }
    }

    /// Locates the Usage & Billing settings panel through its semantic
    /// AutomationId, scoped to the app's own window subtree. `None` means
    /// this window does not currently show that screen.
    fn find_usage_panel(
        uia: &IUIAutomation,
        root: &IUIAutomationElement,
    ) -> Option<IUIAutomationElement> {
        unsafe {
            let condition =
                property_condition_bstr(uia, UIA_AutomationIdPropertyId, USAGE_PANEL_AUTOMATION_ID)
                    .ok()?;
            root.FindFirst(TreeScope_Descendants, &condition).ok()
        }
    }

    fn element_name(element: &IUIAutomationElement) -> String {
        unsafe { element.CurrentName().unwrap_or_default().to_string() }
    }

    /// Reads the weekly used percentage from the panel's progress bars:
    /// the exact `RangeValuePattern` value first, the validated single
    /// percentage in the accessible name as a fallback. Bars whose name
    /// names the weekly window win over unnamed ones.
    fn read_weekly_used_percent(uia: &IUIAutomation, panel: &IUIAutomationElement) -> Option<f64> {
        unsafe {
            let condition = property_condition_i32(
                uia,
                UIA_ControlTypePropertyId,
                UIA_ProgressBarControlTypeId.0,
            )
            .ok()?;
            let bars: IUIAutomationElementArray =
                panel.FindAll(TreeScope_Descendants, &condition).ok()?;
            let count = bars
                .Length()
                .ok()
                .unwrap_or(0)
                .min(MAX_PROGRESS_BARS as i32);
            let mut weekly_named: Option<f64> = None;
            let mut any: Option<f64> = None;
            for index in 0..count {
                let Ok(bar) = bars.GetElement(index) else {
                    continue;
                };
                let name = element_name(&bar);
                let from_pattern = read_range_value(&bar);
                let value = from_pattern.or_else(|| parse_percent_from_name(&name));
                if let Some(value) = value {
                    let weekly = name.to_lowercase().contains("weekly");
                    if weekly && weekly_named.is_none() {
                        weekly_named = Some(value);
                    }
                    if any.is_none() {
                        any = Some(value);
                    }
                }
            }
            weekly_named.or(any)
        }
    }

    fn read_range_value(element: &IUIAutomationElement) -> Option<f64> {
        unsafe {
            let pattern = element
                .GetCurrentPatternAs::<IUIAutomationRangeValuePattern>(UIA_RangeValuePatternId)
                .ok()?;
            let value = pattern.CurrentValue().ok()?;
            let minimum = pattern.CurrentMinimum().unwrap_or(0.0);
            let maximum = pattern.CurrentMaximum().unwrap_or(100.0);
            // A degenerate exposed range (max <= min) means the pattern is
            // present but unpopulated; the raw value is validated as-is.
            if maximum <= minimum {
                return sanitize_used_percent(value, 0.0, 100.0);
            }
            sanitize_used_percent(value, minimum, maximum)
        }
    }

    /// Finds the reset countdown line among the panel's text elements and
    /// returns it verbatim (sanitized). No absolute timestamp is derived.
    fn read_reset_text(uia: &IUIAutomation, panel: &IUIAutomationElement) -> Option<String> {
        unsafe {
            let condition =
                property_condition_i32(uia, UIA_ControlTypePropertyId, UIA_TextControlTypeId.0)
                    .ok()?;
            let texts: IUIAutomationElementArray =
                panel.FindAll(TreeScope_Descendants, &condition).ok()?;
            let count = texts
                .Length()
                .ok()
                .unwrap_or(0)
                .min(MAX_TEXT_ELEMENTS as i32);
            for index in 0..count {
                let Ok(text) = texts.GetElement(index) else {
                    continue;
                };
                let name = element_name(&text);
                if let Some(reset) = sanitize_reset_text(&name) {
                    return Some(reset);
                }
            }
            None
        }
    }

    fn file_version(path: &str) -> Option<String> {
        unsafe {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let size = GetFileVersionInfoSizeExW(
                FILE_VER_GET_NEUTRAL,
                PCWSTR(wide.as_ptr()),
                std::ptr::null_mut(),
            );
            if size == 0 || size > (1 << 20) {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            GetFileVersionInfoExW(
                FILE_VER_GET_NEUTRAL,
                PCWSTR(wide.as_ptr()),
                None,
                size as u32,
                data.as_mut_ptr() as *mut core::ffi::c_void,
            )
            .ok()?;
            let sub_block: Vec<u16> = "\\".encode_utf16().chain(std::iter::once(0)).collect();
            let mut buffer: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut length = 0u32;
            if !VerQueryValueW(
                data.as_ptr() as *const core::ffi::c_void,
                PCWSTR(sub_block.as_ptr()),
                &mut buffer,
                &mut length,
            )
            .as_bool()
                || length < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
            {
                return None;
            }
            let info = &*(buffer as *const VS_FIXEDFILEINFO);
            let version = format!(
                "{}.{}.{}.{}",
                info.dwFileVersionMS >> 16,
                info.dwFileVersionMS & 0xffff,
                info.dwFileVersionLS >> 16,
                info.dwFileVersionLS & 0xffff
            );
            sanitize_app_version(&version)
        }
    }

    fn read_usage_panel(
        uia: &IUIAutomation,
        panel: &IUIAutomationElement,
        exe_path: &str,
    ) -> ScanOutcome {
        let used_percent = read_weekly_used_percent(uia, panel);
        let reset_text = read_reset_text(uia, panel);
        if used_percent.is_none() && reset_text.is_none() {
            // The screen is there but nothing readable surfaced — do not
            // guess, and do not call it "not visible" either.
            return ScanOutcome::unknown();
        }
        ScanOutcome::ok(GrokBotSnapshot {
            used_percent,
            reset_text,
            app_version: file_version(exe_path),
            observed_at: wire_stamp(Utc::now()),
        })
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> &'static str {
        "2026-10-08T14:32:00Z"
    }

    /// Live integration probe — deliberately ignored in the ordinary suite.
    /// Run with `cargo test grok_bot_live_probe -- --ignored --nocapture`
    /// while Grok Bot is (ideally) running with Settings → Usage & Billing
    /// open; it asserts only that the real platform scan returns a typed
    /// status inside its watchdog budget, never specific usage values.
    #[cfg(windows)]
    #[test]
    #[ignore = "manual live probe: needs the real desktop session state"]
    fn grok_bot_live_probe_reports_a_typed_status_within_the_watchdog() {
        let started = std::time::Instant::now();
        let outcome = native::scan();
        let elapsed = started.elapsed();
        println!("live probe status: {:?} in {elapsed:?}", outcome.status);
        assert!(
            elapsed < std::time::Duration::from_millis(SCAN_TIMEOUT_MS + 1_000),
            "scan exceeded its watchdog budget: {elapsed:?}"
        );
        // A live answer must be one of the typed states — never a panic,
        // never an error envelope.
        assert!(matches!(
            outcome.status,
            GrokBotStatus::Ok
                | GrokBotStatus::NotRunning
                | GrokBotStatus::ScreenNotVisible
                | GrokBotStatus::Unknown
        ));
    }

    // -- exact percentage extraction and conversion --

    #[test]
    fn percent_passes_through_a_unit_range_unchanged() {
        assert_eq!(sanitize_used_percent(73.0, 0.0, 100.0), Some(73.0));
        assert_eq!(sanitize_used_percent(0.0, 0.0, 100.0), Some(0.0));
        assert_eq!(sanitize_used_percent(100.0, 0.0, 100.0), Some(100.0));
    }

    #[test]
    fn percent_scales_by_the_patterns_own_range() {
        assert_eq!(sanitize_used_percent(0.36, 0.0, 0.4), Some(90.0));
        assert_eq!(sanitize_used_percent(36.0, 0.0, 40.0), Some(90.0));
    }

    #[test]
    fn malformed_or_out_of_range_percent_is_dropped_not_clamped() {
        assert_eq!(sanitize_used_percent(f64::NAN, 0.0, 100.0), None);
        assert_eq!(sanitize_used_percent(f64::INFINITY, 0.0, 100.0), None);
        assert_eq!(sanitize_used_percent(-5.0, 0.0, 100.0), None);
        assert_eq!(sanitize_used_percent(140.0, 0.0, 100.0), None);
        // Unreadable range falls back to the raw value, still validated.
        assert_eq!(sanitize_used_percent(50.0, 100.0, 100.0), Some(50.0));
        assert_eq!(sanitize_used_percent(101.0, 100.0, 100.0), None);
    }

    // -- accessible-name fallback --

    #[test]
    fn name_fallback_accepts_exactly_one_percentage_token() {
        assert_eq!(parse_percent_from_name("73%"), Some(73.0));
        assert_eq!(parse_percent_from_name("Weekly usage 41.5%"), Some(41.5));
        assert_eq!(parse_percent_from_name("41 %"), Some(41.0));
    }

    #[test]
    fn name_fallback_rejects_ambiguous_or_absent_percentages() {
        // Two percentage tokens: never guess which one is the quota.
        assert_eq!(parse_percent_from_name("73% used, 50% cap"), None);
        assert_eq!(parse_percent_from_name("Weekly credits"), None);
        assert_eq!(parse_percent_from_name("plan 5 resets"), None);
        assert_eq!(parse_percent_from_name("150%"), None);
    }

    // -- reset text: verbatim, never a fabricated timestamp --

    #[test]
    fn reset_text_is_returned_verbatim() {
        assert_eq!(
            sanitize_reset_text("Resets in 3 days"),
            Some("Resets in 3 days".to_string())
        );
        assert_eq!(
            sanitize_reset_text("  Reset in 8 hours  "),
            Some("Reset in 8 hours".to_string())
        );
    }

    #[test]
    fn reset_text_rejects_unrelated_accessibility_text() {
        assert_eq!(sanitize_reset_text("Usage & Billing"), None);
        assert_eq!(sanitize_reset_text("Weekly usage"), None);
        assert_eq!(sanitize_reset_text(""), None);
        assert_eq!(sanitize_reset_text("   "), None);
    }

    #[test]
    fn reset_text_strips_control_characters_and_caps_length() {
        assert_eq!(
            sanitize_reset_text("Resets in 3\u{200b} days"),
            Some("Resets in 3 days".to_string())
        );
        assert_eq!(
            sanitize_reset_text("Resets in 3\u{7} days"),
            Some("Resets in 3 days".to_string())
        );
        let long = format!("Resets in {}", "a".repeat(200));
        assert_eq!(sanitize_reset_text(&long).map(|text| text.len()), Some(80));
    }

    #[test]
    fn app_version_keeps_only_version_shaped_characters() {
        assert_eq!(
            sanitize_app_version("0.68.1.0"),
            Some("0.68.1.0".to_string())
        );
        assert_eq!(sanitize_app_version("v0.68-beta"), Some("0.68".to_string()));
        assert_eq!(sanitize_app_version("unknown"), None);
        assert_eq!(sanitize_app_version(""), None);
    }

    // -- retention: the last successful reading survives failures --

    fn reading(used: f64, at: &str) -> ScanOutcome {
        ScanOutcome::ok(GrokBotSnapshot {
            used_percent: Some(used),
            reset_text: Some("Resets in 3 days".to_string()),
            app_version: Some("0.68.1.0".to_string()),
            observed_at: at.to_string(),
        })
    }

    #[test]
    fn a_successful_reading_replaces_the_cache() {
        let mut cache = None;
        let retained = retain_reading(&mut cache, &reading(73.0, stamp()));
        assert!(retained.is_some());
        assert_eq!(cache.as_ref().unwrap().used_percent, Some(73.0));
        let _ = retain_reading(&mut cache, &reading(41.0, stamp()));
        assert_eq!(cache.as_ref().unwrap().used_percent, Some(41.0));
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_known_reading_and_its_stamp() {
        let mut cache = None;
        let _ = retain_reading(&mut cache, &reading(73.0, stamp()));
        let later = "2026-10-08T16:32:00Z";
        let retained = retain_reading(&mut cache, &ScanOutcome::screen_not_visible());
        // Same reading, same original observation time — not re-stamped.
        assert_eq!(retained.as_ref().unwrap().observed_at, stamp());
        assert_eq!(retained.as_ref().unwrap().used_percent, Some(73.0));
        assert!(later > stamp());
    }

    #[test]
    fn not_running_also_preserves_the_last_known_reading() {
        let mut cache = None;
        let _ = retain_reading(&mut cache, &reading(73.0, stamp()));
        let retained = retain_reading(&mut cache, &ScanOutcome::not_running());
        assert!(retained.is_some());
        assert_eq!(cache.as_ref().unwrap().used_percent, Some(73.0));
    }

    #[test]
    fn unknown_without_prior_data_returns_none() {
        let mut cache = None;
        let retained = retain_reading(&mut cache, &ScanOutcome::unknown());
        assert!(retained.is_none());
    }

    // -- wire contract --

    #[test]
    fn wire_uses_camel_case_and_snake_case_status() {
        let response = GrokBotRefresh {
            status: GrokBotStatus::ScreenNotVisible,
            observed_at: stamp().to_string(),
            used_percent: None,
            reset_text: None,
            app_version: None,
            last_known: Some(GrokBotSnapshot {
                used_percent: Some(73.0),
                reset_text: Some("Resets in 3 days".to_string()),
                app_version: Some("0.68.1.0".to_string()),
                observed_at: stamp().to_string(),
            }),
        };
        let wire = serde_json::to_string(&response).unwrap();
        assert!(wire.contains("\"status\":\"screen_not_visible\""));
        assert!(wire.contains("\"observedAt\":\"2026-10-08T14:32:00Z\""));
        assert!(wire.contains("\"usedPercent\":73.0"));
        assert!(wire.contains("\"resetText\":\"Resets in 3 days\""));
        assert!(wire.contains("\"appVersion\":\"0.68.1.0\""));
        assert!(wire.contains("\"lastKnown\":{"));
        // Unavailable values are omitted, not zeroed out.
        let ok_wire = serde_json::to_string(&reading(73.0, stamp()).snapshot).unwrap();
        assert!(!ok_wire.contains("resetAt"));
        let empty = serde_json::to_string(&GrokBotRefresh {
            status: GrokBotStatus::NotRunning,
            observed_at: stamp().to_string(),
            used_percent: None,
            reset_text: None,
            app_version: None,
            last_known: None,
        })
        .unwrap();
        assert!(!empty.contains("usedPercent"));
        assert!(!empty.contains("resetText"));
        assert!(!empty.contains("appVersion"));
        assert!(!empty.contains("lastKnown"));
    }

    #[test]
    fn snapshot_never_carries_a_derived_absolute_reset_time() {
        // The reset stays the source's own countdown text; nothing converts
        // "Resets in 3 days" into a guessed absolute timestamp.
        let snapshot = reading(73.0, stamp()).snapshot.clone().unwrap();
        let wire = serde_json::to_string(&snapshot).unwrap();
        assert!(wire.contains("resetText"));
        assert!(!wire.contains("resetAt"));
        assert!(!wire.contains("reset_at"));
    }

    // -- read-only and offline, pinned against the module text --

    #[test]
    fn module_stays_read_only_offline_and_manual() {
        // Assembled from fragments so this test's own text cannot satisfy it.
        let forbidden = [
            ["Set", "Focus"].concat(),
            ["Send", "Input"].concat(),
            ["keybd", "_event"].concat(),
            ["mouse", "_event"].concat(),
            ["Invoke", "Pattern"].concat(),
            ["Set", "Value"].concat(),
            ["Window", "Pattern_Close"].concat(),
            ["Post", "Message"].concat(),
            ["Send", "Message"].concat(),
            ["req", "west"].concat(),
            ["http", "s://"].concat(),
            ["api2", ".cursor.sh"].concat(),
            ["Set", "WindowPos"].concat(),
            ["Show", "Window"].concat(),
        ];
        let text = include_str!("grok_bot.rs");
        for symbol in forbidden {
            assert!(
                !text.contains(&symbol),
                "grok_bot.rs must stay read-only and offline; found {symbol}"
            );
        }
    }
}
