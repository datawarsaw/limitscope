//! Claude Code subscription usage limits, received passively from the
//! documented `statusLine` hook (https://code.claude.com/docs/en/statusline).
//!
//! Claude Code runs a user-configured `statusLine` command on its own events
//! and writes a JSON document to that command's stdin. For claude.ai
//! Pro/Max/Ultra subscribers the document may carry
//! `rate_limits.five_hour` and `rate_limits.seven_day`, each with
//! `used_percentage` (0-100) and `resets_at` (Unix epoch seconds). Either
//! window can be absent, the object appears only after the first API
//! response of a session, and Claude Code drops a window once it has reset.
//!
//! This module is the whole integration. It has two halves that share one
//! tiny file and nothing else:
//!
//! * the ingest half, run by the packaged LimitScope executable itself as
//!   `LimitScope.exe --claude-statusline-ingest` (so nothing depends on the
//!   source checkout, Node, jq, or a script on disk). It reads stdin with a
//!   hard size cap, keeps ONLY the two allow-listed windows after strict
//!   validation, merges them into the stored snapshot and replaces the
//!   file atomically. It prints nothing and always exits 0, so it can never
//!   disturb a Claude Code session;
//! * the read half, used by the shared runtime to project the stored
//!   snapshot as the `claude-code` provider.
//!
//! Nothing here uses credentials, the network, a private API, or a model
//! call, and nothing ever touches `~/.claude`. The rest of the stdin
//! document (session id, transcript path, workspace, model, cost, ...) is
//! parsed past and discarded; it is never stored or logged.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only argument that switches the executable into ingest mode. It must
/// be the first argument; anything else starts the normal desktop app.
pub(crate) const INGEST_FLAG: &str = "--claude-statusline-ingest";
/// Same value as `identifier` in `tauri.conf.json` (pinned by a test): the
/// ingest process runs before Tauri exists, so it derives the app-data
/// directory itself.
pub(crate) const APP_IDENTIFIER: &str = "com.ratelimits.desktop";
pub(crate) const STORE_FILE_NAME: &str = "claude-statusline-v1.json";
/// Provenance label stored with the snapshot.
pub(crate) const SOURCE: &str = "claude-code-statusline";

const SCHEMA_VERSION: u64 = 1;
/// Claude Code's stdin document is a few KiB; 1 MiB leaves ample headroom
/// while bounding memory. An oversized document is rejected unread-further.
pub(crate) const MAX_INPUT_BYTES: usize = 1024 * 1024;
/// The stored snapshot is a few hundred bytes; anything larger is not ours.
const MAX_STORE_BYTES: u64 = 4096;
/// A five-hour window cannot reset more than five hours out; one hour of
/// slack tolerates server-side rounding. The weekly window gets a day.
const FIVE_HOUR_HORIZON_S: i64 = 6 * 3600;
const SEVEN_DAY_HORIZON_S: i64 = 8 * 86_400;
/// Two reset stamps this close describe the same window (server-side
/// rounding between responses).
const SAME_WINDOW_TOLERANCE_S: i64 = 300;
/// Tolerated clock skew for stored observation times.
const CLOCK_SKEW_S: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowKind {
    FiveHour,
    SevenDay,
}

impl WindowKind {
    fn horizon_s(self) -> i64 {
        match self {
            WindowKind::FiveHour => FIVE_HOUR_HORIZON_S,
            WindowKind::SevenDay => SEVEN_DAY_HORIZON_S,
        }
    }
}

/// One validated window. `observed_at` is when LimitScope received it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WindowReading {
    pub used_percent: f64,
    /// Unix epoch seconds, as Claude Code reports it.
    pub resets_at: i64,
    /// Unix epoch seconds at receipt of the last defensibly new information.
    pub observed_at: i64,
}

/// The entire persisted document. No session, transcript, workspace, model,
/// cost or account field exists in this schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub version: u64,
    pub source: String,
    pub five_hour: Option<WindowReading>,
    pub seven_day: Option<WindowReading>,
}

impl Snapshot {
    fn empty() -> Self {
        Snapshot {
            version: SCHEMA_VERSION,
            source: SOURCE.to_string(),
            five_hour: None,
            seven_day: None,
        }
    }
}

/// The result of reading the stored snapshot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StoreRead {
    /// No file: no hook has delivered anything yet.
    Missing,
    /// A file exists but cannot be trusted (corrupt, oversized, foreign or
    /// future schema, out-of-range values).
    Invalid,
    Valid(Snapshot),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IngestError {
    Oversized,
    Malformed,
}

/// Windows extracted from one stdin document, already validated.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Observed {
    pub five_hour: Option<(f64, i64)>,
    pub seven_day: Option<(f64, i64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IngestOutcome {
    Written,
    /// The stored snapshot already holds this reading.
    Unchanged,
    /// A valid document with no usable allow-listed window: nothing stored,
    /// and nothing already stored is touched or cleared.
    NoRateLimits,
    Rejected(IngestError),
    /// The store belongs to a newer schema; it is left untouched.
    StoreNotWritable,
    WriteFailed,
}

// ---------- parsing ----------

/// Reads at most `MAX_INPUT_BYTES`; one byte more means oversized.
pub(crate) fn read_bounded<R: Read>(reader: R) -> Result<Vec<u8>, IngestError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| IngestError::Malformed)?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(IngestError::Oversized);
    }
    Ok(bytes)
}

fn parse_window(value: Option<&Value>, kind: WindowKind, now_s: i64) -> Option<(f64, i64)> {
    let object = value?.as_object()?;
    // JSON numbers only: a string such as "50" is not a percentage.
    let percent = object.get("used_percentage")?.as_f64()?;
    let resets_at = object.get("resets_at")?.as_i64()?;
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return None;
    }
    // Already-reset windows are dropped by Claude Code itself; one that
    // arrives anyway carries no current information.
    if resets_at <= now_s || resets_at > now_s + kind.horizon_s() {
        return None;
    }
    Some((percent, resets_at))
}

/// Extracts the two allow-listed windows from one stdin document. Every
/// other field is ignored and dropped. Each window is validated on its own,
/// so one malformed window never costs the other.
pub(crate) fn parse_payload(bytes: &[u8], now_s: i64) -> Result<Observed, IngestError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(IngestError::Oversized);
    }
    let root: Value = serde_json::from_slice(bytes).map_err(|_| IngestError::Malformed)?;
    let root = root.as_object().ok_or(IngestError::Malformed)?;
    let limits = root.get("rate_limits").and_then(Value::as_object);
    Ok(Observed {
        five_hour: parse_window(
            limits.and_then(|l| l.get("five_hour")),
            WindowKind::FiveHour,
            now_s,
        ),
        seven_day: parse_window(
            limits.and_then(|l| l.get("seven_day")),
            WindowKind::SevenDay,
            now_s,
        ),
    })
}

// ---------- store ----------

pub(crate) fn store_path(dir: &Path) -> PathBuf {
    dir.join(STORE_FILE_NAME)
}

fn stored_window_is_valid(window: &WindowReading, kind: WindowKind, now_s: i64) -> bool {
    window.used_percent.is_finite()
        && (0.0..=100.0).contains(&window.used_percent)
        && window.observed_at > 0
        && window.observed_at <= now_s + CLOCK_SKEW_S
        && window.resets_at > window.observed_at - CLOCK_SKEW_S
        && window.resets_at <= window.observed_at + kind.horizon_s()
}

/// Reads and fully re-validates the stored snapshot.
pub(crate) fn read_store(dir: &Path, now_s: i64) -> StoreRead {
    let file = match fs::File::open(store_path(dir)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return StoreRead::Missing,
        Err(_) => return StoreRead::Invalid,
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_STORE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_STORE_BYTES
    {
        return StoreRead::Invalid;
    }
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return StoreRead::Invalid;
    };
    if value.get("version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return StoreRead::Invalid;
    }
    let Ok(snapshot) = serde_json::from_value::<Snapshot>(value) else {
        return StoreRead::Invalid;
    };
    let five_ok = snapshot
        .five_hour
        .as_ref()
        .is_none_or(|w| stored_window_is_valid(w, WindowKind::FiveHour, now_s));
    let seven_ok = snapshot
        .seven_day
        .as_ref()
        .is_none_or(|w| stored_window_is_valid(w, WindowKind::SevenDay, now_s));
    if snapshot.source != SOURCE || !five_ok || !seven_ok {
        return StoreRead::Invalid;
    }
    StoreRead::Valid(snapshot)
}

/// Merges one new reading into the stored window.
///
/// Several Claude Code sessions can run at once and each runs the hook, so
/// the rule must tolerate interleaving and replays: within one window the
/// used percentage never decreases, so the larger value wins; a reading for
/// a different reset while the old window remains open is ambiguous and
/// ignored. An expired stored window is discarded before a new one is accepted.
fn merge_window(
    old: Option<WindowReading>,
    new: Option<(f64, i64)>,
    now_s: i64,
) -> Option<WindowReading> {
    let old = old.filter(|window| window.resets_at > now_s);
    let Some((percent, resets_at)) = new else {
        return old;
    };
    let fresh = WindowReading {
        used_percent: percent,
        resets_at,
        observed_at: now_s,
    };
    let Some(old) = old else {
        return Some(fresh);
    };
    if (old.resets_at - resets_at).abs() <= SAME_WINDOW_TOLERANCE_S {
        // Rounding and lagging sessions do not establish a new observation.
        // Keep the old reset identity to avoid re-arming threshold alerts.
        if percent > old.used_percent {
            Some(WindowReading {
                used_percent: percent,
                observed_at: now_s,
                ..old
            })
        } else {
            Some(old)
        }
    } else {
        // A later-looking reset before the old window ends is ambiguous.
        Some(old)
    }
}

fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // Concurrent hooks can race renames onto one target on Windows; the
    // loser gets a transient sharing/permission error, so retry briefly.
    let mut last = None;
    for attempt in 0..5 {
        match crate::durable_file::replace(path, bytes) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(15 * (attempt + 1)));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("replace failed")))
}

/// Applies one stdin document to the store in `dir`.
pub(crate) fn ingest(dir: &Path, input: &[u8], now_s: i64) -> IngestOutcome {
    let observed = match parse_payload(input, now_s) {
        Ok(observed) => observed,
        Err(error) => return IngestOutcome::Rejected(error),
    };
    if observed.five_hour.is_none() && observed.seven_day.is_none() {
        return IngestOutcome::NoRateLimits;
    }
    // A persistent lock file is never removed: removing it can split waiters
    // across different inodes. The OS releases the lock even after a crash.
    if fs::create_dir_all(dir).is_err() {
        return IngestOutcome::WriteFailed;
    }
    let lock = match fs::OpenOptions::new().read(true).write(true).create(true)
        .open(dir.join("claude-statusline-v1.lock")) {
        Ok(lock) => lock,
        Err(_) => return IngestOutcome::WriteFailed,
    };
    if lock.lock().is_err() {
        return IngestOutcome::WriteFailed;
    }
    let path = store_path(dir);
    let stored = match read_store(dir, now_s) {
        StoreRead::Valid(snapshot) => snapshot,
        StoreRead::Missing => Snapshot::empty(),
        StoreRead::Invalid => {
            // A newer schema is never overwritten (an app downgrade must not
            // destroy it); any other unreadable file is a derived cache and
            // is replaced.
            let future = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|value| value.get("version").and_then(Value::as_u64))
                .is_some_and(|version| version > SCHEMA_VERSION);
            if future {
                return IngestOutcome::StoreNotWritable;
            }
            Snapshot::empty()
        }
    };
    let next = Snapshot {
        version: SCHEMA_VERSION,
        source: SOURCE.to_string(),
        five_hour: merge_window(stored.five_hour, observed.five_hour, now_s),
        seven_day: merge_window(stored.seven_day, observed.seven_day, now_s),
    };
    if stored == next {
        return IngestOutcome::Unchanged;
    }
    let Ok(bytes) = serde_json::to_vec(&next) else {
        return IngestOutcome::WriteFailed;
    };
    match replace_file(&path, &bytes) {
        Ok(()) => IngestOutcome::Written,
        Err(_) => IngestOutcome::WriteFailed,
    }
}

// ---------- CLI entry ----------

/// The app-data directory the Tauri runtime resolves for this identifier.
/// A test pins this to `app.path().app_data_dir()`.
pub(crate) fn data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library").join("Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    base.filter(|base| base.is_absolute())
        .map(|base| base.join(APP_IDENTIFIER))
}

pub(crate) fn read_default_store(now: DateTime<Utc>) -> StoreRead {
    match data_dir() {
        Some(dir) => read_store(&dir, now.timestamp()),
        None => StoreRead::Missing,
    }
}

/// Runs ingest mode when the process was started with the ingest flag.
/// Returns the exit code, or `None` for a normal app start. The exit code is
/// always 0 and nothing is printed: a status line command must never
/// disturb the Claude Code session that runs it.
pub(crate) fn run_cli_if_requested(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some(INGEST_FLAG) {
        return None;
    }
    if let (Some(dir), Ok(input)) = (data_dir(), read_bounded(std::io::stdin().lock())) {
        let _ = ingest(&dir, &input, Utc::now().timestamp());
    }
    Some(0)
}

// ---------- operator setup (display only) ----------

/// What the dashboard shows an operator who wants to opt in. LimitScope
/// never writes this into Claude Code's settings itself.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeStatuslineSetup {
    /// The `statusLine.command` value for Git Bash (Claude Code's Windows
    /// default when Git Bash is installed). `None` when this executable's
    /// path cannot be quoted safely.
    pub command: Option<String>,
    /// The PowerShell spelling; equal to `command` when the path needs no
    /// quoting.
    pub powershell_command: Option<String>,
    /// A complete `statusLine` settings.json fragment for `command`.
    pub settings_snippet: Option<String>,
}

fn path_is_quotable(path: &str) -> bool {
    !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-' | ':' | '/' | '(' | ')'))
}

pub(crate) fn setup_for_executable(exe: Option<&Path>) -> ClaudeStatuslineSetup {
    let unavailable = ClaudeStatuslineSetup {
        command: None,
        powershell_command: None,
        settings_snippet: None,
    };
    let Some(exe) = exe.and_then(Path::to_str) else {
        return unavailable;
    };
    let path = exe.replace('\\', "/");
    if !path_is_quotable(&path) {
        return unavailable;
    }
    let (command, powershell) = if path.contains(' ') || path.contains('(') || path.contains(')') {
        (
            format!("\"{path}\" {INGEST_FLAG}"),
            format!("& \"{path}\" {INGEST_FLAG}"),
        )
    } else {
        let bare = format!("{path} {INGEST_FLAG}");
        (bare.clone(), bare)
    };
    let snippet = serde_json::to_string_pretty(&serde_json::json!({
        "statusLine": { "type": "command", "command": command }
    }))
    .ok();
    ClaudeStatuslineSetup {
        command: Some(command),
        powershell_command: Some(powershell),
        settings_snippet: snippet,
    }
}

#[tauri::command]
pub fn get_claude_statusline_setup() -> ClaudeStatuslineSetup {
    setup_for_executable(std::env::current_exe().ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const NOW: i64 = 1_800_000_000;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("limitscope-claude-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn payload(five: Option<(f64, i64)>, seven: Option<(f64, i64)>) -> Vec<u8> {
        let mut limits = serde_json::Map::new();
        if let Some((p, r)) = five {
            limits.insert("five_hour".into(), serde_json::json!({"used_percentage": p, "resets_at": r}));
        }
        if let Some((p, r)) = seven {
            limits.insert("seven_day".into(), serde_json::json!({"used_percentage": p, "resets_at": r}));
        }
        // Surrounding fields stand in for the session metadata that must
        // never be stored. Synthetic values only.
        serde_json::to_vec(&serde_json::json!({
            "session_id": "synthetic-session-marker",
            "transcript_path": "/synthetic/transcript-marker.jsonl",
            "cwd": "/synthetic/workspace-marker",
            "model": {"display_name": "synthetic-model-marker"},
            "rate_limits": limits,
        }))
        .unwrap()
    }

    #[test]
    fn valid_payload_yields_both_windows() {
        let bytes = payload(Some((23.5, NOW + 3600)), Some((41.2, NOW + 86_400)));
        let observed = parse_payload(&bytes, NOW).unwrap();
        assert_eq!(observed.five_hour, Some((23.5, NOW + 3600)));
        assert_eq!(observed.seven_day, Some((41.2, NOW + 86_400)));
    }

    #[test]
    fn windows_are_independent_and_absence_is_not_zero() {
        let only_weekly = payload(None, Some((10.0, NOW + 1000)));
        let observed = parse_payload(&only_weekly, NOW).unwrap();
        assert_eq!(observed.five_hour, None);
        assert_eq!(observed.seven_day, Some((10.0, NOW + 1000)));
        let none = parse_payload(br#"{"model":{"display_name":"x"}}"#, NOW).unwrap();
        assert_eq!(none, Observed::default());
        let empty_limits = parse_payload(br#"{"rate_limits":{}}"#, NOW).unwrap();
        assert_eq!(empty_limits, Observed::default());
    }

    #[test]
    fn malformed_documents_are_rejected() {
        for bytes in [
            &b""[..],
            b"not json",
            b"[1,2,3]",
            b"null",
            b"\"rate_limits\"",
            b"{\"rate_limits\": ",
        ] {
            assert_eq!(parse_payload(bytes, NOW), Err(IngestError::Malformed), "{bytes:?}");
        }
    }

    #[test]
    fn invalid_windows_are_dropped_individually() {
        let cases = [
            serde_json::json!({"used_percentage": -0.1, "resets_at": NOW + 10}),
            serde_json::json!({"used_percentage": 100.1, "resets_at": NOW + 10}),
            serde_json::json!({"used_percentage": "50", "resets_at": NOW + 10}),
            serde_json::json!({"used_percentage": null, "resets_at": NOW + 10}),
            serde_json::json!({"used_percentage": 5, "resets_at": "soon"}),
            serde_json::json!({"used_percentage": 5, "resets_at": 1.5}),
            serde_json::json!({"used_percentage": 5}),
            serde_json::json!({"resets_at": NOW + 10}),
            serde_json::json!({"used_percentage": 5, "resets_at": NOW}),
            serde_json::json!({"used_percentage": 5, "resets_at": NOW - 5}),
            serde_json::json!({"used_percentage": 5, "resets_at": NOW + FIVE_HOUR_HORIZON_S + 1}),
            serde_json::json!(7),
            serde_json::json!(null),
        ];
        for case in cases {
            let doc = serde_json::json!({"rate_limits": {
                "five_hour": case,
                "seven_day": {"used_percentage": 30, "resets_at": NOW + 5000}
            }});
            let observed = parse_payload(&serde_json::to_vec(&doc).unwrap(), NOW).unwrap();
            assert_eq!(observed.five_hour, None, "{case}");
            assert_eq!(observed.seven_day, Some((30.0, NOW + 5000)), "sibling survives: {case}");
        }
        // The boundary values are accepted.
        let edge = payload(Some((0.0, NOW + 1)), Some((100.0, NOW + SEVEN_DAY_HORIZON_S)));
        let observed = parse_payload(&edge, NOW).unwrap();
        assert_eq!(observed.five_hour, Some((0.0, NOW + 1)));
        assert_eq!(observed.seven_day, Some((100.0, NOW + SEVEN_DAY_HORIZON_S)));
    }

    #[test]
    fn spend_limit_and_unknown_fields_are_never_ingested() {
        let doc = serde_json::json!({"rate_limits": {
            "spend_limit": {"used_percentage": 99, "resets_at": NOW + 100, "used_usd": 1, "limit_usd": 2},
            "monthly": {"used_percentage": 99, "resets_at": NOW + 100}
        }});
        let observed = parse_payload(&serde_json::to_vec(&doc).unwrap(), NOW).unwrap();
        assert_eq!(observed, Observed::default());
    }

    #[test]
    fn oversized_input_is_rejected_without_reading_it_all() {
        let mut big = br#"{"padding":""#.to_vec();
        big.resize(MAX_INPUT_BYTES + 10, b'a');
        assert_eq!(read_bounded(&big[..]), Err(IngestError::Oversized));
        assert_eq!(parse_payload(&big, NOW), Err(IngestError::Oversized));
        let exactly = vec![b' '; MAX_INPUT_BYTES];
        assert_eq!(read_bounded(&exactly[..]).unwrap().len(), MAX_INPUT_BYTES);
        // Pathologically deep nesting fails closed instead of overflowing.
        let deep = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
        assert_eq!(parse_payload(deep.as_bytes(), NOW), Err(IngestError::Malformed));
    }

    #[test]
    fn ingest_persists_only_the_allow_listed_minimum() {
        let dir = temp_dir("minimal");
        let outcome = ingest(&dir, &payload(Some((12.5, NOW + 4000)), Some((3.0, NOW + 90_000))), NOW);
        assert_eq!(outcome, IngestOutcome::Written);
        let raw = fs::read_to_string(store_path(&dir)).unwrap();
        for marker in ["synthetic", "session", "transcript", "workspace", "model", "cwd"] {
            assert!(!raw.contains(marker), "{marker} must not be persisted: {raw}");
        }
        let value: Value = serde_json::from_str(&raw).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["fiveHour", "sevenDay", "source", "version"]);
        assert!(raw.len() < 400, "{}", raw.len());
        match read_store(&dir, NOW) {
            StoreRead::Valid(snapshot) => {
                assert_eq!(snapshot.source, SOURCE);
                assert_eq!(snapshot.five_hour.unwrap().used_percent, 12.5);
                assert_eq!(snapshot.five_hour.unwrap().observed_at, NOW);
                assert_eq!(snapshot.seven_day.unwrap().resets_at, NOW + 90_000);
            }
            other => panic!("{other:?}"),
        }
        // No temporary files linger.
        let mut names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        assert_eq!(names, [STORE_FILE_NAME, "claude-statusline-v1.lock"]);
        assert_eq!(fs::metadata(dir.join("claude-statusline-v1.lock")).unwrap().len(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ingest_replay_never_refreshes_even_after_idle() {
        let dir = temp_dir("idem");
        let doc = payload(Some((20.0, NOW + 4000)), None);
        assert_eq!(ingest(&dir, &doc, NOW), IngestOutcome::Written);
        let first = fs::read(store_path(&dir)).unwrap();
        assert_eq!(ingest(&dir, &doc, NOW + 1), IngestOutcome::Unchanged);
        assert_eq!(fs::read(store_path(&dir)).unwrap(), first);
        // Identical replays, including after 15 minutes, never advance freshness.
        assert_eq!(ingest(&dir, &doc, NOW + 1000), IngestOutcome::Unchanged);
        assert_eq!(fs::read(store_path(&dir)).unwrap(), first);
        match read_store(&dir, NOW + 1000) {
            StoreRead::Valid(s) => {
                let w = s.five_hour.unwrap();
                assert_eq!((w.used_percent, w.resets_at, w.observed_at), (20.0, NOW + 4000, NOW));
            }
            other => panic!("{other:?}"),
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_or_rejected_documents_never_create_or_clear_data() {
        let dir = temp_dir("noclear");
        assert_eq!(ingest(&dir, br#"{"model":{}}"#, NOW), IngestOutcome::NoRateLimits);
        assert_eq!(ingest(&dir, b"garbage", NOW), IngestOutcome::Rejected(IngestError::Malformed));
        assert_eq!(read_store(&dir, NOW), StoreRead::Missing);
        assert_eq!(ingest(&dir, &payload(Some((50.0, NOW + 4000)), None), NOW), IngestOutcome::Written);
        let before = fs::read(store_path(&dir)).unwrap();
        // A session that has not had a response yet sends no rate_limits:
        // the stored reading must survive untouched.
        assert_eq!(ingest(&dir, br#"{"model":{}}"#, NOW + 100), IngestOutcome::NoRateLimits);
        assert_eq!(ingest(&dir, b"[]", NOW + 100), IngestOutcome::Rejected(IngestError::Malformed));
        assert_eq!(fs::read(store_path(&dir)).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn merge_never_lowers_a_window_and_prefers_the_later_window() {
        let dir = temp_dir("merge");
        ingest(&dir, &payload(Some((60.0, NOW + 4000)), None), NOW);
        // A lagging session reports a lower value for the same window.
        ingest(&dir, &payload(Some((55.0, NOW + 4001)), None), NOW + 30);
        let StoreRead::Valid(s) = read_store(&dir, NOW + 30) else { panic!() };
        assert_eq!(s.five_hour.unwrap().used_percent, 60.0);
        assert_eq!(s.five_hour.unwrap().resets_at, NOW + 4000);
        assert_eq!(s.five_hour.unwrap().observed_at, NOW);
        // A stale session reporting an EARLIER window is ignored.
        ingest(&dir, &payload(Some((99.0, NOW + 100)), None), NOW + 40);
        let StoreRead::Valid(s) = read_store(&dir, NOW + 40) else { panic!() };
        assert_eq!(s.five_hour.unwrap().used_percent, 60.0);
        // After the reset the next window replaces it (low usage is real).
        let later = NOW + 5000;
        ingest(&dir, &payload(Some((2.0, later + 17_000)), None), later);
        let StoreRead::Valid(s) = read_store(&dir, later) else { panic!() };
        assert_eq!(s.five_hour.unwrap().used_percent, 2.0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn independently_advances_windows_and_survives_restart_and_rounding() {
        let dir = temp_dir("provenance");
        let first = payload(Some((10.0, NOW + 4000)), Some((20.0, NOW + 90_000)));
        assert_eq!(ingest(&dir, &first, NOW), IngestOutcome::Written);
        // A second session advances only 5h; its rounded weekly replay is not new.
        let next = payload(Some((11.0, NOW + 4001)), Some((20.0, NOW + 90_100)));
        assert_eq!(ingest(&dir, &next, NOW + 1000), IngestOutcome::Written);
        let StoreRead::Valid(saved) = read_store(&dir, NOW + 1000) else {
            panic!()
        };
        assert_eq!(saved.five_hour.unwrap().observed_at, NOW + 1000);
        assert_eq!(saved.seven_day.unwrap().observed_at, NOW);
        assert_eq!(saved.seven_day.unwrap().resets_at, NOW + 90_000);
        // Re-reading the persisted v1 snapshot and replaying either session is idempotent.
        assert_eq!(ingest(&dir, &first, NOW + 1100), IngestOutcome::Unchanged);
        assert_eq!(ingest(&dir, &next, NOW + 1100), IngestOutcome::Unchanged);
        assert_eq!(read_store(&dir, NOW + 1100), StoreRead::Valid(saved));
        // A later reset before the old one expires is not sufficient proof of a new cycle.
        assert_eq!(
            ingest(&dir, &payload(Some((99.0, NOW + 8000)), None), NOW + 1100),
            IngestOutcome::Unchanged
        );
        // After each reset a low new-window value is real, including the weekly window.
        let later = NOW + 90_001;
        assert_eq!(
            ingest(
                &dir,
                &payload(Some((2.0, later + 4000)), Some((1.0, later + 90_000))),
                later
            ),
            IngestOutcome::Written
        );
        let StoreRead::Valid(saved) = read_store(&dir, later) else {
            panic!()
        };
        assert_eq!(saved.five_hour.unwrap().used_percent, 2.0);
        assert_eq!(saved.seven_day.unwrap().used_percent, 1.0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn expired_stored_windows_are_dropped_on_the_next_write() {
        let dir = temp_dir("expire");
        ingest(&dir, &payload(Some((70.0, NOW + 100)), Some((10.0, NOW + 80_000))), NOW);
        let later = NOW + 200;
        ingest(&dir, &payload(None, Some((11.0, NOW + 80_000))), later);
        let StoreRead::Valid(s) = read_store(&dir, later) else { panic!() };
        assert_eq!(s.five_hour, None, "an expired window is dropped, never shown as 0");
        assert_eq!(s.seven_day.unwrap().used_percent, 11.0);
        fs::remove_dir_all(dir).unwrap();
    }

    // Launched in separate test processes so this exercises the OS lock,
    // not just synchronization inside one process.
    #[test]
    fn concurrent_ingest_child() {
        let Some(dir) = std::env::var_os("LIMITSCOPE_TEST_INGEST_DIR") else { return };
        let dir = PathBuf::from(dir);
        let value: f64 = std::env::var("LIMITSCOPE_TEST_INGEST_VALUE").unwrap().parse().unwrap();
        fs::write(dir.join(format!("ready-{value}")), b"ready").unwrap();
        assert!(matches!(ingest(&dir, &payload(Some((value, NOW + 4000)), None), NOW),
            IngestOutcome::Written | IngestOutcome::Unchanged));
    }

    #[test]
    fn concurrent_processes_serialize_read_merge_write() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let dir = temp_dir("process-lock");
        assert_eq!(ingest(&dir, &payload(Some((50.0, NOW + 4000)), None), NOW), IngestOutcome::Written);
        let lock = fs::OpenOptions::new().read(true).write(true)
            .open(dir.join("claude-statusline-v1.lock")).unwrap();
        lock.lock().unwrap();
        let mut children: Vec<_> = [60, 55].into_iter().map(|value| {
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "claude_statusline::tests::concurrent_ingest_child", "--nocapture"])
                .env("LIMITSCOPE_TEST_INGEST_DIR", &dir)
                .env("LIMITSCOPE_TEST_INGEST_VALUE", value.to_string())
                .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
        }).collect();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !(dir.join("ready-60").exists() && dir.join("ready-55").exists()) {
            assert!(Instant::now() < deadline, "children did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        for child in &mut children {
            assert!(child.try_wait().unwrap().is_none(), "ingest must wait for the lock");
        }
        assert_eq!(read_store(&dir, NOW), StoreRead::Valid(Snapshot {
            version: SCHEMA_VERSION, source: SOURCE.to_string(),
            five_hour: Some(WindowReading { used_percent: 50.0, resets_at: NOW + 4000, observed_at: NOW }),
            seven_day: None,
        }));
        drop(lock);
        for child in &mut children { assert!(child.wait().unwrap().success()); }
        let StoreRead::Valid(snapshot) = read_store(&dir, NOW) else { panic!("invalid store") };
        assert_eq!(snapshot.five_hour.unwrap().used_percent, 60.0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_hooks_leave_one_valid_snapshot() {
        let dir = Arc::new(temp_dir("concurrent"));
        let mut handles = Vec::new();
        for worker in 0..8i64 {
            let dir = dir.clone();
            handles.push(std::thread::spawn(move || {
                for step in 0..10i64 {
                    let percent = (worker * 10 + step) as f64 / 2.0;
                    let doc = payload(Some((percent, NOW + 4000)), Some((percent, NOW + 90_000)));
                    // A WriteFailed under extreme contention is tolerated;
                    // a corrupt file never is.
                    let _ = ingest(&dir, &doc, NOW + step * 6);
                    if let StoreRead::Invalid = read_store(&dir, NOW + step * 6) {
                        panic!("observed a torn or invalid store");
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let StoreRead::Valid(snapshot) = read_store(&dir, NOW + 100) else { panic!("final store invalid") };
        let w = snapshot.five_hour.unwrap();
        assert!((0.0..=100.0).contains(&w.used_percent));
        let leftovers = fs::read_dir(&*dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        fs::remove_dir_all(&*dir).unwrap();
    }

    #[test]
    fn read_store_fails_closed_on_untrusted_files() {
        let dir = temp_dir("untrusted");
        let path = store_path(&dir);
        let good = |five: &str| {
            format!(r#"{{"version":1,"source":"{SOURCE}","fiveHour":{five},"sevenDay":null}}"#)
        };
        let window = |pct: &str, resets: i64, observed: i64| {
            format!(r#"{{"usedPercent":{pct},"resetsAt":{resets},"observedAt":{observed}}}"#)
        };
        assert_eq!(read_store(&dir, NOW), StoreRead::Missing);
        let invalid: Vec<String> = vec![
            "".into(),
            "{ not json".into(),
            "[]".into(),
            r#"{"version":2,"source":"claude-code-statusline","fiveHour":null,"sevenDay":null}"#.into(),
            r#"{"version":1,"source":"someone-else","fiveHour":null,"sevenDay":null}"#.into(),
            r#"{"version":1,"source":"claude-code-statusline","fiveHour":null,"sevenDay":null,"extra":1}"#.into(),
            good(&window("101", NOW + 100, NOW)),
            good(&window("-1", NOW + 100, NOW)),
            good(&window("5", NOW + 100, NOW + 100_000)), // observed in the future
            good(&window("5", NOW + 100_000, NOW)),       // beyond the horizon
            good(&window("\"5\"", NOW + 100, NOW)),
            format!(r#"{{"version":1,"source":"{SOURCE}","pad":"{}"}}"#, "x".repeat(5000)),
        ];
        for (index, body) in invalid.iter().enumerate() {
            fs::write(&path, body).unwrap();
            assert_eq!(read_store(&dir, NOW), StoreRead::Invalid, "case {index}");
        }
        fs::write(&path, good(&window("5", NOW + 100, NOW))).unwrap();
        assert!(matches!(read_store(&dir, NOW), StoreRead::Valid(_)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn future_schema_store_is_never_overwritten_but_corrupt_cache_is_replaced() {
        let dir = temp_dir("future");
        let path = store_path(&dir);
        let future = br#"{"version":9,"data":"keep"}"#;
        fs::write(&path, future).unwrap();
        let doc = payload(Some((5.0, NOW + 4000)), None);
        assert_eq!(ingest(&dir, &doc, NOW), IngestOutcome::StoreNotWritable);
        assert_eq!(fs::read(&path).unwrap(), future);
        fs::write(&path, b"{ corrupt").unwrap();
        assert_eq!(ingest(&dir, &doc, NOW), IngestOutcome::Written);
        assert!(matches!(read_store(&dir, NOW), StoreRead::Valid(_)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cli_requires_the_exact_first_argument() {
        let exe = "LimitScope.exe".to_string();
        assert_eq!(run_cli_if_requested(&[exe.clone()]), None);
        assert_eq!(run_cli_if_requested(&[exe.clone(), "--hidden".into()]), None);
        assert_eq!(
            run_cli_if_requested(&[exe.clone(), "--hidden".into(), INGEST_FLAG.into()]),
            None
        );
    }

    #[test]
    fn app_identifier_matches_the_tauri_configuration() {
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], APP_IDENTIFIER);
    }

    #[test]
    fn ingest_directory_is_the_directory_the_app_resolves() {
        use tauri::Manager;
        let app = tauri::test::mock_builder()
            .build(tauri::generate_context!())
            .expect("mock app");
        let resolved = app.path().app_data_dir().expect("app data dir");
        assert_eq!(Some(resolved), data_dir());
    }

    #[test]
    fn setup_command_is_quoted_per_shell_and_never_unsafe() {
        let plain = setup_for_executable(Some(Path::new(r"C:\Users\someone\AppData\Local\LimitScope\LimitScope.exe")));
        assert_eq!(
            plain.command.as_deref(),
            Some("C:/Users/someone/AppData/Local/LimitScope/LimitScope.exe --claude-statusline-ingest")
        );
        assert_eq!(plain.command, plain.powershell_command);
        let spaced = setup_for_executable(Some(Path::new(r"C:\Program Files\LimitScope\LimitScope.exe")));
        assert_eq!(
            spaced.command.as_deref(),
            Some("\"C:/Program Files/LimitScope/LimitScope.exe\" --claude-statusline-ingest")
        );
        assert_eq!(
            spaced.powershell_command.as_deref(),
            Some("& \"C:/Program Files/LimitScope/LimitScope.exe\" --claude-statusline-ingest")
        );
        let snippet: Value = serde_json::from_str(spaced.settings_snippet.as_deref().unwrap()).unwrap();
        assert_eq!(snippet["statusLine"]["type"], "command");
        assert_eq!(snippet["statusLine"]["command"].as_str(), spaced.command.as_deref());
        for hostile in [
            r"C:\a'b\LimitScope.exe",
            r"C:\a$b\LimitScope.exe",
            r"C:\a;b\LimitScope.exe",
            r"C:\a&b\LimitScope.exe",
            "C:\\a`b\\LimitScope.exe",
            "C:\\a\"b\\LimitScope.exe",
            r"C:\a%b%\LimitScope.exe",
        ] {
            let setup = setup_for_executable(Some(Path::new(hostile)));
            assert_eq!((setup.command, setup.powershell_command, setup.settings_snippet), (None, None, None), "{hostile}");
        }
        assert_eq!(setup_for_executable(None).command, None);
    }
}
