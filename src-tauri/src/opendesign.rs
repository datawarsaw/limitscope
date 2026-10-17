//! OpenDesign Cloud usage — manual, read-only Windows UI Automation read.
//!
//! OpenDesign Cloud has no public usage API, and its CLI exposes no quota
//! windows, so the only source of the Go plan's 5-hour and 7-day allowance is
//! the signed-in Plan & balance screen in Chrome. This module reads exactly
//! that screen through the standard Windows accessibility tree.
//!
//! Contract — manual and read-only, by design:
//! - the ONLY entry point is the refresh_opendesign_usage command, invoked
//!   exclusively by an explicit user action in the OpenDesign detail card.
//!   There is no scheduler, no timer, and no hook into the shared runtime
//!   cycle: this module never runs unless the user asked for it, because the
//!   network refresh cycle must never drive UI automation;
//! - enumeration is scoped strictly to processes whose image name is
//!   chrome.exe and to that process's own top-level windows. No other
//!   application is ever touched;
//! - the read uses only Windows accessibility interfaces. It never launches,
//!   navigates, refreshes the page, clicks, types into, or restarts Chrome; no input
//!   synthesis, no pattern that mutates page state, no window manager
//!   commands. A minimized or hidden window is still readable, and the
//!   module never changes that state;
//! - the page is located through its own accessibility root: the document
//!   whose accessible name is "Open Design Cloud" and whose URL is on
//!   open-design.ai under the /cloud/ console. Cookies, tokens, session
//!   storage, authentication headers, and every other browser file are never
//!   read; this module makes no network requests;
//! - the two quota windows are located by their semantic group names
//!   ("5-hour limit", "7-day limit"). Generated React AutomationIds
//!   (_r_p_-panel-*), process ids and window handles are never anchors;
//! - each window must contain exactly one progress bar and at most one
//!   "Resets in …" line. Anything missing or duplicated makes the whole
//!   attempt unknown — the parser never guesses, never substitutes one
//!   window for the other, and never reports a partial pair;
//! - returned percentages are the values the page states as REMAINING, kept
//!   as their own field. They are never inverted into a used percentage, and
//!   the separate pay-as-you-go wallet is never mixed in;
//! - reset information stays the page's own countdown text, verbatim. No
//!   absolute reset timestamp is derived from a relative string;
//! - the scan runs on a dedicated background thread with a hard watchdog
//!   (SCAN_TIMEOUT_MS), never on the UI thread, and the async runtime is
//!   never blocked by it. The watchdog returns without waiting, but the
//!   single-flight slot stays taken until that worker actually finishes, so
//!   a later refresh cannot start a second accessibility walk;
//! - the last successful reading is retained in memory (process-local, never
//!   persisted) and returned alongside a failed attempt, so the UI can show
//!   it clearly marked as last known. A failed attempt never overwrites it.
//!
//! The user keeps ownership of navigation: Chrome must already be running
//! with Plan & balance open and signed in for a read to succeed. When it is
//! not, the command answers chrome_not_running / dashboard_not_visible
//! instead of reaching for the browser.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

/// RFC 3339 UTC, second precision — the repo's wire convention for stamps.
fn wire_stamp(moment: DateTime<Utc>) -> String {
    moment.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Hard upper bound for one accessibility scan, wall-clock. A healthy read
/// takes a few tens of milliseconds; anything past this is treated as a
/// failed attempt (unknown) rather than blocking the caller.
#[cfg(windows)]
const SCAN_TIMEOUT_MS: u64 = 2_500;

/// The browser is identified by image name only. A match grants nothing on
/// its own: every read still requires the OpenDesign document root below.
const CHROME_EXE_NAME: &str = "chrome.exe";

/// The accessible name of the console document, as Chrome exposes it.
const DASHBOARD_DOCUMENT_NAME: &str = "Open Design Cloud";
/// Host and console path prefix the document URL must match. The workspace
/// id is deliberately not pinned: any signed-in workspace is the user's own.
const DASHBOARD_HOST: &str = "open-design.ai";
const DASHBOARD_PATH_PREFIX: &str = "/cloud/";

/// The two quota windows, by their semantic group names. These are the only
/// accepted anchors; the surrounding numbers are read inside each group.
const FIVE_HOUR_GROUP: &str = "5-hour limit";
const SEVEN_DAY_GROUP: &str = "7-day limit";

/// The plan label the page states for the Go allowance.
const PLAN_LABEL: &str = "Go";

/// At most this many of Chrome's windows are probed for the document.
#[cfg(windows)]
const MAX_WINDOWS: usize = 4;
/// At most this many text nodes are inspected inside one quota group.
const MAX_GROUP_TEXTS: usize = 32;
/// Text longer than this is not a quota label and is discarded unread.
const MAX_TEXT_LEN: usize = 200;

// ---------- wire contract (camelCase on the wire, status snake_case) ----------

/// Outcome of one observation attempt. ok means both quota windows were
/// identified unambiguously and each carried exactly one percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenDesignStatus {
    Ok,
    ChromeNotRunning,
    DashboardNotVisible,
    Unknown,
}

/// Which quota window a reading belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OpenDesignWindowLabel {
    #[serde(rename = "5-hour")]
    FiveHour,
    #[serde(rename = "7-day")]
    SevenDay,
}

/// One quota window as the page states it. remaining_percent is exactly
/// that: the remaining allowance, never a used percentage. reset_text is
/// the page's own countdown line, verbatim.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDesignWindowReading {
    pub label: OpenDesignWindowLabel,
    pub remaining_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_text: Option<String>,
}

/// One successful reading: both windows, the plan label, the period line the
/// page shows, and when the scan actually ran.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDesignSnapshot {
    pub windows: Vec<OpenDesignWindowReading>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_ends_text: Option<String>,
    pub observed_at: String,
}

/// The command's answer: the fresh attempt (top-level fields) plus, when one
/// exists, the retained last successful reading. The command never returns an
/// error envelope — every failure mode is a typed status.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDesignRefresh {
    pub status: OpenDesignStatus,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<OpenDesignWindowReading>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_ends_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_known: Option<OpenDesignSnapshot>,
}

/// What the platform scan produced, before retention is applied.
#[derive(Debug, Clone, PartialEq)]
struct ScanOutcome {
    status: OpenDesignStatus,
    snapshot: Option<OpenDesignSnapshot>,
}

impl ScanOutcome {
    fn chrome_not_running() -> Self {
        Self {
            status: OpenDesignStatus::ChromeNotRunning,
            snapshot: None,
        }
    }

    fn dashboard_not_visible() -> Self {
        Self {
            status: OpenDesignStatus::DashboardNotVisible,
            snapshot: None,
        }
    }

    fn unknown() -> Self {
        Self {
            status: OpenDesignStatus::Unknown,
            snapshot: None,
        }
    }

    fn ok(snapshot: OpenDesignSnapshot) -> Self {
        Self {
            status: OpenDesignStatus::Ok,
            snapshot: Some(snapshot),
        }
    }
}

// ---------- sanitization (pure, hermetically testable) ----------

/// Normalizes a raw pattern value into a remaining percentage and validates
/// it: scaled by the pattern's own minimum/maximum when the range is
/// meaningful, then required to be finite and within 0–100. Anything else is
/// discarded — missing data stays missing, never zero.
pub fn sanitize_remaining_percent(value: f64, minimum: f64, maximum: f64) -> Option<f64> {
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
/// misread as a quota. The page labels the remaining share, so the token is
/// taken as remaining only when the name also reads "left".
pub fn parse_remaining_from_name(name: &str) -> Option<f64> {
    let normalized = normalized_label(name);
    if !normalized.contains("left") {
        return None;
    }
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
        sanitize_remaining_percent(matches[0], 0.0, 100.0)
    } else {
        None
    }
}

fn normalized_label(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn strip_control(text: &str) -> String {
    text.chars()
        // Control characters and invisible format marks (zero-width spaces,
        // BOM) never belong in a displayed line; everything else is kept.
        .filter(|c| !c.is_control() && !matches!(*c as u32, 0x200b..=0x200f | 0xfeff))
        .take(MAX_TEXT_LEN)
        .collect()
}

/// Accepts a reset countdown only in the source shape — a "Resets in …" /
/// "Reset in …" prefix — and returns it trimmed, with control characters
/// stripped, capped at a sane length. The text stays verbatim: a relative
/// countdown is never turned into an absolute timestamp.
pub fn sanitize_reset_text(text: &str) -> Option<String> {
    // Invisible marks are stripped before the shape check, so a zero-width
    // or bidi mark between the words cannot hide an otherwise exact line.
    let cleaned = strip_control(text).trim().to_string();
    let lower = cleaned.to_lowercase();
    if !(lower.starts_with("resets in") || lower.starts_with("reset in")) {
        return None;
    }
    let cleaned: String = cleaned.chars().take(80).collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Accepts the plan name only in the verified shape: the single token the
/// page shows for this allowance. Anything else is dropped rather than shown.
pub fn sanitize_plan(text: &str) -> Option<String> {
    let cleaned = strip_control(text).trim().to_string();
    if cleaned == PLAN_LABEL {
        Some(cleaned)
    } else {
        None
    }
}

/// Accepts the subscription period line the page shows in the plan block,
/// e.g. "Current period ends Nov 15, 2026". It is display text, not a date
/// to compute from, so it is kept verbatim and only shape-checked.
pub fn sanitize_period_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.to_lowercase().starts_with("current period ends") {
        return None;
    }
    let cleaned: String = strip_control(trimmed).chars().take(80).collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Validates the document URL: the console host and path only. Query values
/// (the workspace id) are not inspected, returned, or logged.
pub fn is_dashboard_url(url: &str) -> bool {
    let trimmed = url.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let without_fragment = without_scheme.split('#').next().unwrap_or("");
    let without_query = without_fragment.split('?').next().unwrap_or("");
    let (host, path) = match without_query.split_once('/') {
        Some((host, rest)) => (host, format!("/{rest}")),
        None => (without_query, "/".to_string()),
    };
    let host = host.to_ascii_lowercase();
    host == DASHBOARD_HOST && path.to_ascii_lowercase().starts_with(DASHBOARD_PATH_PREFIX)
}

// ---------- retention (last-known for the reading) ----------

/// The last successful reading, retained so a failed refresh cannot erase it
/// from the UI. In-memory only, process-local, never persisted, and there is
/// deliberately no TTL: manual refresh means the cadence is the user's, and
/// the panel always shows the reading's own observation stamp.
static LAST_KNOWN: OnceLock<Mutex<Option<OpenDesignSnapshot>>> = OnceLock::new();

fn last_known() -> &'static Mutex<Option<OpenDesignSnapshot>> {
    LAST_KNOWN.get_or_init(|| Mutex::new(None))
}

/// Retention core (pure, testable): a complete successful pair replaces the
/// cache. A failed attempt, and a fragment without both windows, leaves the
/// cache untouched.
fn retain_reading(
    cache: &mut Option<OpenDesignSnapshot>,
    outcome: &ScanOutcome,
) -> Option<OpenDesignSnapshot> {
    if outcome.status == OpenDesignStatus::Ok {
        if let Some(snapshot) = outcome.snapshot.clone() {
            if snapshot.windows.len() == 2 {
                *cache = Some(snapshot);
            }
        }
    }
    cache.clone()
}

// ---------- pure parsing of one quota group ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupNodeKind {
    Progress,
    Text,
}

/// One accessibility node inside a quota group, in tree order.
#[derive(Debug, Clone, PartialEq)]
struct GroupNode {
    kind: GroupNodeKind,
    name: String,
    /// Sanitized remaining percentage for a progress bar.
    percent: Option<f64>,
}

/// One quota group as a whole. None means the group was missing, held no
/// or several bars, or carried an ambiguous reset line: a group is either
/// read exactly or not reported at all.
#[derive(Debug, Clone, PartialEq)]
struct WindowReading {
    label: OpenDesignWindowLabel,
    remaining_percent: f64,
    reset_text: Option<String>,
}

/// Reads one quota group. Exactly one progress bar must be present; its
/// percentage comes from the pattern, then from a single "…% left" text.
/// A reset line is accepted only when exactly one such line exists in the
/// group; zero lines leave it absent, several make the group ambiguous.
fn select_window_reading(
    label: OpenDesignWindowLabel,
    nodes: &[GroupNode],
) -> Option<WindowReading> {
    let bars: Vec<&GroupNode> = nodes
        .iter()
        .filter(|node| node.kind == GroupNodeKind::Progress)
        .collect();
    if bars.len() != 1 {
        return None;
    }
    let bar = bars[0];
    let caption = nodes
        .iter()
        .filter(|node| node.kind == GroupNodeKind::Text)
        .find_map(|node| parse_remaining_from_name(&node.name));
    if let (Some(bar_percent), Some(caption_percent)) = (bar.percent, caption) {
        // The caption is the same share rounded to a whole percent, so the
        // two may differ by rounding only. Anything wider means the bar and
        // the caption are not describing one number, and neither is shown.
        if (bar_percent - caption_percent).abs() > 1.0 {
            return None;
        }
    }
    let remaining_percent = bar.percent.or(caption)?;

    let mut resets: Vec<String> = nodes
        .iter()
        .filter(|node| node.kind == GroupNodeKind::Text)
        .filter_map(|node| sanitize_reset_text(&node.name))
        .collect();
    if resets.len() > 1 {
        return None;
    }
    Some(WindowReading {
        label,
        remaining_percent,
        reset_text: resets.pop(),
    })
}

/// Reads the plan block: the plan label and the period line, each only when
/// it appears exactly once with the expected shape.
fn select_plan(nodes: &[GroupNode]) -> (Option<String>, Option<String>) {
    let plans: Vec<String> = nodes
        .iter()
        .filter(|node| node.kind == GroupNodeKind::Text)
        .filter_map(|node| sanitize_plan(&node.name))
        .collect();
    let periods: Vec<String> = nodes
        .iter()
        .filter(|node| node.kind == GroupNodeKind::Text)
        .filter_map(|node| sanitize_period_text(&node.name))
        .collect();
    (
        if plans.len() == 1 {
            plans.into_iter().next()
        } else {
            None
        },
        if periods.len() == 1 {
            periods.into_iter().next()
        } else {
            None
        },
    )
}

/// Combines both windows into one snapshot. Both must be present, and each
/// must carry its own label — the pair is never completed by substitution.
fn combine_windows(
    five_hour: &[GroupNode],
    seven_day: &[GroupNode],
    plan: Option<String>,
    period_ends_text: Option<String>,
) -> Option<OpenDesignSnapshot> {
    let five = select_window_reading(OpenDesignWindowLabel::FiveHour, five_hour)?;
    let seven = select_window_reading(OpenDesignWindowLabel::SevenDay, seven_day)?;
    if five.label == seven.label {
        return None;
    }
    Some(OpenDesignSnapshot {
        windows: vec![
            OpenDesignWindowReading {
                label: five.label,
                remaining_percent: five.remaining_percent,
                reset_text: five.reset_text,
            },
            OpenDesignWindowReading {
                label: seven.label,
                remaining_percent: seven.remaining_percent,
                reset_text: seven.reset_text,
            },
        ],
        plan,
        period_ends_text,
        observed_at: wire_stamp(Utc::now()),
    })
}

/// How far one Chrome window got before the dashboard was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowTree {
    /// ElementFromHandle failed, so this window produced no accessibility tree.
    Unavailable,
    /// A tree was obtained without a matching OpenDesign document.
    DocumentAbsent,
    /// The OpenDesign document is present but its two quota groups were not.
    QuotaAbsent,
}

/// Classifies a scan that did not produce a reading.
///
/// No windows means Chrome is absent. A tree without the OpenDesign document
/// means Chrome is running but the dashboard is not showing. Failure to
/// obtain any tree is unknown, not a missing screen.
fn classify_window_trees(trees: &[WindowTree]) -> ScanOutcome {
    if trees.is_empty() {
        return ScanOutcome::chrome_not_running();
    }
    if trees
        .iter()
        .any(|tree| matches!(tree, WindowTree::DocumentAbsent | WindowTree::QuotaAbsent))
    {
        return ScanOutcome::dashboard_not_visible();
    }
    ScanOutcome::unknown()
}

fn scan_slot() -> &'static Arc<AtomicBool> {
    static SLOT: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    SLOT.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

struct SlotRelease(Arc<AtomicBool>);

impl Drop for SlotRelease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Runs body on one dedicated thread. A second call while that thread is
/// still inside body does not start another one, including after this
/// caller's timeout. The slot is released only when the worker returns.
fn run_single_flight<F>(slot: &Arc<AtomicBool>, timeout: Duration, body: F) -> ScanOutcome
where
    F: FnOnce() -> ScanOutcome + Send + 'static,
{
    if slot
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ScanOutcome::unknown();
    }
    let (tx, rx) = mpsc::channel();
    let slot_for_worker = Arc::clone(slot);
    let spawned = thread::Builder::new()
        .name("opendesign-uia-read".to_string())
        .spawn(move || {
            let _release = SlotRelease(slot_for_worker);
            let outcome =
                catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|_| ScanOutcome::unknown());
            let _ = tx.send(outcome);
        });
    if spawned.is_err() {
        slot.store(false, Ordering::Release);
        return ScanOutcome::unknown();
    }
    match rx.recv_timeout(timeout) {
        Ok(outcome) => outcome,
        Err(_) => ScanOutcome::unknown(),
    }
}

// ---------- the one command: explicit, user-triggered only ----------

/// Reads OpenDesign Cloud's Plan & balance values once, through Windows UI
/// Automation, and returns the attempt plus the retained last-known reading.
/// Callers must invoke this only on explicit user action — nothing in this
/// module ever schedules itself.
#[tauri::command]
pub async fn refresh_opendesign_usage() -> OpenDesignRefresh {
    let observed_at = wire_stamp(Utc::now());
    let outcome = tauri::async_runtime::spawn_blocking(perform_scan)
        .await
        .unwrap_or_else(|_| ScanOutcome::unknown());

    let snapshot = outcome.snapshot.clone();
    let last_known = {
        let mut cache = last_known().lock().unwrap();
        retain_reading(&mut cache, &outcome)
    };

    match snapshot {
        Some(snapshot) => OpenDesignRefresh {
            status: outcome.status,
            observed_at,
            windows: Some(snapshot.windows),
            plan: snapshot.plan,
            period_ends_text: snapshot.period_ends_text,
            last_known,
        },
        None => OpenDesignRefresh {
            status: outcome.status,
            observed_at,
            windows: None,
            plan: None,
            period_ends_text: None,
            last_known,
        },
    }
}

/// Dispatches to the platform reader. Off-Windows builds answer unknown:
/// the read is a Windows accessibility contract with no fallback source.
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
    use std::time::Duration;

    use super::{
        classify_window_trees, combine_windows, is_dashboard_url, normalized_label,
        run_single_flight, sanitize_remaining_percent, scan_slot, select_plan, wire_stamp,
        GroupNode, GroupNodeKind, OpenDesignSnapshot, ScanOutcome, WindowTree, CHROME_EXE_NAME,
        DASHBOARD_DOCUMENT_NAME, FIVE_HOUR_GROUP, MAX_GROUP_TEXTS, MAX_WINDOWS, SCAN_TIMEOUT_MS,
        SEVEN_DAY_GROUP,
    };
    use chrono::Utc;
    use windows::core::{BSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
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
        IUIAutomationRangeValuePattern, IUIAutomationValuePattern, TreeScope_Descendants,
        UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_GroupControlTypeId,
        UIA_NamePropertyId, UIA_ProgressBarControlTypeId, UIA_RangeValuePatternId,
        UIA_TextControlTypeId, UIA_ValuePatternId, UIA_PROPERTY_ID,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId,
    };

    /// The scan runs on its own short-lived thread so COM apartment state,
    /// UIA handles, and any stall stay contained. The caller waits under the
    /// hard watchdog. A timeout answers unknown without releasing the slot,
    /// so the worker remains the only scan in flight until it returns.
    pub(super) fn scan() -> ScanOutcome {
        run_single_flight(
            scan_slot(),
            Duration::from_millis(SCAN_TIMEOUT_MS),
            run_scan,
        )
    }

    fn run_scan() -> ScanOutcome {
        unsafe {
            // Microsoft's UI Automation threading contract: a client thread
            // that owns no windows initializes COM as an MTA
            // (COINIT_MULTITHREADED). This worker does not pump messages, so
            // an STA would be the wrong apartment for the platform contract.
            let com = CoInitializeEx(None, COINIT_MULTITHREADED);
            if com.is_err() {
                return ScanOutcome::unknown();
            }
            let outcome = read_opendesign();
            CoUninitialize();
            outcome
        }
    }

    fn read_opendesign() -> ScanOutcome {
        unsafe {
            let windows = find_chrome_windows();
            if windows.is_empty() {
                return classify_window_trees(&[]);
            }

            let uia: IUIAutomation =
                match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                    Ok(uia) => uia,
                    Err(_) => return ScanOutcome::unknown(),
                };

            let mut trees = Vec::new();
            for hwnd in windows.iter().take(MAX_WINDOWS) {
                let root = match uia.ElementFromHandle(*hwnd) {
                    Ok(root) => root,
                    Err(_) => {
                        trees.push(WindowTree::Unavailable);
                        continue;
                    }
                };
                let Some(document) = find_dashboard_document(&uia, &root) else {
                    trees.push(WindowTree::DocumentAbsent);
                    continue;
                };
                match read_document(&uia, &document) {
                    Some(snapshot) => return ScanOutcome::ok(snapshot),
                    None => trees.push(WindowTree::QuotaAbsent),
                }
            }
            classify_window_trees(&trees)
        }
    }

    // ----- window discovery: strictly Chrome's own processes -----

    struct WindowProbe {
        // Top-level windows in enumeration order. A hidden or minimized
        // window is kept: the accessibility tree of a loaded dashboard stays
        // readable, and this module never changes that state.
        windows: Vec<HWND>,
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        if GetWindowTextLengthW(hwnd) == 0 {
            return windows::core::BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        if pid == 0 {
            return windows::core::BOOL(1);
        }
        if let Some(exe_name) = process_image_name(pid) {
            if exe_name.eq_ignore_ascii_case(CHROME_EXE_NAME) {
                let probe = &mut *(lparam.0 as *mut WindowProbe);
                probe.windows.push(hwnd);
            }
        }
        windows::core::BOOL(1)
    }

    /// The process image name only (no path). The path is never stored,
    /// returned, or logged.
    fn process_image_name(pid: u32) -> Option<String> {
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
            let path = String::from_utf16_lossy(&buffer[..length as usize]);
            Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
        }
    }

    fn find_chrome_windows() -> Vec<HWND> {
        let mut probe = WindowProbe {
            windows: Vec::new(),
        };
        unsafe {
            let _ = EnumWindows(
                Some(enum_windows_proc),
                LPARAM(&mut probe as *mut WindowProbe as isize),
            );
        }
        probe.windows
    }

    // ----- semantic anchors inside the page -----

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

    fn control_type_condition(
        uia: &IUIAutomation,
        control_type: i32,
    ) -> Option<IUIAutomationCondition> {
        property_condition_i32(uia, UIA_ControlTypePropertyId, control_type).ok()
    }

    /// Locates the OpenDesign console document by its accessible name, then
    /// validates the documented URL shape. Name alone is not enough: the
    /// page must be the cloud console on the expected host.
    fn find_dashboard_document(
        uia: &IUIAutomation,
        root: &IUIAutomationElement,
    ) -> Option<IUIAutomationElement> {
        unsafe {
            let document_type = control_type_condition(uia, UIA_DocumentControlTypeId.0)?;
            let name =
                property_condition_bstr(uia, UIA_NamePropertyId, DASHBOARD_DOCUMENT_NAME).ok()?;
            let condition = uia.CreateAndCondition(&document_type, &name).ok()?;
            let document = root.FindFirst(TreeScope_Descendants, &condition).ok()?;
            if is_dashboard_url(&document_url(&document).unwrap_or_default()) {
                Some(document)
            } else {
                None
            }
        }
    }

    fn document_url(document: &IUIAutomationElement) -> Option<String> {
        unsafe {
            let pattern = document
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .ok()?;
            pattern.CurrentValue().ok().map(|value| value.to_string())
        }
    }

    /// The named groups on the page: quota windows and the plan block. Only
    /// names this module recognises are collected; every other group is
    /// ignored unread.
    fn named_groups(
        uia: &IUIAutomation,
        document: &IUIAutomationElement,
    ) -> Vec<(String, IUIAutomationElement)> {
        unsafe {
            let Some(group_type) = control_type_condition(uia, UIA_GroupControlTypeId.0) else {
                return Vec::new();
            };
            let Ok(elements) = document.FindAll(TreeScope_Descendants, &group_type) else {
                return Vec::new();
            };
            let count = elements.Length().ok().unwrap_or(0).clamp(0, 512);
            let mut groups = Vec::new();
            for index in 0..count {
                let Ok(element) = elements.GetElement(index) else {
                    continue;
                };
                let name = element_name(&element);
                let normalized = normalized_label(&name);
                if normalized == FIVE_HOUR_GROUP
                    || normalized == SEVEN_DAY_GROUP
                    || normalized == "plan allowance"
                {
                    groups.push((normalized, element));
                }
            }
            groups
        }
    }

    fn exactly_one<'a>(
        groups: &'a [(String, IUIAutomationElement)],
        name: &str,
    ) -> Option<&'a IUIAutomationElement> {
        let mut matches = groups.iter().filter(|(group, _)| group == name);
        let first = matches.next()?;
        if matches.next().is_some() {
            // Two groups with the same name: which one is the quota window
            // is unknowable, so neither is used.
            return None;
        }
        Some(&first.1)
    }

    fn element_name(element: &IUIAutomationElement) -> String {
        unsafe { element.CurrentName().unwrap_or_default().to_string() }
    }

    /// Text and progress bars inside one group, in tree order.
    fn collect_group_nodes(uia: &IUIAutomation, group: &IUIAutomationElement) -> Vec<GroupNode> {
        unsafe {
            let Some(progress) = control_type_condition(uia, UIA_ProgressBarControlTypeId.0) else {
                return Vec::new();
            };
            let Some(text) = control_type_condition(uia, UIA_TextControlTypeId.0) else {
                return Vec::new();
            };
            let Ok(condition) = uia.CreateOrCondition(&progress, &text) else {
                return Vec::new();
            };
            let Ok(elements) = group.FindAll(TreeScope_Descendants, &condition) else {
                return Vec::new();
            };
            let count = elements
                .Length()
                .ok()
                .unwrap_or(0)
                .clamp(0, MAX_GROUP_TEXTS as i32);
            let mut nodes = Vec::new();
            for index in 0..count {
                let Ok(element) = elements.GetElement(index) else {
                    continue;
                };
                let Ok(control_type) = element.CurrentControlType() else {
                    continue;
                };
                let name = element_name(&element);
                if control_type == UIA_ProgressBarControlTypeId {
                    let percent = read_range_value(&element);
                    nodes.push(GroupNode {
                        kind: GroupNodeKind::Progress,
                        name,
                        percent,
                    });
                } else if control_type == UIA_TextControlTypeId {
                    nodes.push(GroupNode {
                        kind: GroupNodeKind::Text,
                        name,
                        percent: None,
                    });
                }
            }
            nodes
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
            if maximum <= minimum {
                return sanitize_remaining_percent(value, 0.0, 100.0);
            }
            sanitize_remaining_percent(value, minimum, maximum)
        }
    }

    fn read_document(
        uia: &IUIAutomation,
        document: &IUIAutomationElement,
    ) -> Option<OpenDesignSnapshot> {
        let groups = named_groups(uia, document);
        let five_group = exactly_one(&groups, FIVE_HOUR_GROUP)?;
        let seven_group = exactly_one(&groups, SEVEN_DAY_GROUP)?;
        let plan_group = exactly_one(&groups, "plan allowance");

        let five_nodes = collect_group_nodes(uia, five_group);
        let seven_nodes = collect_group_nodes(uia, seven_group);
        let (plan, period_ends_text) = match plan_group {
            Some(group) => {
                let nodes = collect_group_nodes(uia, group);
                select_plan(&nodes)
            }
            None => (None, None),
        };
        let snapshot = combine_windows(&five_nodes, &seven_nodes, plan, period_ends_text)?;
        Some(OpenDesignSnapshot {
            observed_at: wire_stamp(Utc::now()),
            ..snapshot
        })
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn stamp() -> &'static str {
        "2026-10-17T09:12:00Z"
    }

    fn text_node(name: &str) -> GroupNode {
        GroupNode {
            kind: GroupNodeKind::Text,
            name: name.to_string(),
            percent: None,
        }
    }

    fn bar_node(name: &str, percent: Option<f64>) -> GroupNode {
        GroupNode {
            kind: GroupNodeKind::Progress,
            name: name.to_string(),
            percent,
        }
    }

    /// The live shape of one quota group, in tree order, as observed on
    /// 2026-10-17: caption, heading, remaining text, bar, empty text, reset.
    fn live_group(heading: &str, remaining: &str, percent: f64, reset: &str) -> Vec<GroupNode> {
        vec![
            text_node("Design Plan · "),
            text_node(heading),
            text_node(remaining),
            bar_node(heading, Some(percent)),
            text_node(""),
            text_node(reset),
        ]
    }

    fn snapshot() -> OpenDesignSnapshot {
        combine_windows(
            &live_group(FIVE_HOUR_GROUP, "73% left", 73.21, "Resets in 39 min"),
            &live_group(SEVEN_DAY_GROUP, "91% left", 90.86, "Resets in 6 days 14 hr"),
            Some("Go".to_string()),
            Some("Current period ends Nov 15, 2026".to_string()),
        )
        .expect("both windows")
    }

    // -- percentages stay remaining percentages --

    #[test]
    fn remaining_percent_passes_through_a_unit_range_unchanged() {
        assert_eq!(sanitize_remaining_percent(73.21, 0.0, 100.0), Some(73.21));
        assert_eq!(sanitize_remaining_percent(0.0, 0.0, 100.0), Some(0.0));
        assert_eq!(sanitize_remaining_percent(100.0, 0.0, 100.0), Some(100.0));
    }

    #[test]
    fn remaining_percent_scales_by_the_patterns_own_range() {
        assert_eq!(sanitize_remaining_percent(73.2, 0.0, 100.0), Some(73.2));
        assert_eq!(sanitize_remaining_percent(0.732, 0.0, 1.0), Some(73.2));
    }

    #[test]
    fn out_of_range_or_non_finite_percentages_are_discarded() {
        assert_eq!(sanitize_remaining_percent(101.0, 0.0, 100.0), None);
        assert_eq!(sanitize_remaining_percent(-1.0, 0.0, 100.0), None);
        assert_eq!(sanitize_remaining_percent(f64::NAN, 0.0, 100.0), None);
        assert_eq!(sanitize_remaining_percent(f64::INFINITY, 0.0, 100.0), None);
    }

    #[test]
    fn a_left_caption_yields_a_single_remaining_percentage() {
        assert_eq!(parse_remaining_from_name("73% left"), Some(73.0));
        assert_eq!(parse_remaining_from_name("  91.4 % left "), Some(91.4));
        assert_eq!(parse_remaining_from_name("0% left"), Some(0.0));
        assert_eq!(parse_remaining_from_name("100% left"), Some(100.0));
    }

    #[test]
    fn a_caption_without_left_or_with_several_numbers_is_not_a_quota() {
        assert_eq!(parse_remaining_from_name("73%"), None);
        assert_eq!(parse_remaining_from_name("73% used"), None);
        assert_eq!(parse_remaining_from_name("5-hour limit"), None);
        assert_eq!(parse_remaining_from_name("73% left · 2% left"), None);
    }

    // -- reset text stays the page's own countdown --

    #[test]
    fn reset_text_is_kept_verbatim_and_never_becomes_a_timestamp() {
        assert_eq!(
            sanitize_reset_text("Resets in 39 min").as_deref(),
            Some("Resets in 39 min")
        );
        assert_eq!(
            sanitize_reset_text("  Resets in 6 days 14 hr  ").as_deref(),
            Some("Resets in 6 days 14 hr")
        );
        assert_eq!(
            sanitize_reset_text("Reset in 1 hour").as_deref(),
            Some("Reset in 1 hour")
        );
        assert_eq!(
            sanitize_reset_text("Current period ends Nov 15, 2026"),
            None
        );
        assert_eq!(sanitize_reset_text("No resets available"), None);
        assert_eq!(
            sanitize_reset_text("Resets in 39\u{200b} min"),
            Some("Resets in 39 min".to_string())
        );
    }

    #[test]
    fn plan_and_period_are_only_accepted_in_their_verified_shape() {
        assert_eq!(sanitize_plan("Go").as_deref(), Some("Go"));
        assert_eq!(sanitize_plan("Go\n").as_deref(), Some("Go"));
        assert_eq!(sanitize_plan("Free"), None);
        assert_eq!(sanitize_plan("Upgrade"), None);
        assert_eq!(
            sanitize_period_text("Current period ends Nov 15, 2026").as_deref(),
            Some("Current period ends Nov 15, 2026")
        );
        assert_eq!(sanitize_period_text("Nov 15, 2026"), None);
    }

    // -- window mapping: each percentage with its own label and reset --

    #[test]
    fn each_window_keeps_its_own_percentage_and_reset() {
        let snapshot = snapshot();
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].label, OpenDesignWindowLabel::FiveHour);
        assert_eq!(snapshot.windows[0].remaining_percent, 73.21);
        assert_eq!(
            snapshot.windows[0].reset_text.as_deref(),
            Some("Resets in 39 min")
        );
        assert_eq!(snapshot.windows[1].label, OpenDesignWindowLabel::SevenDay);
        assert_eq!(snapshot.windows[1].remaining_percent, 90.86);
        assert_eq!(
            snapshot.windows[1].reset_text.as_deref(),
            Some("Resets in 6 days 14 hr")
        );
        assert_eq!(snapshot.plan.as_deref(), Some("Go"));
        assert_eq!(
            snapshot.period_ends_text.as_deref(),
            Some("Current period ends Nov 15, 2026")
        );
    }

    #[test]
    fn windows_are_never_swapped_even_when_the_seven_day_value_is_lower() {
        let snapshot = combine_windows(
            &live_group(FIVE_HOUR_GROUP, "91% left", 91.4, "Resets in 12 min"),
            &live_group(SEVEN_DAY_GROUP, "23% left", 23.4, "Resets in 2 days"),
            None,
            None,
        )
        .expect("both windows");
        let five = snapshot
            .windows
            .iter()
            .find(|window| window.label == OpenDesignWindowLabel::FiveHour)
            .unwrap();
        let seven = snapshot
            .windows
            .iter()
            .find(|window| window.label == OpenDesignWindowLabel::SevenDay)
            .unwrap();
        assert_eq!(five.remaining_percent, 91.4);
        assert_eq!(five.reset_text.as_deref(), Some("Resets in 12 min"));
        assert_eq!(seven.remaining_percent, 23.4);
        assert_eq!(seven.reset_text.as_deref(), Some("Resets in 2 days"));
    }

    #[test]
    fn a_group_with_no_bar_or_several_bars_is_not_read() {
        let no_bar = vec![text_node("5-hour limit"), text_node("73% left")];
        assert!(select_window_reading(OpenDesignWindowLabel::FiveHour, &no_bar).is_none());

        let two_bars = vec![
            bar_node(FIVE_HOUR_GROUP, Some(73.0)),
            bar_node(FIVE_HOUR_GROUP, Some(72.0)),
            text_node("Resets in 39 min"),
        ];
        assert!(select_window_reading(OpenDesignWindowLabel::FiveHour, &two_bars).is_none());
    }

    #[test]
    fn a_bar_without_a_readable_value_is_not_read() {
        let nodes = vec![bar_node(FIVE_HOUR_GROUP, None), text_node("73% used")];
        assert!(select_window_reading(OpenDesignWindowLabel::FiveHour, &nodes).is_none());
    }

    #[test]
    fn pattern_and_caption_must_agree_to_within_rounding() {
        let agreeing = vec![
            bar_node(FIVE_HOUR_GROUP, Some(73.21)),
            text_node("73% left"),
            text_node("Resets in 39 min"),
        ];
        assert!(select_window_reading(OpenDesignWindowLabel::FiveHour, &agreeing).is_some());

        let disagreeing = vec![
            bar_node(FIVE_HOUR_GROUP, Some(73.21)),
            text_node("91% left"),
        ];
        assert!(select_window_reading(OpenDesignWindowLabel::FiveHour, &disagreeing).is_none());
    }

    #[test]
    fn zero_resets_leaves_it_absent_and_two_make_the_group_ambiguous() {
        let absent = vec![bar_node(FIVE_HOUR_GROUP, Some(73.0))];
        let reading = select_window_reading(OpenDesignWindowLabel::FiveHour, &absent).unwrap();
        assert_eq!(reading.reset_text, None);

        let ambiguous = vec![
            bar_node(FIVE_HOUR_GROUP, Some(73.0)),
            text_node("Resets in 39 min"),
            text_node("Resets in 2 hr"),
        ];
        assert!(select_window_reading(OpenDesignWindowLabel::FiveHour, &ambiguous).is_none());
    }

    #[test]
    fn a_partial_pair_is_never_completed_by_substitution() {
        let five = live_group(FIVE_HOUR_GROUP, "73% left", 73.0, "Resets in 39 min");
        assert!(combine_windows(&five, &[], None, None).is_none());
        assert!(combine_windows(&[], &five, None, None).is_none());

        let no_reset = vec![bar_node(SEVEN_DAY_GROUP, Some(91.0))];
        assert!(combine_windows(&five, &no_reset, None, None).is_some());
    }

    #[test]
    fn control_characters_never_reach_the_displayed_text() {
        let reading = select_window_reading(
            OpenDesignWindowLabel::FiveHour,
            &[
                bar_node(FIVE_HOUR_GROUP, Some(73.0)),
                text_node("Resets\u{200e} in 39 min\u{0}"),
            ],
        )
        .unwrap();
        assert_eq!(reading.reset_text.as_deref(), Some("Resets in 39 min"));
    }

    // -- document URL anchor --

    #[test]
    fn only_the_open_design_console_url_is_accepted() {
        assert!(is_dashboard_url(
            "https://open-design.ai/cloud/dashboard?workspaceId=xog0mwpcnr0y9fr98j8w7cgm"
        ));
        assert!(is_dashboard_url(
            "open-design.ai/cloud/usage?workspaceId=abc"
        ));
        assert!(!is_dashboard_url("https://open-design.ai/"));
        assert!(!is_dashboard_url(
            "https://open-design.ai.evil.example/cloud/dashboard"
        ));
        assert!(!is_dashboard_url("https://example.com/cloud/dashboard"));
        assert!(!is_dashboard_url(""));
    }

    // -- failure classification and retention --

    #[test]
    fn no_chrome_window_is_chrome_not_running() {
        let outcome = classify_window_trees(&[]);
        assert_eq!(outcome.status, OpenDesignStatus::ChromeNotRunning);
        assert!(outcome.snapshot.is_none());
    }

    #[test]
    fn chrome_without_the_dashboard_or_quota_groups_is_dashboard_not_visible() {
        assert_eq!(
            classify_window_trees(&[WindowTree::DocumentAbsent]).status,
            OpenDesignStatus::DashboardNotVisible
        );
        assert_eq!(
            classify_window_trees(&[WindowTree::DocumentAbsent, WindowTree::QuotaAbsent]).status,
            OpenDesignStatus::DashboardNotVisible
        );
    }

    #[test]
    fn an_unreadable_tree_is_unknown_not_a_missing_screen() {
        assert_eq!(
            classify_window_trees(&[WindowTree::Unavailable, WindowTree::Unavailable]).status,
            OpenDesignStatus::Unknown
        );
    }

    #[test]
    fn a_failed_attempt_keeps_the_last_known_reading_untouched() {
        let mut cache = None;
        let first = ScanOutcome::ok(snapshot());
        let retained = retain_reading(&mut cache, &first).unwrap();
        assert_eq!(
            retained.observed_at,
            first.snapshot.as_ref().unwrap().observed_at
        );

        let failed = retain_reading(&mut cache, &ScanOutcome::dashboard_not_visible()).unwrap();
        assert_eq!(failed, retained);
        assert_eq!(failed.windows, retained.windows);
    }

    #[test]
    fn a_partial_reading_never_replaces_the_cached_pair() {
        let mut cache = Some(snapshot());
        let before = cache.clone();
        let single = OpenDesignSnapshot {
            windows: vec![OpenDesignWindowReading {
                label: OpenDesignWindowLabel::FiveHour,
                remaining_percent: 10.0,
                reset_text: None,
            }],
            plan: None,
            period_ends_text: None,
            observed_at: stamp().to_string(),
        };
        let after = retain_reading(&mut cache, &ScanOutcome::ok(single)).unwrap();
        assert_eq!(Some(after), before);

        let mut empty = None;
        assert!(retain_reading(&mut empty, &ScanOutcome::unknown()).is_none());
    }

    // -- wire contract --

    #[test]
    fn wire_uses_camel_case_verbatim_resets_and_snake_case_status() {
        let response = OpenDesignRefresh {
            status: OpenDesignStatus::DashboardNotVisible,
            observed_at: stamp().to_string(),
            windows: None,
            plan: None,
            period_ends_text: None,
            last_known: Some(snapshot()),
        };
        let wire = serde_json::to_string(&response).unwrap();
        assert!(wire.contains("\"status\":\"dashboard_not_visible\""));
        assert!(wire.contains("\"observedAt\":\"2026-10-17T09:12:00Z\""));
        assert!(wire.contains("\"lastKnown\":{"));
        assert!(wire.contains("\"label\":\"5-hour\""));
        assert!(wire.contains("\"label\":\"7-day\""));
        assert!(wire.contains("\"remainingPercent\":73.21"));
        assert!(wire.contains("\"resetText\":\"Resets in 39 min\""));
        assert!(wire.contains("\"plan\":\"Go\""));
        // The attempt itself carries no windows: the only pair on the wire
        // belongs to the retained last-known reading.
        assert!(wire.contains(
            "\"status\":\"dashboard_not_visible\",\"observedAt\":\"2026-10-17T09:12:00Z\",\"lastKnown\""
        ));
        assert!(!wire.contains("\"resetAt\""));
        assert!(!wire.contains("usedPercent"));
    }

    #[test]
    fn a_conflict_or_ambiguity_is_a_typed_status_never_an_error_envelope() {
        let wire = serde_json::to_string(&OpenDesignRefresh {
            status: OpenDesignStatus::ChromeNotRunning,
            observed_at: stamp().to_string(),
            windows: None,
            plan: None,
            period_ends_text: None,
            last_known: None,
        })
        .unwrap();
        assert!(!wire.contains("windows"));
        assert!(!wire.contains("lastKnown"));
        assert!(!wire.contains("error"));
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
            ["Set", "WindowPos"].concat(),
            ["Show", "Window"].concat(),
            ["re", "load"].concat(),
            ["reqw", "est"].concat(),
            ["document", ".cookie"].concat(),
            ["Auto", "rization"].concat(),
            ["Bear", "er "].concat(),
            ["set", "Interval"].concat(),
            ["TO", "KEN"].concat(),
        ];
        let text = include_str!("opendesign.rs");
        for symbol in forbidden {
            assert!(
                !text.contains(&symbol),
                "opendesign.rs must stay read-only and offline; found {symbol}"
            );
        }
    }

    #[test]
    fn the_cache_is_only_written_by_a_complete_read() {
        // Process-local, in-memory only: no persistence call exists here.
        let text = include_str!("opendesign.rs");
        for symbol in [["write", "_all"], ["File", "::create"], ["fs", "::write"]] {
            assert!(!text.contains(&symbol.concat()));
        }
    }

    #[test]
    fn a_finished_scan_releases_the_slot_for_the_next_refresh() {
        let slot = Arc::new(AtomicBool::new(false));
        for _ in 0..2 {
            let outcome =
                run_single_flight(&slot, Duration::from_secs(10), || ScanOutcome::unknown());
            assert_eq!(outcome.status, OpenDesignStatus::Unknown);
        }
        assert!(!slot.load(Ordering::Acquire));
    }

    #[test]
    fn a_second_refresh_cannot_start_while_the_first_is_in_flight() {
        let slot = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let slot_for_first = Arc::clone(&slot);
        let first = thread::spawn(move || {
            run_single_flight(&slot_for_first, Duration::from_secs(10), move || {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
                ScanOutcome::unknown()
            })
        });
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker entered");
        let second = run_single_flight(&slot, Duration::from_millis(10), || {
            panic!("a second scan must not start")
        });
        assert_eq!(second.status, OpenDesignStatus::Unknown);
        let _ = release_tx.send(());
        let first_outcome = first.join().expect("first caller");
        assert_eq!(first_outcome.status, OpenDesignStatus::Unknown);
        assert!(!slot.load(Ordering::Acquire));
    }

    /// Live integration probe — deliberately ignored in the ordinary suite.
    /// Run it with the --ignored flag while Chrome shows the signed-in
    /// Plan & balance screen; it asserts only that the real platform scan
    /// returns a typed status inside its watchdog budget, never specific
    /// usage values.
    #[cfg(windows)]
    #[test]
    #[ignore = "manual live probe: needs the real desktop session state"]
    fn opendesign_live_probe_reports_a_typed_status_within_the_watchdog() {
        let started = std::time::Instant::now();
        let outcome = native::scan();
        let elapsed = started.elapsed();
        println!("live probe status: {:?} in {elapsed:?}", outcome.status);
        println!(
            "live probe windows: {:?}",
            outcome.snapshot.as_ref().map(|snapshot| &snapshot.windows)
        );
        println!(
            "live probe plan: {:?} period: {:?}",
            outcome.snapshot.as_ref().and_then(|s| s.plan.as_deref()),
            outcome
                .snapshot
                .as_ref()
                .and_then(|s| s.period_ends_text.as_deref())
        );
        assert!(
            elapsed < Duration::from_millis(SCAN_TIMEOUT_MS + 1_000),
            "scan exceeded its watchdog budget: {elapsed:?}"
        );
        assert!(matches!(
            outcome.status,
            OpenDesignStatus::Ok
                | OpenDesignStatus::ChromeNotRunning
                | OpenDesignStatus::DashboardNotVisible
                | OpenDesignStatus::Unknown
        ));
    }
}
