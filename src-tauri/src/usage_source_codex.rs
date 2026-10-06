//! Codex local usage source — the second Usage Intelligence collector.
//!
//! Reads OpenAI Codex CLI rollout logs (`~/.codex/sessions/**` and
//! `~/.codex/archived_sessions/**`, discovery-verified on 2026-10-06) and
//! normalizes completed model requests into the source-neutral
//! [`UsageEvent`] model. Rollout files are foreign-owned, append-only
//! JSONL logs: this module opens them read-only, never writes, renames,
//! truncates, locks for writing, or touches their structure.
//!
//! # Opt-in and probing
//!
//! Collection is gated by the store's persisted `enabled` flag. While
//! disabled, [`collect`] returns before the Codex home is even resolved —
//! no path resolution, no stat, no directory enumeration, no file reads.
//!
//! # Record selection (verified against the live corpus, 2026-10-06)
//!
//! - Modern rollouts (`session_meta.cli_version >= 0.153.0`) carry
//!   top-level `token_usage_record` lines whose `payload.usage` is the
//!   per-response usage. `payload.turn_token_usage` and
//!   `payload.thread_token_usage` are cumulative mirrors and are never
//!   ingested. A legacy `event_msg`/`token_count` mirror in the same file
//!   is never ingested either — the top-level record wins.
//! - Legacy rollouts (`cli_version < 0.153.0`) carry only
//!   `event_msg`/`token_count` lines; the per-event numbers live in
//!   `payload.info.last_token_usage` (`total_token_usage` is a cumulative
//!   mirror and is never ingested). The policy is anchored to the file's
//!   own first-line version, so a scan interrupted between appended lines
//!   can never classify a mirror as a real event.
//! - `compacted.payload.latest_token_usage_record` compaction artifacts
//!   are excluded (empty response id, ambiguous additive semantics).
//! - `state_5.sqlite` is never read; it is not an additive source.
//!
//! # Attribution
//!
//! - Modern: `payload.turn_id` joins the `turn_context` with the same
//!   `turn_id` in the same file; that turn's model is authoritative. A
//!   record whose turn has no context, or whose turn context switched
//!   models mid-turn, is rejected (counted) — never guessed.
//! - Legacy: the most recent preceding `turn_context`'s model.
//! - Provider comes from the model string only: `provider/model` splits
//!   on the first `/`; bare `gpt-*` maps narrowly to the OpenAI/Codex
//!   normalized identity; any other bare model is classified explicitly
//!   `unknown`. Session-level provider metadata
//!   (`session_meta.model_provider`, thread settings) is NEVER used — it
//!   reports "openai" even for `xai/...` and `google-antigravity/...`
//!   models.
//!
//! # Token semantics (verified: 6,954-record live sample, 2026-10-06)
//!
//! - `total_tokens == input_tokens + output_tokens` (universal);
//! - `cached_input_tokens` and `cache_write_input_tokens` are contained
//!   in `input_tokens` (universal);
//! - `reasoning_output_tokens` is contained in `output_tokens` for
//!   OpenAI-native responses but NOT universally (some provider-qualified
//!   models report reasoning that exceeds output). Normalization follows
//!   the shared plane — input minus caches, output minus reasoning — and
//!   a record whose reasoning exceeds its output cannot be represented
//!   without invented numbers, so it is rejected whole (counted in the
//!   source diagnostics), never clamped.
//!
//! # Event identity and idempotence
//!
//! - Modern: `codex:{thread_id}:{turn_id}:{response_id}` (source-native).
//! - Legacy: `codex:{session_id}:{ordinal}` (the rollout line ordinal).
//!
//! Re-reading the same bytes therefore can never grow totals; overlap and
//! replay dedup by the owned event id.
//!
//! # Incremental contract (bounded local-log scanning)
//!
//! - First successful observation baselines every existing rollout file
//!   at its current EOF and imports zero historical events. The baseline
//!   reads only each file's first line (identity + format policy, in
//!   bounded stages up to [`FIRST_LINE_CAP`]) and is capped by
//!   [`BASELINE_FILE_CEILING`] — exceeding it fails closed rather than
//!   silently skipping files. Zero-byte files (created before their first
//!   write) are left untracked; they become ordinary new files once they
//!   grow.
//! - A persistent per-file cursor (byte offset + size/mtime witnesses +
//!   identity witness + a bounded tail of recent turn contexts) survives
//!   restarts inside the shared store. Files appearing after the baseline
//!   are read from byte 0; existing files are only ever read from their
//!   stored offset forward. The carried turn tail lets a usage record
//!   appended after the scan that saw its `turn_context` — the normal
//!   case whenever a cycle boundary falls inside a turn — still join its
//!   turn. A record whose turn context predates the baseline itself
//!   (turns in flight at the enable instant) is rejected: the baseline
//!   reads no file bodies.
//! - Each cycle re-enumerates the tree (metadata only), then reads at
//!   most [`SCAN_FILE_CEILING`] changed files, [`SCAN_BYTE_CEILING`] bytes
//!   (identity verification included) and accepts at most
//!   [`SCAN_EVENT_CEILING`] events. Hitting a bound commits the safe
//!   prefix — never past an incomplete trailing line — reports
//!   `scanCeiling`, and leaves the remainder for the next cycle: the
//!   cursor never jumps past unprocessed data.
//! - A tracked file that shrinks below its recorded offset (truncation)
//!   or whose first-line identity no longer matches (same-path
//!   replacement) fails the whole scan closed: no continuation guess, no
//!   silent restart from zero. The state is visible in diagnostics and
//!   recovers on the next clean pass.
//! - A single rollout line at or above [`LINE_CAP`] with no terminator
//!   fails the scan closed rather than being skipped or partially parsed.
//! - Files whose first line cannot be identified are refused: tracked at
//!   EOF, never read again, never ingested (the fail-closed direction for
//!   foreign or malformed files appearing in the Codex tree).
//! - Clearing the plane (shared `clear_usage_intelligence`) drops the
//!   Codex cursors with everything else; the next cycle re-baselines at
//!   the then-current EOFs and does not backfill.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use crate::usage_intelligence::{
    SourceFileCursor, SourceScan, SourceState, SourceTurnContext, UsageEvent,
    UsageIntelligenceStore, EVENT_EPOCH_FLOOR_MS, EVENT_FUTURE_TOLERANCE_MS, USAGE_SOURCE_CODEX,
};

/// Maximum number of rollout files the tree may hold. Enumeration above
/// the ceiling fails closed (no baseline, no incremental progress)
/// instead of silently skipping files.
pub const BASELINE_FILE_CEILING: usize = 8_192;

/// Maximum changed files read per incremental cycle.
pub const SCAN_FILE_CEILING: usize = 200;

/// Maximum bytes read per incremental cycle (identity verification
/// included). Exceeding it commits the safe prefix and defers the rest.
pub const SCAN_BYTE_CEILING: u64 = 8 * 1024 * 1024;

/// Maximum events accepted per incremental cycle.
pub const SCAN_EVENT_CEILING: usize = 5_000;

/// Escalating first-line read stages: most `session_meta` lines end
/// within the first stage, so the common cost per file stays tiny; the
/// later stages cover sessions whose meta line carries long payloads.
/// The last stage is the hard identity-read cap — a first line that has
/// not ended by there means the file cannot be identified.
const FIRST_LINE_STAGES: [usize; 4] = [
    4 * 1024,
    16 * 1024,
    64 * 1024,
    256 * 1024,
];

/// Per-file incremental region cap (a single file cannot consume more
/// than one cycle's byte budget).
const REGION_CAP: u64 = SCAN_BYTE_CEILING;

/// A single JSONL line at or above this size with no terminator fails the
/// scan closed: no real rollout record approaches it, so it is drift,
/// not data to reinterpret. The bound is reachable within one cycle's
/// byte budget after identity verification.
const LINE_CAP: usize = 4 * 1024 * 1024;

/// Codex CLI version from which rollouts carry top-level
/// `token_usage_record` lines. Verified against the live corpus: every
/// sampled file below 0.153.0 is legacy-only, every file at or above it
/// is modern-policy.
const MODERN_VERSION: (u64, u64, u64) = (0, 153, 0);

/// Directory-nesting guard for the rollout walk (real layouts are ~4
/// levels deep). Exceeding it fails closed rather than skipping.
const WALK_DEPTH_LIMIT: usize = 12;

/// Outcome of one Codex collection pass, for diagnostics and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexScanOutcome {
    /// The store is disabled; nothing was probed.
    Disabled,
    /// The baseline was established: `tracked` files recorded at their
    /// current EOF, `refused` unidentifiable files tracked-but-never-read.
    /// Zero historical events were imported.
    Baseline { tracked: usize, refused: usize },
    /// An incremental scan ran to completion.
    Collected {
        accepted: usize,
        duplicates: usize,
        rejected: usize,
        files_read: usize,
    },
    /// An incremental scan hit a per-cycle bound; the safe prefix was
    /// committed and the remainder deferred (never skipped).
    Bounded {
        accepted: usize,
        duplicates: usize,
        rejected: usize,
        files_read: usize,
    },
    /// Nothing changed since the last successful scan; no files were read.
    Unchanged,
    /// The Codex home has no rollout trees (Codex not installed or never
    /// run). A normal state, not an error surface.
    Absent,
    /// The rollout tree exceeds the file ceiling (or the walk depth
    /// limit); nothing was ingested and no cursor moved.
    CeilingExceeded,
    /// The scan failed closed (truncation, replacement, oversized line,
    /// I/O). The detail is display-safe; the cursors did not move.
    ReadFailure(String),
}

/// Runs one collection pass. The disabled gate is first: a disabled store
/// returns without resolving the Codex home.
pub fn collect(store: &UsageIntelligenceStore) -> CodexScanOutcome {
    if !store.enabled() {
        return CodexScanOutcome::Disabled;
    }
    let Some(home) = (store.codex_home_resolver())() else {
        store.mark_source_state(USAGE_SOURCE_CODEX, SourceState::SourceAbsent, None);
        return CodexScanOutcome::Absent;
    };
    let sessions = home.join("sessions");
    let archived = home.join("archived_sessions");
    if !sessions.is_dir() && !archived.is_dir() {
        store.mark_source_state(USAGE_SOURCE_CODEX, SourceState::SourceAbsent, None);
        return CodexScanOutcome::Absent;
    }
    let entries = match enumerate_rollouts(&sessions, &archived, BASELINE_FILE_CEILING) {
        Ok(entries) => entries,
        Err(WalkError::TooManyFiles(found)) => {
            store.mark_source_state(
                USAGE_SOURCE_CODEX,
                SourceState::ScanCeiling,
                Some(format!(
                    "the rollout tree holds {found} files above the {BASELINE_FILE_CEILING} ceiling; collection is paused fail-closed"
                )),
            );
            return CodexScanOutcome::CeilingExceeded;
        }
        Err(WalkError::DepthLimit) => {
            store.mark_source_state(
                USAGE_SOURCE_CODEX,
                SourceState::ScanCeiling,
                Some("the rollout tree is nested too deeply to walk safely".to_string()),
            );
            return CodexScanOutcome::CeilingExceeded;
        }
        Err(WalkError::Io) => {
            let detail =
                "the rollout tree could not be walked; collection is paused fail-closed"
                    .to_string();
            store.mark_source_state(USAGE_SOURCE_CODEX, SourceState::ReadFailure, Some(detail.clone()));
            return CodexScanOutcome::ReadFailure(detail);
        }
    };
    let baselined = store
        .cursor(USAGE_SOURCE_CODEX)
        .map(|cursor| cursor.baselined)
        .unwrap_or(false);
    if !baselined {
        establish_codex_baseline(store, &home, entries)
    } else {
        incremental_scan(store, &home, entries)
    }
}

// ---------- enumeration ----------

#[derive(Clone)]
struct FileEntry {
    rel_path: String,
    size: u64,
    mtime_ms: i64,
}

#[derive(Debug)]
enum WalkError {
    TooManyFiles(usize),
    DepthLimit,
    Io,
}

/// Walks both rollout trees, collecting `.jsonl` files (metadata only) in
/// deterministic path order. `ceiling` bounds the result — beyond it the
/// enumeration fails closed instead of silently skipping.
fn enumerate_rollouts(
    sessions: &Path,
    archived: &Path,
    ceiling: usize,
) -> Result<Vec<FileEntry>, WalkError> {
    let mut entries = Vec::new();
    walk_tree(sessions, "sessions", 0, &mut entries)?;
    walk_tree(archived, "archived_sessions", 0, &mut entries)?;
    if entries.len() > ceiling {
        return Err(WalkError::TooManyFiles(entries.len()));
    }
    entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(entries)
}

fn walk_tree(
    dir: &Path,
    prefix: &str,
    depth: usize,
    out: &mut Vec<FileEntry>,
) -> Result<(), WalkError> {
    if depth > WALK_DEPTH_LIMIT {
        return Err(WalkError::DepthLimit);
    }
    // A missing root (e.g. no archived_sessions yet) is a normal shape,
    // not an error; anything else that cannot be read fails closed.
    if !dir.is_dir() {
        return Ok(());
    }
    let reader = fs::read_dir(dir).map_err(|_| WalkError::Io)?;
    for entry in reader {
        let entry = entry.map_err(|_| WalkError::Io)?;
        let file_type = entry.file_type().map_err(|_| WalkError::Io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            walk_tree(&entry.path(), &format!("{prefix}/{name}"), depth + 1, out)?;
        } else if file_type.is_file() && name.ends_with(".jsonl") {
            let meta = entry.metadata().map_err(|_| WalkError::Io)?;
            out.push(FileEntry {
                rel_path: format!("{prefix}/{name}"),
                size: meta.len(),
                mtime_ms: mtime_ms(&meta),
            });
        }
    }
    Ok(())
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

// ---------- identity and format policy ----------

struct FileIdentity {
    session_id: String,
    modern: bool,
}

/// Parses the first line of a rollout file: its `session_meta` id (the
/// identity witness) and its record-format policy from `cli_version`.
/// `None` = the file cannot be identified (refused, never ingested).
fn parse_identity(line: &[u8]) -> Option<FileIdentity> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = value.get("payload")?;
    let session_id = payload.get("id")?.as_str()?.trim();
    if session_id.is_empty() {
        return None;
    }
    let version = payload.get("cli_version")?.as_str()?;
    let tuple = version_tuple(version)?;
    Some(FileIdentity {
        session_id: session_id.to_string(),
        modern: tuple >= MODERN_VERSION,
    })
}

/// Parses a `major.minor.patch` prefix of a CLI version string into a
/// comparable tuple (pre-release / build suffixes ignored, matching the
/// verified boundary: `0.153.0-alpha.5` files are modern).
fn version_tuple(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

enum FirstLine {
    /// The first complete (newline-terminated) line.
    Found(Vec<u8>),
    /// The file ended (or the cap was hit) before any newline.
    NoNewlineWithinCap,
    /// The cycle's byte budget ran out before the line did; retry later.
    BudgetExhausted,
    /// The file could not be opened or read.
    IoError,
}

/// Reads the first complete line of a file in bounded stages, counting
/// every byte against the scan budget.
fn read_first_line(path: &Path, budgets: &mut ScanBudgets) -> FirstLine {
    let mut accumulated: Vec<u8> = Vec::new();
    for &stage in FIRST_LINE_STAGES.iter() {
        if stage <= accumulated.len() {
            continue;
        }
        let want = (stage - accumulated.len()).min(budgets.remaining_bytes as usize);
        if want == 0 {
            return FirstLine::BudgetExhausted;
        }
        let chunk = match read_region(path, accumulated.len() as u64, want) {
            Ok(chunk) => chunk,
            Err(_) => return FirstLine::IoError,
        };
        let chunk_len = chunk.len();
        accumulated.extend_from_slice(&chunk);
        budgets.remaining_bytes -= chunk_len as u64;
        if let Some(pos) = accumulated.iter().position(|&b| b == b'\n') {
            accumulated.truncate(pos + 1);
            return FirstLine::Found(accumulated);
        }
        if chunk_len < want {
            // EOF before any newline: the file has no complete first line.
            return FirstLine::NoNewlineWithinCap;
        }
    }
    FirstLine::NoNewlineWithinCap
}

fn read_region(path: &Path, offset: u64, max: usize) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0u8; max];
    let mut filled = 0usize;
    while filled < max {
        let n = file.read(&mut buffer[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

// ---------- baseline (backlog skip) ----------

fn establish_codex_baseline(
    store: &UsageIntelligenceStore,
    home: &Path,
    entries: Vec<FileEntry>,
) -> CodexScanOutcome {
    let now_ms = store.now_ms();
    let mut files = Vec::with_capacity(entries.len());
    let mut refused = 0usize;
    let mut budgets = ScanBudgets::unbounded();
    for entry in &entries {
        if entry.size == 0 {
            // A created-but-not-yet-written rollout: nothing to baseline.
            // It becomes an ordinary new file once it grows.
            continue;
        }
        let path = home.join(&entry.rel_path);
        let identity = match read_first_line(&path, &mut budgets) {
            FirstLine::Found(line) => parse_identity(&line),
            // Unopenable at baseline: fail closed (no baseline is better
            // than one that cannot account for its own files).
            FirstLine::IoError => {
                let detail = "a rollout file could not be read during baseline; collection is paused fail-closed".to_string();
                store.mark_source_state(USAGE_SOURCE_CODEX, SourceState::ReadFailure, Some(detail.clone()));
                return CodexScanOutcome::ReadFailure(detail);
            }
            FirstLine::NoNewlineWithinCap | FirstLine::BudgetExhausted => None,
        };
        match identity {
            Some(id) => files.push(SourceFileCursor {
                path: entry.rel_path.clone(),
                offset: entry.size,
                size: entry.size,
                mtime_ms: entry.mtime_ms,
                identity: id.session_id,
                modern: id.modern,
                refused: false,
                turns: Vec::new(),
            }),
            None => {
                refused += 1;
                files.push(SourceFileCursor {
                    path: entry.rel_path.clone(),
                    offset: entry.size,
                    size: entry.size,
                    mtime_ms: entry.mtime_ms,
                    identity: String::new(),
                    modern: false,
                    refused: true,
                    turns: Vec::new(),
                });
            }
        }
    }
    let tracked = files.len() - refused;
    store.establish_baseline_with_files(USAGE_SOURCE_CODEX, now_ms, None, files);
    if refused > 0 {
        store.mark_source_state(
            USAGE_SOURCE_CODEX,
            SourceState::Ok,
            Some(format!(
                "{refused} rollout file(s) could not be identified; their usage is never ingested"
            )),
        );
    }
    CodexScanOutcome::Baseline { tracked, refused }
}

// ---------- incremental scan ----------

/// Turn contexts carried across scans (most recent last). A usage record
/// appended after the scan that saw its `turn_context` — the normal case
/// whenever a cycle boundary falls inside a turn — must still join its
/// turn, so the recent tail of turn identities persists in the file
/// cursor. 32 covers any real turn spacing by a wide margin.
const TURN_TAIL_LIMIT: usize = 32;

#[derive(Default)]
struct FileScanState {
    /// turn_id -> model, seeded from the cursor's carried tail and
    /// extended by this region's `turn_context` lines.
    turn_models: HashMap<String, String>,
    /// Turn ids in first-observation order (recency for the carry tail).
    turn_order: Vec<String>,
    /// Turns whose context switched models within the visible history:
    /// their usage is not attributable without guessing and is rejected.
    poisoned_turns: HashSet<String>,
    /// Most recent `turn_context` (the legacy fallback anchor).
    last_context: Option<(String, String)>,
}

impl FileScanState {
    fn from_carried(turns: &[SourceTurnContext]) -> Self {
        let mut state = Self::default();
        for turn in turns {
            state
                .turn_models
                .insert(turn.turn_id.clone(), turn.model.clone());
            state.turn_order.push(turn.turn_id.clone());
        }
        state.last_context = turns
            .last()
            .map(|turn| (turn.turn_id.clone(), turn.model.clone()));
        state
    }

    /// The bounded attribution tail to persist: the most recently
    /// observed turns, poisoned ones excluded (their usage must stay
    /// unattributable, never fall back to the first model).
    fn carried_tail(&self, limit: usize) -> Vec<SourceTurnContext> {
        let mut tail: Vec<SourceTurnContext> = Vec::new();
        for id in self.turn_order.iter().rev() {
            if tail.len() == limit {
                break;
            }
            if self.poisoned_turns.contains(id.as_str()) {
                continue;
            }
            tail.push(SourceTurnContext {
                turn_id: id.clone(),
                model: self.turn_models[id].clone(),
            });
        }
        tail.reverse();
        tail
    }
}

struct ScanBudgets {
    remaining_bytes: u64,
    accepted: usize,
    rejected: usize,
    events: Vec<UsageEvent>,
}

impl ScanBudgets {
    fn new() -> Self {
        Self {
            remaining_bytes: SCAN_BYTE_CEILING,
            accepted: 0,
            rejected: 0,
            events: Vec::new(),
        }
    }

    fn unbounded() -> Self {
        Self {
            remaining_bytes: u64::MAX,
            accepted: 0,
            rejected: 0,
            events: Vec::new(),
        }
    }
}

enum ScanAbort {
    /// A tracked file's identity no longer matches (replacement) or a
    /// tracked file shrank below its offset (truncation).
    Replaced,
    /// A single line exceeded [`LINE_CAP`] with no terminator.
    OversizedLine,
    /// A file could not be opened or read.
    Io,
    /// The byte budget ran out mid-file; retry the file next cycle.
    Deferred,
}

/// Outcome of processing one file: whether a per-cycle bound stopped it
/// with unprocessed bytes remaining (the scan must then report
/// `scanCeiling`, even when every pending file was read).
struct FileProgress {
    partial: bool,
}

#[derive(Debug)]
enum LineOutcome {
    /// Structural or deliberately excluded record (session_meta,
    /// turn_context, compacted artifacts, legacy mirrors, unknown types).
    Ignored,
    /// Malformed or unattributable usage record; counted, never guessed.
    Rejected,
    Event(UsageEvent),
}

fn incremental_scan(
    store: &UsageIntelligenceStore,
    home: &Path,
    entries: Vec<FileEntry>,
) -> CodexScanOutcome {
    let cursor = store
        .cursor(USAGE_SOURCE_CODEX)
        .expect("incremental scan requires a baselined cursor");
    let mut state: HashMap<String, SourceFileCursor> = cursor
        .files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    let now_ms = store.now_ms();
    let mut budgets = ScanBudgets::new();

    enum Pending {
        New(FileEntry),
        Resume(FileEntry, SourceFileCursor),
    }

    let mut to_read: Vec<Pending> = Vec::new();
    let mut mutated = false;
    for entry in &entries {
        match state.get(&entry.rel_path).cloned() {
            None if entry.size == 0 => {
                // Still empty: nothing to read yet, nothing to track.
            }
            None => to_read.push(Pending::New(entry.clone())),
            Some(tracked) if tracked.refused => {
                // Refused files are never read again, no matter how they
                // grow: their usage cannot be safely attributed.
            }
            Some(tracked) => {
                if entry.size < tracked.offset {
                    let detail = "a tracked rollout file shrank below its recorded offset (truncation or replacement); collection is paused fail-closed".to_string();
                    store.mark_source_state(USAGE_SOURCE_CODEX, SourceState::ReadFailure, Some(detail.clone()));
                    return CodexScanOutcome::ReadFailure(detail);
                }
                if entry.size == tracked.offset {
                    if entry.mtime_ms == tracked.mtime_ms {
                        continue; // untouched: no new bytes to read
                    }
                    // Same length, mtime moved: a same-length rewrite is
                    // the one replacement shape the size check cannot
                    // catch — verify the identity witness.
                    match verify_identity(&home.join(&entry.rel_path), &tracked, &mut budgets) {
                        Ok(true) => {
                            state.insert(
                                entry.rel_path.clone(),
                                SourceFileCursor {
                                    mtime_ms: entry.mtime_ms,
                                    ..tracked
                                },
                            );
                            mutated = true;
                        }
                        // Budget ran out mid-verification: leave the file
                        // untouched in the cursor and retry next cycle.
                        Ok(false) => {}
                        Err(abort) => return fail_closed(store, abort),
                    }
                } else {
                    to_read.push(Pending::Resume(entry.clone(), tracked));
                }
            }
        }
    }
    // Files that vanished from the tree: drop their cursors (their
    // already-ingested events are owned history; a same-path reappearance
    // dedups by event id).
    let live: HashSet<&str> = entries.iter().map(|entry| entry.rel_path.as_str()).collect();
    let before = state.len();
    state.retain(|path, _| live.contains(path.as_str()));
    if state.len() != before {
        mutated = true;
    }

    if to_read.is_empty() && !mutated {
        return CodexScanOutcome::Unchanged;
    }

    let mut files_read = 0usize;
    let mut any_partial = false;
    for pending in &to_read {
        if budgets.accepted >= SCAN_EVENT_CEILING
            || budgets.remaining_bytes == 0
            || files_read >= SCAN_FILE_CEILING
        {
            break;
        }
        let result = match pending {
            Pending::New(entry) => read_new_file(home, entry, &mut budgets, &mut state, now_ms),
            Pending::Resume(entry, tracked) => {
                resume_file(home, entry, tracked, &mut budgets, &mut state, now_ms)
            }
        };
        match result {
            Ok(progress) => {
                files_read += 1;
                any_partial |= progress.partial;
            }
            Err(ScanAbort::Deferred) => {} // budget ran dry mid-file; retried next cycle
            Err(abort) => return fail_closed(store, abort),
        }
    }
    let bounded = files_read < to_read.len() || any_partial;

    let new_watermark = budgets
        .events
        .iter()
        .map(|event| event.event_at)
        .max()
        .unwrap_or(cursor.watermark_ms);
    let mut files: Vec<SourceFileCursor> = state.into_values().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let stats = store.apply_scan(
        USAGE_SOURCE_CODEX,
        SourceScan {
            watermark_ms: new_watermark,
            fingerprint: None,
            events: std::mem::take(&mut budgets.events),
            rejected: budgets.rejected,
            files,
        },
    );
    if bounded {
        let deferred = to_read.len() - files_read;
        store.mark_source_state(
            USAGE_SOURCE_CODEX,
            SourceState::ScanCeiling,
            Some(format!(
                "scan bounds reached; {deferred} changed rollout file(s) deferred to the next cycle"
            )),
        );
        CodexScanOutcome::Bounded {
            accepted: stats.accepted,
            duplicates: stats.duplicates,
            rejected: stats.rejected,
            files_read,
        }
    } else {
        CodexScanOutcome::Collected {
            accepted: stats.accepted,
            duplicates: stats.duplicates,
            rejected: stats.rejected,
            files_read,
        }
    }
}

fn fail_closed(store: &UsageIntelligenceStore, abort: ScanAbort) -> CodexScanOutcome {
    let detail = match abort {
        ScanAbort::Replaced => {
            "a tracked rollout file's identity no longer matches its cursor (replacement); collection is paused fail-closed"
        }
        ScanAbort::OversizedLine => {
            "a rollout line exceeds the maximum supported record size; collection is paused fail-closed"
        }
        ScanAbort::Io => "a rollout file could not be read; collection is paused fail-closed",
        ScanAbort::Deferred => unreachable!("deferred scans do not fail closed"),
    };
    store.mark_source_state(USAGE_SOURCE_CODEX, SourceState::ReadFailure, Some(detail.to_string()));
    CodexScanOutcome::ReadFailure(detail.to_string())
}

/// Verifies a tracked file's identity witness before trusting any of its
/// content. `Ok(false)` = budget ran out mid-verification (retry later;
/// the cursor is left untouched so the file is re-verified next cycle).
fn verify_identity(
    path: &Path,
    tracked: &SourceFileCursor,
    budgets: &mut ScanBudgets,
) -> Result<bool, ScanAbort> {
    match read_first_line(path, budgets) {
        FirstLine::Found(line) => {
            let identity = parse_identity(&line).ok_or(ScanAbort::Replaced)?;
            if identity.session_id != tracked.identity || identity.modern != tracked.modern {
                Err(ScanAbort::Replaced)
            } else {
                Ok(true)
            }
        }
        FirstLine::NoNewlineWithinCap => Err(ScanAbort::Replaced),
        FirstLine::BudgetExhausted => Ok(false),
        FirstLine::IoError => Err(ScanAbort::Io),
    }
}

/// First observation of a file that did not exist at baseline: it is read
/// from byte 0 (its whole content is post-baseline by definition).
fn read_new_file(
    home: &Path,
    entry: &FileEntry,
    budgets: &mut ScanBudgets,
    state: &mut HashMap<String, SourceFileCursor>,
    now_ms: i64,
) -> Result<FileProgress, ScanAbort> {
    let path = home.join(&entry.rel_path);
    let identity = match read_first_line(&path, budgets) {
        FirstLine::Found(line) => parse_identity(&line),
        FirstLine::NoNewlineWithinCap => None,
        FirstLine::BudgetExhausted => return Err(ScanAbort::Deferred),
        FirstLine::IoError => return Err(ScanAbort::Io),
    };
    let Some(identity) = identity else {
        // Unidentifiable: refuse — track at EOF, never read, never
        // ingested. Re-reading it from zero every cycle would starve the
        // byte budget; refusing is the fail-closed direction.
        state.insert(
            entry.rel_path.clone(),
            SourceFileCursor {
                path: entry.rel_path.clone(),
                offset: entry.size,
                size: entry.size,
                mtime_ms: entry.mtime_ms,
                identity: String::new(),
                modern: false,
                refused: true,
                turns: Vec::new(),
            },
        );
        return Ok(FileProgress { partial: false });
    };
    read_region_and_ingest(&path, entry, identity, 0, Vec::new(), budgets, state, now_ms)
}

/// A tracked file with appended bytes: identity is re-verified first, then
/// only the appended region is read.
fn resume_file(
    home: &Path,
    entry: &FileEntry,
    tracked: &SourceFileCursor,
    budgets: &mut ScanBudgets,
    state: &mut HashMap<String, SourceFileCursor>,
    now_ms: i64,
) -> Result<FileProgress, ScanAbort> {
    let path = home.join(&entry.rel_path);
    match read_first_line(&path, budgets) {
        FirstLine::Found(line) => {
            let identity = parse_identity(&line).ok_or(ScanAbort::Replaced)?;
            if identity.session_id != tracked.identity || identity.modern != tracked.modern {
                return Err(ScanAbort::Replaced);
            }
        }
        FirstLine::NoNewlineWithinCap => return Err(ScanAbort::Replaced),
        FirstLine::BudgetExhausted => return Err(ScanAbort::Deferred),
        FirstLine::IoError => return Err(ScanAbort::Io),
    }
    let identity = FileIdentity {
        session_id: tracked.identity.clone(),
        modern: tracked.modern,
    };
    let carried = tracked.turns.clone();
    read_region_and_ingest(
        &path,
        entry,
        identity,
        tracked.offset,
        carried,
        budgets,
        state,
        now_ms,
    )
}

/// Reads `[start_offset, entry.size)` (bounded), ingests every complete
/// line deterministically, and commits the cursor only to the end of the
/// last processed line — never past an incomplete trailing line.
#[allow(clippy::too_many_arguments)]
fn read_region_and_ingest(
    path: &Path,
    entry: &FileEntry,
    identity: FileIdentity,
    start_offset: u64,
    carried: Vec<SourceTurnContext>,
    budgets: &mut ScanBudgets,
    state: &mut HashMap<String, SourceFileCursor>,
    now_ms: i64,
) -> Result<FileProgress, ScanAbort> {
    let unread = entry.size.saturating_sub(start_offset);
    let want = unread.min(budgets.remaining_bytes).min(REGION_CAP);
    if want == 0 {
        return Err(ScanAbort::Deferred);
    }
    let buffer = read_region(path, start_offset, want as usize).map_err(|_| ScanAbort::Io)?;
    budgets.remaining_bytes -= buffer.len() as u64;
    // A region cut short by the byte budget or the per-file cap leaves
    // unprocessed bytes: the scan must surface that, never label the
    // cycle complete.
    let byte_bounded = want < unread;

    // Attribution is seeded from the carried tail so a record appended
    // after the scan that saw its turn context still joins its turn.
    let mut file_state = FileScanState::from_carried(&carried);
    let scan_end = buffer
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|pos| pos + 1)
        .unwrap_or(0);
    let mut committed = start_offset;
    let mut line_start = 0usize;
    let mut event_stopped = false;
    for (index, &byte) in buffer[..scan_end].iter().enumerate() {
        if byte != b'\n' {
            continue;
        }
        let line = &buffer[line_start..index];
        line_start = index + 1;
        if line.iter().all(|&b| b.is_ascii_whitespace()) {
            // Blank lines are consumed deterministically.
            committed = start_offset + line_start as u64;
            continue;
        }
        // The event ceiling stops the file at a safe line boundary; the
        // unprocessed remainder is deferred, never skipped.
        if budgets.accepted >= SCAN_EVENT_CEILING {
            event_stopped = true;
            break;
        }
        match ingest_line(line, &mut file_state, &identity, now_ms) {
            LineOutcome::Ignored => {}
            LineOutcome::Rejected => budgets.rejected += 1,
            LineOutcome::Event(event) => {
                budgets.accepted += 1;
                budgets.events.push(event);
            }
        }
        committed = start_offset + line_start as u64;
    }
    if scan_end == 0 && buffer.len() >= LINE_CAP {
        // A full region with no complete line is one unterminated record
        // at (or beyond) the size no real rollout line reaches.
        return Err(ScanAbort::OversizedLine);
    }

    state.insert(
        entry.rel_path.clone(),
        SourceFileCursor {
            path: entry.rel_path.clone(),
            offset: committed,
            size: entry.size,
            mtime_ms: entry.mtime_ms,
            identity: identity.session_id,
            modern: identity.modern,
            refused: false,
            turns: file_state.carried_tail(TURN_TAIL_LIMIT),
        },
    );
    Ok(FileProgress {
        partial: byte_bounded || event_stopped,
    })
}

// ---------- record ingestion ----------

fn ingest_line(
    line: &[u8],
    file_state: &mut FileScanState,
    identity: &FileIdentity,
    now_ms: i64,
) -> LineOutcome {
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return LineOutcome::Rejected;
    };
    let Some(record_type) = value.get("type").and_then(Value::as_str) else {
        return LineOutcome::Ignored;
    };
    match record_type {
        "turn_context" => {
            record_turn_context(&value, file_state);
            LineOutcome::Ignored
        }
        "token_usage_record" => {
            if identity.modern {
                modern_usage_event(&value, file_state, now_ms)
            } else {
                // Modern records in a legacy-policy file do not occur
                // (verified boundary); ignore rather than reinterpret.
                LineOutcome::Ignored
            }
        }
        "event_msg" => {
            let is_token_count = value
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(Value::as_str)
                == Some("token_count");
            if !is_token_count {
                return LineOutcome::Ignored;
            }
            if identity.modern {
                // The legacy mirror of a modern record: never ingested
                // alongside its twin (top-level token_usage_record wins).
                LineOutcome::Ignored
            } else {
                legacy_usage_event(&value, file_state, &identity.session_id, now_ms)
            }
        }
        // `compacted` embeds `latest_token_usage_record`, a compaction
        // artifact with an empty response id and ambiguous additive
        // semantics — excluded by policy. session_meta, response_item,
        // world_state and unknown types carry no owned usage.
        _ => LineOutcome::Ignored,
    }
}

fn record_turn_context(value: &Value, file_state: &mut FileScanState) {
    let Some(payload) = value.get("payload") else {
        return;
    };
    let Some(turn_id) = str_field(payload, "turn_id") else {
        return;
    };
    match str_field(payload, "model") {
        Some(model) => {
            let existing = file_state.turn_models.get(turn_id.as_str()).cloned();
            match existing {
                None => {
                    file_state.turn_models.insert(turn_id.clone(), model.clone());
                    file_state.turn_order.push(turn_id.clone());
                }
                Some(previous) if previous != model => {
                    // Mid-turn model switch: drift this reader refuses to
                    // attribute. The first mapping stands; the turn's
                    // usage is rejected.
                    file_state.poisoned_turns.insert(turn_id.clone());
                }
                Some(_) => {}
            }
            file_state.last_context = Some((turn_id, model));
        }
        // A turn context without a usable model cannot anchor the legacy
        // fallback; fail closed until a valid context appears.
        None => file_state.last_context = None,
    }
}

fn modern_usage_event(value: &Value, file_state: &FileScanState, now_ms: i64) -> LineOutcome {
    let Some(payload) = value.get("payload") else {
        return LineOutcome::Rejected;
    };
    let (Some(thread_id), Some(turn_id), Some(response_id)) = (
        str_field(payload, "thread_id"),
        str_field(payload, "turn_id"),
        str_field(payload, "response_id"),
    ) else {
        return LineOutcome::Rejected;
    };
    // Only the per-response `usage` is the event: turn/thread totals are
    // cumulative mirrors and would multiply every count.
    let Some(usage) = payload.get("usage").and_then(parse_tokens) else {
        return LineOutcome::Rejected;
    };
    let Some(model) = file_state.turn_models.get(turn_id.as_str()) else {
        return LineOutcome::Rejected; // no turn context in this file
    };
    if file_state.poisoned_turns.contains(turn_id.as_str()) {
        return LineOutcome::Rejected;
    }
    finish_event(
        format!("{USAGE_SOURCE_CODEX}:{thread_id}:{turn_id}:{response_id}"),
        model,
        &usage,
        value,
        now_ms,
    )
}

fn legacy_usage_event(
    value: &Value,
    file_state: &FileScanState,
    session_id: &str,
    now_ms: i64,
) -> LineOutcome {
    let Some(info) = value
        .get("payload")
        .and_then(|payload| payload.get("info"))
        .filter(|info| info.is_object())
    else {
        return LineOutcome::Rejected; // aborted / empty token_count
    };
    let Some(usage) = info.get("last_token_usage").and_then(parse_tokens) else {
        return LineOutcome::Rejected;
    };
    let Some(model) = file_state
        .last_context
        .as_ref()
        .map(|(_, model)| model.as_str())
    else {
        return LineOutcome::Rejected; // no preceding turn context
    };
    let Some(ordinal) = value
        .get("ordinal")
        .and_then(Value::as_i64)
        .filter(|n| *n >= 0)
    else {
        return LineOutcome::Rejected;
    };
    if session_id.is_empty() {
        return LineOutcome::Rejected;
    }
    finish_event(
        format!("{USAGE_SOURCE_CODEX}:{session_id}:{ordinal}"),
        model,
        &usage,
        value,
        now_ms,
    )
}

/// Applies the verified token identities and the shared validation to
/// build the final event, or rejects the record whole.
fn finish_event(
    id: String,
    model: &str,
    usage: &CodexTokens,
    value: &Value,
    now_ms: i64,
) -> LineOutcome {
    if usage.total != usage.input + usage.output
        || usage.cached + usage.cache_write > usage.input
        || usage.reasoning > usage.output
    {
        return LineOutcome::Rejected;
    }
    let Some(attribution) = attribute(model) else {
        return LineOutcome::Rejected;
    };
    let Some(event_at) = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp_ms)
    else {
        return LineOutcome::Rejected;
    };
    if event_at < EVENT_EPOCH_FLOOR_MS || event_at - now_ms > EVENT_FUTURE_TOLERANCE_MS {
        return LineOutcome::Rejected;
    }
    LineOutcome::Event(UsageEvent {
        id,
        source: USAGE_SOURCE_CODEX.to_string(),
        provider: attribution.provider,
        source_provider_id: attribution.source_provider_id,
        model: attribution.model,
        event_at,
        observed_at: now_ms,
        input_tokens: (usage.input - usage.cached - usage.cache_write) as u64,
        cache_read_tokens: usage.cached as u64,
        cache_write_tokens: usage.cache_write as u64,
        output_tokens: (usage.output - usage.reasoning) as u64,
        reasoning_tokens: usage.reasoning as u64,
        total_tokens: usage.total as u64,
    })
}

struct CodexTokens {
    input: i64,
    cached: i64,
    cache_write: i64,
    output: i64,
    reasoning: i64,
    total: i64,
}

/// Reads the six token dimensions; a missing or non-integer field rejects
/// (source schema drift is never guessed into numbers).
fn parse_tokens(value: &Value) -> Option<CodexTokens> {
    let field = |key: &str| -> Option<i64> {
        let raw = value.get(key)?.as_i64()?;
        if raw < 0 {
            return None;
        }
        Some(raw)
    };
    Some(CodexTokens {
        input: field("input_tokens")?,
        cached: field("cached_input_tokens")?,
        cache_write: field("cache_write_input_tokens")?,
        output: field("output_tokens")?,
        reasoning: field("reasoning_output_tokens")?,
        total: field("total_tokens")?,
    })
}

struct Attribution {
    provider: String,
    model: String,
    source_provider_id: String,
}

/// Provider attribution from the model string alone — the only
/// trustworthy source. Qualified `provider/model` splits on the first
/// `/`; bare `gpt-*` maps narrowly to the OpenAI/Codex identity; every
/// other bare model stays explicitly `unknown` rather than guessed.
fn attribute(raw: &str) -> Option<Attribution> {
    let model = raw.trim();
    if model.is_empty() {
        return None;
    }
    if let Some((prefix, rest)) = model.split_once('/') {
        let rest = rest.trim();
        if !is_provider_identifier(prefix) || rest.is_empty() {
            return None; // qualified but malformed: reject, never guess
        }
        return Some(Attribution {
            provider: prefix.to_string(),
            model: rest.to_string(),
            source_provider_id: model.to_string(),
        });
    }
    if model.starts_with("gpt-") {
        return Some(Attribution {
            provider: "openai-codex".to_string(),
            model: model.to_string(),
            source_provider_id: String::new(),
        });
    }
    Some(Attribution {
        provider: "unknown".to_string(),
        model: model.to_string(),
        source_provider_id: String::new(),
    })
}

fn is_provider_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    let raw = value.get(key)?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(raw.to_string())
}

fn parse_timestamp_ms(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|dt| dt.timestamp_millis())
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_intelligence::{UsageRange, USAGE_SOURCE_ZCODE};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW_MS: i64 = 1_791_288_000_000; // 2026-10-06T12:00:00.000Z
    const SESSION: &str = "0f0f0f0f-1111-2222-3333-444444444444";
    const THREAD: &str = "aaaa1111-1111-2222-3333-444444444444";
    const TURN_A: &str = "bbbb1111-1111-2222-3333-444444444444";
    const TURN_B: &str = "bbbb2222-1111-2222-3333-444444444444";

    fn fixed_clock() -> Box<dyn Fn() -> i64 + Send + Sync> {
        Box::new(|| NOW_MS)
    }

    mod tempdir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn path(&self) -> &PathBuf {
                &self.path
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        pub fn temp_dir(tag: &str) -> TempDir {
            let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "limitscope-usage-codex-test-{}-{}-{}",
                tag,
                std::process::id(),
                unique
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }
    }

    // ---------- synthetic rollout fixtures (no real content) ----------

    struct Dims {
        input: i64,
        cached: i64,
        cache_write: i64,
        output: i64,
        reasoning: i64,
    }

    impl Dims {
        fn total(&self) -> i64 {
            self.input + self.output
        }

        fn usage_json(&self) -> String {
            format!(
                "{{\"input_tokens\":{},\"cached_input_tokens\":{},\"cache_write_input_tokens\":{},\
                 \"output_tokens\":{},\"reasoning_output_tokens\":{},\"total_tokens\":{}}}",
                self.input,
                self.cached,
                self.cache_write,
                self.output,
                self.reasoning,
                self.total()
            )
        }
    }

    fn standard_dims() -> Dims {
        // Mirrors the verified overlap shape: cached ⊆ input, total ==
        // input + output.
        Dims {
            input: 46_129,
            cached: 39_040,
            cache_write: 0,
            output: 1_377,
            reasoning: 0,
        }
    }

    fn session_meta_line(version: &str) -> String {
        format!(
            "{{\"timestamp\":\"2026-10-06T08:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\
             \"id\":\"{SESSION}\",\"session_id\":\"{SESSION}\",\"cli_version\":\"{version}\",\
             \"originator\":\"codex\",\"model_provider\":\"openai\"}}}}"
        )
    }

    fn turn_context_line(turn: &str, model: &str) -> String {
        format!(
            "{{\"timestamp\":\"2026-10-06T09:00:00.000Z\",\"type\":\"turn_context\",\"payload\":{{\
             \"turn_id\":\"{turn}\",\"model\":\"{model}\",\"cwd\":\"C:/synthetic\",\"approval_policy\":\"never\"}}}}"
        )
    }

    fn token_usage_record_line(turn: &str, response: &str, dims: &Dims) -> String {
        format!(
            "{{\"timestamp\":\"2026-10-06T09:01:00.000Z\",\"type\":\"token_usage_record\",\"payload\":{{\
             \"thread_id\":\"{THREAD}\",\"turn_id\":\"{turn}\",\"session_id\":\"{SESSION}\",\
             \"response_id\":\"{response}\",\"usage\":{}}}}}",
            dims.usage_json()
        )
    }

    /// A modern record with the cumulative mirrors present, like the live
    /// schema; only `usage` may ever be ingested.
    fn token_usage_record_with_mirrors(turn: &str, response: &str, dims: &Dims) -> String {
        let cumulative = Dims {
            input: dims.input * 3,
            cached: dims.cached * 3,
            cache_write: 0,
            output: dims.output * 3,
            reasoning: dims.reasoning * 3,
        };
        format!(
            "{{\"timestamp\":\"2026-10-06T09:01:00.000Z\",\"type\":\"token_usage_record\",\"payload\":{{\
             \"thread_id\":\"{THREAD}\",\"turn_id\":\"{turn}\",\"session_id\":\"{SESSION}\",\
             \"response_id\":\"{response}\",\"usage\":{},\
             \"turn_token_usage\":{},\"thread_token_usage\":{}}}}}",
            dims.usage_json(),
            cumulative.usage_json(),
            cumulative.usage_json()
        )
    }

    fn token_count_line(ordinal: u64, dims: Option<&Dims>) -> String {
        let info = match dims {
            Some(dims) => format!(
                "\"info\":{{\"last_token_usage\":{},\"total_token_usage\":{}}}",
                dims.usage_json(),
                dims.usage_json()
            ),
            None => "\"info\":null".to_string(),
        };
        format!(
            "{{\"timestamp\":\"2026-10-06T09:02:00.000Z\",\"ordinal\":{ordinal},\"type\":\"event_msg\",\
             \"payload\":{{\"type\":\"token_count\",{info},\"rate_limits\":null}}}}"
        )
    }

    fn compacted_line() -> String {
        format!(
            "{{\"timestamp\":\"2026-10-06T09:03:00.000Z\",\"type\":\"compacted\",\"payload\":{{\
             \"message\":\"synthetic compaction\",\"window_number\":1,\
             \"latest_token_usage_record\":{{\"thread_id\":\"{THREAD}\",\"turn_id\":\"{TURN_A}\",\
             \"response_id\":\"\",\"usage\":{}}}}}}}",
            standard_dims().usage_json()
        )
    }

    fn rollout_rel_path(name: &str) -> String {
        format!("sessions/2026/10/06/{name}")
    }

    fn write_rollout(home: &Path, rel_path: &str, lines: &[String]) -> PathBuf {
        let path = home.join(rel_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        path
    }

    fn modern_rollout_lines(turn: &str, response: &str) -> Vec<String> {
        vec![
            session_meta_line("0.154.0"),
            turn_context_line(turn, "xai/grok-4.6"),
            token_usage_record_line(turn, response, &standard_dims()),
        ]
    }

    fn codex_home() -> tempdir::TempDir {
        tempdir::temp_dir("codex-home")
    }

    fn store_at(tag: &str, home: Option<PathBuf>) -> (UsageIntelligenceStore, tempdir::TempDir) {
        let dir = tempdir::temp_dir(tag);
        let store = UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        )
        .with_codex_home_resolver(Box::new(move || home.clone()));
        (store, dir)
    }

    fn enable(store: &UsageIntelligenceStore) {
        store.set_enabled(true);
    }

    fn cursor_files(store: &UsageIntelligenceStore) -> Vec<SourceFileCursor> {
        store
            .cursor(USAGE_SOURCE_CODEX)
            .map(|cursor| cursor.files)
            .unwrap_or_default()
    }

    // 1. disabled = zero probes: the Codex home is never resolved.
    #[test]
    fn disabled_collect_never_resolves_the_codex_home() {
        let dir = tempdir::temp_dir("disabled");
        let probes = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = probes.clone();
        let store = UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        )
        .with_codex_home_resolver(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            None
        }));
        let outcome = collect(&store);
        assert_eq!(outcome, CodexScanOutcome::Disabled);
        assert_eq!(probes.load(Ordering::SeqCst), 0, "no probe while disabled");
    }

    // 2. no rollout trees: source-absent, a normal state, no baseline.
    #[test]
    fn missing_rollout_trees_report_source_absent() {
        let home = codex_home();
        let (store, _dir) = store_at("absent", Some(home.path().to_path_buf()));
        enable(&store);
        let outcome = collect(&store);
        assert_eq!(outcome, CodexScanOutcome::Absent);
        let cursor = store
            .cursor(USAGE_SOURCE_CODEX)
            .expect("diagnostics record exists");
        assert!(!cursor.baselined);
        assert_eq!(cursor.state, SourceState::SourceAbsent);
    }

    // 3. baseline: every existing file recorded at its EOF, zero history.
    #[test]
    fn baseline_records_files_at_eof_and_imports_zero_history() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        write_rollout(
            home.path(),
            "archived_sessions/2026/09/rollout-b.jsonl",
            &modern_rollout_lines(TURN_B, "resp_2"),
        );
        let (store, _dir) = store_at("baseline", Some(home.path().to_path_buf()));
        enable(&store);
        let outcome = collect(&store);
        assert_eq!(outcome, CodexScanOutcome::Baseline { tracked: 2, refused: 0 });
        assert!(
            store.events().is_empty(),
            "no historical event may be imported"
        );
        assert_eq!(store.collection_started_at(), Some(NOW_MS));
        let files = cursor_files(&store);
        assert_eq!(files.len(), 2);
        for file in &files {
            assert!(file.offset > 0, "cursor sits at the file's EOF");
            assert_eq!(file.offset, file.size);
            assert!(!file.refused);
        }
        // The aggregate stays empty: the backlog was skipped, not zeroed.
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.events_in_range, 0);
        assert_eq!(dto.sources.len(), 1);
        assert_eq!(dto.sources[0].source, USAGE_SOURCE_CODEX);
    }

    // 4. unidentifiable files are refused: tracked at EOF, never read.
    #[test]
    fn baseline_refuses_unidentifiable_files_and_never_reads_them() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-good.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-junk.jsonl"),
            &["this is not json at all".to_string()],
        );
        let (store, _dir) = store_at("refused", Some(home.path().to_path_buf()));
        enable(&store);
        let outcome = collect(&store);
        assert_eq!(outcome, CodexScanOutcome::Baseline { tracked: 1, refused: 1 });
        let junk = cursor_files(&store)
            .into_iter()
            .find(|file| file.path.ends_with("rollout-junk.jsonl"))
            .unwrap();
        assert!(junk.refused);
        assert!(junk.identity.is_empty());
        let cursor = store.cursor(USAGE_SOURCE_CODEX).unwrap();
        assert!(
            cursor
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("1 rollout file"),
            "the refusal is surfaced in diagnostics"
        );
        // Growing the refused file still never reads it.
        let junk_path = home.path().join(rollout_rel_path("rollout-junk.jsonl"));
        std::fs::write(
            &junk_path,
            format!(
                "{}\n{}\n",
                "still not json",
                token_usage_record_line(TURN_A, "resp_9", &standard_dims())
            ),
        )
        .unwrap();
        let outcome = collect(&store);
        assert_eq!(outcome, CodexScanOutcome::Unchanged);
        assert!(store.events().is_empty());
    }

    // 5. new files after baseline are read from byte 0.
    #[test]
    fn new_file_after_baseline_is_read_from_zero() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("new-file", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-new.jsonl"),
            &[
                session_meta_line("0.154.0"),
                turn_context_line(TURN_B, "xai/grok-4.6"),
                token_usage_record_line(TURN_B, "resp_2", &standard_dims()),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected {
            accepted, files_read, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        assert_eq!(files_read, 1);
        let events = store.events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].id,
            format!("codex:{THREAD}:{TURN_B}:resp_2"),
            "source-native identity"
        );
    }

    // 6. appended records are ingested; the pre-baseline prefix never is.
    #[test]
    fn appended_records_are_ingested_without_reingesting_history() {
        let home = codex_home();
        let path = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("append", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        let pre_baseline = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            format!(
                "{pre_baseline}{}\n{}\n",
                turn_context_line(TURN_B, "xai/grok-4.6"),
                token_usage_record_line(TURN_B, "resp_2", &standard_dims())
            ),
        )
        .unwrap();
        let outcome = collect(&store);
        let CodexScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1, "only the appended record is ingested");
        let events = store.events();
        assert_eq!(events.len(), 1);
        assert!(events[0].id.ends_with("resp_2"));
    }

    // 7. an incomplete trailing line never advances the cursor; the
    //    completed line is ingested later.
    #[test]
    fn incomplete_trailing_line_is_safe_and_ingests_once_completed() {
        let home = codex_home();
        let path = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("trailing", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        let offset_before = cursor_files(&store)[0].offset;
        // A fresh turn starts (context line completes), then its usage
        // record lands torn across the scan boundary.
        let record = token_usage_record_line(TURN_B, "resp_2", &standard_dims());
        let split_at = record.len() / 2;
        {
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            use std::io::Write;
            // The context line completes (with its newline); the usage
            // record lands torn across the scan boundary.
            writeln!(file, "{}", turn_context_line(TURN_B, "xai/grok-4.6")).unwrap();
            write!(file, "{}", &record[..split_at]).unwrap();
        }
        let outcome = collect(&store);
        let CodexScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0, "a torn line is not ingested");
        // The cursor advanced only past the completed context line — it
        // never moved into the incomplete record.
        let ctx_len = turn_context_line(TURN_B, "xai/grok-4.6").len() as u64 + 1;
        assert_eq!(cursor_files(&store)[0].offset, offset_before + ctx_len);
        // The writer completes the line.
        {
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            use std::io::Write;
            write!(file, "{}\n", &record[split_at..]).unwrap();
        }
        let outcome = collect(&store);
        let CodexScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1, "the completed line is ingested exactly once");
        assert_eq!(store.events().len(), 1);
    }

    // 8. the byte ceiling commits a safe prefix and the remainder is
    //    picked up exactly (no silent cursor skip, no duplicates).
    #[test]
    fn byte_ceiling_commits_safe_prefix_and_never_skips_data() {
        let home = codex_home();
        let path = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("byte-ceiling", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        // ~900 lines x ~10 KB each: far above the 8 MiB cycle budget but
        // below the 5,000-event ceiling, so the byte bound is the one hit.
        let mut lines = vec![turn_context_line(TURN_B, "xai/grok-4.6")];
        for index in 0..900 {
            lines.push(format!(
                "{{\"timestamp\":\"2026-10-06T09:04:00.000Z\",\"type\":\"token_usage_record\",\"pad\":\"{}\",\
                 \"payload\":{{\"thread_id\":\"{THREAD}\",\"turn_id\":\"{TURN_B}\",\
                 \"response_id\":\"resp_big_{index}\",\"usage\":{}}}}}",
                "p".repeat(9_000),
                standard_dims().usage_json()
            ));
        }
        let existing = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{}{}\n", existing, lines.join("\n"))).unwrap();

        let mut total_accepted = 0usize;
        for _ in 0..10 {
            let outcome = collect(&store);
            match outcome {
                CodexScanOutcome::Bounded { accepted, .. } => total_accepted += accepted,
                CodexScanOutcome::Collected { accepted, .. } => {
                    total_accepted += accepted;
                    break;
                }
                other => panic!("unexpected outcome during bounded drain: {other:?}"),
            }
        }
        assert_eq!(
            total_accepted, 900,
            "every deferred record is eventually ingested exactly once"
        );
        assert_eq!(store.events().len(), 900);
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(
            dto.totals.total_tokens,
            (900 * standard_dims().total()) as u64,
            "bounded scans never grow or shrink totals"
        );
    }

    // 9. the event ceiling bounds a cycle; the rest drains exactly.
    #[test]
    fn event_ceiling_bounds_a_cycle_and_drains_exactly() {
        let home = codex_home();
        let path = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("event-ceiling", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        let mut lines = vec![turn_context_line(TURN_B, "xai/grok-4.6")];
        for index in 0..6_000 {
            lines.push(token_usage_record_line(
                TURN_B,
                &format!("resp_{index}"),
                &standard_dims(),
            ));
        }
        let existing = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{}{}\n", existing, lines.join("\n"))).unwrap();

        let first = collect(&store);
        let CodexScanOutcome::Bounded {
            accepted: first_accepted,
            ..
        } = first
        else {
            panic!("expected a bounded scan, got {first:?}");
        };
        assert_eq!(first_accepted, SCAN_EVENT_CEILING);
        let second = collect(&store);
        let CodexScanOutcome::Collected {
            accepted: second_accepted,
            duplicates,
            ..
        } = second
        else {
            panic!("expected a completion, got {second:?}");
        };
        assert_eq!(second_accepted, 6_000 - SCAN_EVENT_CEILING);
        assert_eq!(duplicates, 0, "no record is ingested twice");
        assert_eq!(store.events().len(), 6_000);
    }

    // 10. the file ceiling defers whole files; nothing is skipped.
    #[test]
    fn file_ceiling_defers_remaining_files_without_skipping() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("file-ceiling", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        for index in 0..SCAN_FILE_CEILING + 5 {
            write_rollout(
                home.path(),
                &rollout_rel_path(&format!("rollout-flood-{index}.jsonl")),
                &[
                    session_meta_line("0.154.0"),
                    turn_context_line(TURN_B, "xai/grok-4.6"),
                    token_usage_record_line(TURN_B, &format!("resp_{index}"), &standard_dims()),
                ],
            );
        }
        let first = collect(&store);
        let CodexScanOutcome::Bounded {
            files_read: first_read,
            accepted: first_accepted,
            ..
        } = first
        else {
            panic!("expected a bounded scan, got {first:?}");
        };
        assert_eq!(first_read, SCAN_FILE_CEILING);
        assert_eq!(first_accepted, SCAN_FILE_CEILING);
        let second = collect(&store);
        let CodexScanOutcome::Collected {
            files_read: second_read,
            accepted: second_accepted,
            ..
        } = second
        else {
            panic!("expected a completion, got {second:?}");
        };
        assert_eq!(second_read, 5);
        assert_eq!(second_accepted, 5);
        assert_eq!(store.events().len(), SCAN_FILE_CEILING + 5);
    }

    // 11. truncation fails closed, preserves cursors, and recovers.
    #[test]
    fn truncated_file_fails_closed_and_recovers_after_repair() {
        let home = codex_home();
        let path = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let original = std::fs::read_to_string(&path).unwrap();
        let (store, _dir) = store_at("truncation", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        // Same session id, but shorter: a truncation, not an append.
        std::fs::write(&path, &original[..original.len() - 40]).unwrap();
        let outcome = collect(&store);
        assert!(matches!(outcome, CodexScanOutcome::ReadFailure(_)));
        let files = cursor_files(&store);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].offset, files[0].size,
            "the cursor is preserved, not rewound to zero"
        );
        assert!(
            store.events().is_empty(),
            "nothing is ingested from a failed-closed scan"
        );
        // Repairing the file (restoring + appending a fresh turn)
        // recovers collection.
        std::fs::write(
            &path,
            format!(
                "{original}{}\n{}\n",
                turn_context_line(TURN_B, "xai/grok-4.6"),
                token_usage_record_line(TURN_B, "resp_2", &standard_dims())
            ),
        )
        .unwrap();
        let outcome = collect(&store);
        let CodexScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected recovery, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
    }

    // 12. a same-path replacement (different session id) fails closed
    //     instead of being read as new content.
    #[test]
    fn replaced_file_fails_closed() {
        let home = codex_home();
        let path = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("replacement", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        // A "different" session entirely, at the same path, longer.
        let foreign = vec![
            session_meta_line("0.154.0").replace(SESSION, "99999999-1111-2222-3333-444444444444"),
            turn_context_line(TURN_B, "xai/grok-4.6"),
            token_usage_record_line(TURN_B, "resp_foreign", &standard_dims()),
        ];
        std::fs::write(&path, format!("{}\n", foreign.join("\n"))).unwrap();
        let outcome = collect(&store);
        assert!(matches!(outcome, CodexScanOutcome::ReadFailure(_)));
        assert!(
            store.events().is_empty(),
            "replaced content is never ingested as continuation"
        );
    }

    // 13. modern attribution: exact turn join across a multi-model file.
    #[test]
    fn modern_records_join_their_turn_across_a_multi_model_session() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("join", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        let mixed_dims = Dims {
            input: 1_000,
            cached: 200,
            cache_write: 100,
            output: 500,
            reasoning: 120,
        };
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "aaaaaaaa-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                turn_context_line(TURN_B, "gpt-5.4"),
                token_usage_record_line(TURN_A, "resp_3", &standard_dims()),
                token_usage_record_line(TURN_B, "resp_4", &mixed_dims),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 2);
        let events = store.events();
        assert_eq!(events.len(), 2);
        let grok = events.iter().find(|e| e.id.ends_with("resp_3")).unwrap();
        assert_eq!(grok.provider, "xai");
        assert_eq!(grok.model, "grok-4.6");
        assert_eq!(grok.source_provider_id, "xai/grok-4.6");
        let gpt = events.iter().find(|e| e.id.ends_with("resp_4")).unwrap();
        assert_eq!(gpt.provider, "openai-codex");
        assert_eq!(gpt.model, "gpt-5.4");
        assert_eq!(gpt.input_tokens, 700);
        assert_eq!(gpt.cache_read_tokens, 200);
        assert_eq!(gpt.cache_write_tokens, 100);
        assert_eq!(gpt.output_tokens, 380);
        assert_eq!(gpt.reasoning_tokens, 120);
        assert_eq!(gpt.total_tokens, 1_500);
    }

    // 14. unknown bare models are classified explicitly, never guessed.
    #[test]
    fn unknown_bare_model_is_classified_unknown() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("unknown-provider", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "bbbbbbbb-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "mystery-model-v9"),
                token_usage_record_line(TURN_A, "resp_2", &standard_dims()),
            ],
        );
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Collected { accepted: 1, .. }
        ));
        let event = &store.events()[0];
        assert_eq!(event.provider, "unknown");
        assert_eq!(event.model, "mystery-model-v9");
    }

    // 15. a record whose turn has no context is rejected, not guessed.
    #[test]
    fn unjoinable_modern_record_is_rejected() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("unjoinable", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "cccccccc-1111-2222-3333-444444444444"),
                // No turn_context for this turn at all.
                token_usage_record_line(TURN_B, "resp_orphan", &standard_dims()),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0);
        assert_eq!(rejected, 1);
        assert!(store.events().is_empty());
    }

    // 16. a mid-turn model switch poisons the turn: usage is rejected.
    #[test]
    fn mid_turn_model_switch_rejects_that_turns_usage() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("poison", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "dddddddd-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                turn_context_line(TURN_A, "gpt-5.4"),
                token_usage_record_line(TURN_A, "resp_switched", &standard_dims()),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0);
        assert_eq!(rejected, 1);
    }

    // 17. legacy rollouts: preceding-turn_context fallback attribution.
    #[test]
    fn legacy_records_fall_back_to_the_preceding_turn_context() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("legacy", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-legacy.jsonl"),
            &[
                session_meta_line("0.149.0").replace(SESSION, "eeeeeeee-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "gpt-5.1"),
                token_count_line(7, Some(&standard_dims())),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        let event = &store.events()[0];
        assert_eq!(
            event.id, "codex:eeeeeeee-1111-2222-3333-444444444444:7",
            "legacy identity is session + line ordinal"
        );
        assert_eq!(event.provider, "openai-codex", "bare gpt-* maps narrowly");
        assert_eq!(event.model, "gpt-5.1");
        assert_eq!(
            event.input_tokens,
            (standard_dims().input - standard_dims().cached) as u64
        );
        assert_eq!(event.total_tokens, standard_dims().total() as u64);
    }

    // 18. legacy records with no preceding context are rejected.
    #[test]
    fn legacy_record_without_preceding_context_is_rejected() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("legacy-orphan", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-legacy-orphan.jsonl"),
            &[
                session_meta_line("0.150.0").replace(SESSION, "ffffffff-1111-2222-3333-444444444444"),
                token_count_line(3, Some(&standard_dims())),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0);
        assert_eq!(rejected, 1);
    }

    // 19. a modern file's legacy mirror is never ingested (no double
    //     counting), and modern-only policy follows the file's version.
    #[test]
    fn modern_file_mirrors_are_never_ingested() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("mirror", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.153.1").replace(SESSION, "11111111-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                token_usage_record_line(TURN_A, "resp_modern", &standard_dims()),
                token_count_line(11, Some(&standard_dims())),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1, "only the modern record is ingested");
        assert_eq!(rejected, 0, "the mirror is ignored, not rejected");
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(
            dto.totals.total_tokens,
            standard_dims().total() as u64,
            "the mirror never doubles the total"
        );

        // A modern-version file with only token_count lines (observed in
        // the wild) contributes nothing: its mirrors have no twins here.
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-c.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "22222222-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                token_count_line(12, Some(&standard_dims())),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0, "modern-version mirrors stay un-ingested");
    }

    // 20. compaction artifacts are excluded.
    #[test]
    fn compacted_artifacts_are_excluded() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("compacted", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "33333333-1111-2222-3333-444444444444"),
                compacted_line(),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0, "the compaction artifact is never ingested");
        assert_eq!(rejected, 0, "it is a deliberate exclusion, not an error");
        assert!(store.events().is_empty());
    }

    // 21. state_5.sqlite is not an additive source and is never read.
    #[test]
    fn state_db_is_never_touched() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        std::fs::write(home.path().join("state_5.sqlite"), b"not a sqlite file").unwrap();
        let (store, _dir) = store_at("state-db", Some(home.path().to_path_buf()));
        enable(&store);
        let outcome = collect(&store);
        assert!(matches!(outcome, CodexScanOutcome::Baseline { .. }));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "44444444-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                token_usage_record_line(TURN_A, "resp_2", &standard_dims()),
            ],
        );
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Collected { accepted: 1, .. }
        ));
    }

    // 22. a new file that cannot be identified is refused and never
    //     re-read (no per-cycle read loop).
    #[test]
    fn new_unidentifiable_file_is_refused_and_not_looped() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("new-refused", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-junk.jsonl"),
            &["{{{{ not json".to_string()],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0);
        assert!(cursor_files(&store).iter().any(|file| file.refused));
        // The refused file grows: still never read, and the scan stays
        // idle instead of re-reading it every cycle.
        let junk = home.path().join(rollout_rel_path("rollout-junk.jsonl"));
        std::fs::write(
            &junk,
            format!(
                "more junk\n{}\n",
                token_usage_record_line(TURN_A, "resp_junk", &standard_dims())
            ),
        )
        .unwrap();
        let outcome = collect(&store);
        assert_eq!(outcome, CodexScanOutcome::Unchanged);
        assert!(store.events().is_empty());
    }

    // 23. replay safety: identical records re-offered under the same ids
    //     dedup; the parser's identity is byte-stable.
    #[test]
    fn event_identity_is_stable_and_replay_never_grows_totals() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("replay", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "55555555-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                token_usage_record_line(TURN_A, "resp_dup", &standard_dims()),
            ],
        );
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Collected { accepted: 1, .. }
        ));
        assert_eq!(store.events().len(), 1);
        let first_id = store.events()[0].id.clone();
        // Re-parse the same record: the identity is byte-stable.
        let mut state = FileScanState::default();
        state
            .turn_models
            .insert(TURN_A.to_string(), "xai/grok-4.6".to_string());
        let identity = FileIdentity {
            session_id: "x".to_string(),
            modern: true,
        };
        match ingest_line(
            token_usage_record_line(TURN_A, "resp_dup", &standard_dims()).as_bytes(),
            &mut state,
            &identity,
            NOW_MS,
        ) {
            LineOutcome::Event(event) => assert_eq!(event.id, first_id),
            other => panic!("expected an event, got {other:?}"),
        }
        // The idle source is not rescanned at all.
        let outcome = collect(&store);
        assert_eq!(outcome, CodexScanOutcome::Unchanged);
        assert_eq!(store.events().len(), 1, "no duplicates from overlap");
    }

    // 24. invalid and malformed records reject instead of guessing.
    #[test]
    fn invalid_records_reject_instead_of_guessing() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("invalid", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        let negative = token_usage_record_line(
            TURN_A,
            "resp_neg",
            &Dims { input: -5, cached: 0, cache_write: 0, output: 10, reasoning: 0 },
        );
        let total_mismatch = token_usage_record_line(TURN_A, "resp_total", &standard_dims())
            .replace(&standard_dims().total().to_string(), "999999");
        let reasoning_over = token_usage_record_line(
            TURN_A,
            "resp_reason",
            &Dims { input: 1_000, cached: 0, cache_write: 0, output: 10, reasoning: 50 },
        );
        let future = token_usage_record_line(TURN_A, "resp_future", &standard_dims())
            .replace("2026-10-06T09:01:00.000Z", "2026-10-06T23:01:00.000Z");
        let malformed = "{not json".to_string();
        let missing_usage = format!(
            "{{\"timestamp\":\"2026-10-06T09:01:00.000Z\",\"type\":\"token_usage_record\",\"payload\":{{\
             \"thread_id\":\"{THREAD}\",\"turn_id\":\"{TURN_A}\",\"response_id\":\"resp_nousage\"}}}}"
        );
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "66666666-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                negative,
                total_mismatch,
                reasoning_over,
                future,
                malformed,
                missing_usage,
                turn_context_line(TURN_B, "   "),
                token_usage_record_line(TURN_B, "resp_blankctx", &standard_dims()),
            ],
        );
        let outcome = collect(&store);
        let CodexScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0);
        assert_eq!(rejected, 7, "every invalid record is counted, none stored");
        assert!(store.events().is_empty());
    }

    // 25. token normalization: the verified overlap semantics.
    #[test]
    fn token_dimensions_normalize_without_double_counting() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("normalize", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "77777777-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                token_usage_record_with_mirrors(
                    TURN_A,
                    "resp_norm",
                    &Dims {
                        input: 1_000,
                        cached: 200,
                        cache_write: 100,
                        output: 500,
                        reasoning: 120,
                    },
                ),
            ],
        );
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Collected { accepted: 1, .. }
        ));
        let event = &store.events()[0];
        assert_eq!(event.input_tokens, 700, "input excludes both caches");
        assert_eq!(event.cache_read_tokens, 200);
        assert_eq!(event.cache_write_tokens, 100);
        assert_eq!(event.output_tokens, 380, "output excludes reasoning");
        assert_eq!(event.reasoning_tokens, 120);
        assert_eq!(event.total_tokens, 1_500, "the source's canonical total");
        assert_eq!(
            event.input_tokens
                + event.cache_read_tokens
                + event.cache_write_tokens
                + event.output_tokens
                + event.reasoning_tokens,
            event.total_tokens,
            "the dimensions decompose the total exactly once"
        );
    }

    // 26. source isolation: a Codex failure never touches ZCode data.
    #[test]
    fn codex_failure_leaves_zcode_intact() {
        let zcode_dir = tempdir::temp_dir("isolation-zcode");
        let db_path = zcode_dir.path().join("db.sqlite");
        {
            let connection = rusqlite::Connection::open(&db_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE model_usage (
                        id text primary key,
                        provider_id text not null,
                        model_id text not null,
                        status text not null,
                        completed_at integer,
                        input_tokens integer not null default 0,
                        output_tokens integer not null default 0,
                        reasoning_tokens integer not null default 0,
                        cache_creation_input_tokens integer not null default 0,
                        cache_read_input_tokens integer not null default 0,
                        computed_total_tokens integer not null default 0
                    );
                    INSERT INTO model_usage (id, provider_id, model_id, status, completed_at,
                        input_tokens, output_tokens, reasoning_tokens,
                        cache_creation_input_tokens, cache_read_input_tokens, computed_total_tokens)
                    VALUES ('z1', 'account:zai-plan', 'GLM-5.3', 'completed', 1791287000000,
                        100, 50, 0, 0, 0, 150);",
                )
                .unwrap();
        }
        let zcode_path = db_path.clone();
        let dir = tempdir::temp_dir("isolation-store");
        let home = codex_home();
        let home_path = home.path().to_path_buf();
        let store = UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(move || Some(zcode_path.clone())),
        )
        .with_codex_home_resolver(Box::new(move || Some(home_path.clone())));
        enable(&store);
        // ZCode collects (baseline) and stays healthy throughout.
        let zcode_outcome = crate::usage_source_zcode::collect(&store);
        assert!(matches!(
            zcode_outcome,
            crate::usage_source_zcode::ZcodeScanOutcome::Baseline { .. }
        ));
        let codex_rel = rollout_rel_path("rollout-a.jsonl");
        let codex_path = home.path().join(&codex_rel);
        write_rollout(home.path(), &codex_rel, &modern_rollout_lines(TURN_A, "resp_1"));
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        // Now Codex fails closed (truncation) while ZCode stays healthy.
        let codex_content = std::fs::read_to_string(&codex_path).unwrap();
        std::fs::write(&codex_path, &codex_content[..codex_content.len() - 30]).unwrap();
        assert!(matches!(collect(&store), CodexScanOutcome::ReadFailure(_)));
        let zcode_cursor = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert_eq!(zcode_cursor.state, SourceState::Ok, "ZCode is untouched");
        assert!(zcode_cursor.baselined);
        let codex_cursor = store.cursor(USAGE_SOURCE_CODEX).unwrap();
        assert_eq!(codex_cursor.state, SourceState::ReadFailure);
        // A later ZCode scan still works end to end.
        let _ = crate::usage_source_zcode::collect(&store);
        let zcode_cursor = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert_eq!(zcode_cursor.state, SourceState::Ok);
    }

    // 27. source isolation the other way: an absent ZCode never blocks
    //     Codex collection.
    #[test]
    fn zcode_failure_leaves_codex_intact() {
        let home = codex_home();
        let missing_db = tempdir::temp_dir("isolation-missing")
            .path()
            .join("db.sqlite");
        let missing = missing_db.clone();
        let dir = tempdir::temp_dir("isolation-store-2");
        let home_path = home.path().to_path_buf();
        let store = UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(move || Some(missing.clone())),
        )
        .with_codex_home_resolver(Box::new(move || Some(home_path.clone())));
        enable(&store);
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-new.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        assert_eq!(
            crate::usage_source_zcode::collect(&store),
            crate::usage_source_zcode::ZcodeScanOutcome::Absent
        );
        let outcome = collect(&store);
        assert!(matches!(outcome, CodexScanOutcome::Baseline { .. }));
        let codex_cursor = store.cursor(USAGE_SOURCE_CODEX).unwrap();
        assert!(codex_cursor.baselined);
        assert_eq!(codex_cursor.state, SourceState::Ok);
    }

    // 28. the shared clear re-baselines Codex at the current EOFs
    //     without backfilling, and foreign files stay untouched.
    #[test]
    fn clear_rebaselines_codex_without_backfill() {
        let home = codex_home();
        let path = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("clear", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        let original = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            format!(
                "{original}{}\n{}\n",
                turn_context_line(TURN_B, "xai/grok-4.6"),
                token_usage_record_line(TURN_B, "resp_2", &standard_dims())
            ),
        )
        .unwrap();
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Collected { accepted: 1, .. }
        ));
        assert_eq!(store.events().len(), 1);
        let bytes_before = std::fs::read(&path).unwrap();
        assert!(store.clear_owned());
        assert!(store.events().is_empty());
        assert_eq!(store.collection_started_at(), None);
        // The next cycle re-baselines at the CURRENT EOF: the record
        // written since the last ingest is backlog now, not history to
        // import.
        let outcome = collect(&store);
        assert!(matches!(
            outcome,
            CodexScanOutcome::Baseline { tracked: 1, refused: 0 }
        ));
        assert!(store.events().is_empty(), "no backfill after clear");
        // The foreign file is byte-identical through all of it.
        assert_eq!(std::fs::read(&path).unwrap(), bytes_before);
        // And collection continues forward from the new baseline.
        let rebaselined = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            format!(
                "{rebaselined}{}\n{}\n",
                turn_context_line(TURN_B, "xai/grok-4.6"),
                token_usage_record_line(TURN_B, "resp_3", &standard_dims())
            ),
        )
        .unwrap();
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Collected { accepted: 1, .. }
        ));
    }

    // 29. the enumeration ceiling fails closed (small injected ceiling).
    #[test]
    fn enumeration_above_the_ceiling_fails_closed() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &modern_rollout_lines(TURN_B, "resp_2"),
        );
        let error = enumerate_rollouts(
            &home.path().join("sessions"),
            &home.path().join("archived_sessions"),
            1,
        )
        .err()
        .expect("two files above a ceiling of one must fail");
        assert!(matches!(error, WalkError::TooManyFiles(2)));
        // And the production ceiling holds the real corpus shape.
        let ok = enumerate_rollouts(
            &home.path().join("sessions"),
            &home.path().join("archived_sessions"),
            BASELINE_FILE_CEILING,
        )
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert!(
            ok.windows(2).all(|pair| pair[0].rel_path < pair[1].rel_path),
            "enumeration is deterministic (path-sorted)"
        );
    }

    // 30. rollout files are byte-identical across collection passes.
    #[test]
    fn rollout_files_remain_byte_identical_after_collection() {
        let home = codex_home();
        let path_a = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let listing_before: Vec<String> = fs::read_dir(home.path().join("sessions/2026/10/06"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let (store, _dir) = store_at("bytes", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        let original = std::fs::read_to_string(&path_a).unwrap();
        std::fs::write(
            &path_a,
            format!(
                "{original}{}\n{}\n",
                turn_context_line(TURN_B, "xai/grok-4.6"),
                token_usage_record_line(TURN_B, "resp_2", &standard_dims())
            ),
        )
        .unwrap();
        let appended_state = std::fs::read(&path_a).unwrap();
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Collected { accepted: 1, .. }
        ));
        assert_eq!(
            std::fs::read(&path_a).unwrap(),
            appended_state,
            "the collector never rewrites rollout files"
        );
        let listing_after: Vec<String> = fs::read_dir(home.path().join("sessions/2026/10/06"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(listing_after, listing_before, "no files appear or vanish");
    }

    // 31. no conversational content is ever stored: the closed field
    //     list means only ids, model, timestamps and counts survive.
    #[test]
    fn events_carry_no_content_fields() {
        let home = codex_home();
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_0"),
        );
        let (store, _dir) = store_at("privacy", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        write_rollout(
            home.path(),
            &rollout_rel_path("rollout-b.jsonl"),
            &[
                session_meta_line("0.154.0").replace(SESSION, "88888888-1111-2222-3333-444444444444"),
                turn_context_line(TURN_A, "xai/grok-4.6"),
                token_usage_record_line(TURN_A, "resp_1", &standard_dims()),
            ],
        );
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Collected { accepted: 1, .. }
        ));
        let events = store.events();
        assert_eq!(events.len(), 1);
        let rendered = serde_json::to_string(&events).unwrap();
        for forbidden in ["payload", "message", "instructions", "cwd", "text", "content"] {
            assert!(
                !rendered.contains(forbidden),
                "the stored event must not carry {forbidden}"
            );
        }
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.events_in_range, 1);
    }

    // 32. an oversized single line fails closed instead of skipping.
    #[test]
    fn oversized_line_fails_closed() {
        let home = codex_home();
        let path = write_rollout(
            home.path(),
            &rollout_rel_path("rollout-a.jsonl"),
            &modern_rollout_lines(TURN_A, "resp_1"),
        );
        let (store, _dir) = store_at("oversized", Some(home.path().to_path_buf()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            CodexScanOutcome::Baseline { .. }
        ));
        // One record padded past LINE_CAP with no newline yet.
        let giant = format!(
            "{{\"timestamp\":\"2026-10-06T09:05:00.000Z\",\"type\":\"token_usage_record\",\"pad\":\"{}\"",
            "x".repeat(LINE_CAP + 1024)
        );
        {
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            use std::io::Write;
            write!(file, "{giant}").unwrap();
        }
        let outcome = collect(&store);
        assert!(matches!(outcome, CodexScanOutcome::ReadFailure(_)));
        assert!(
            store.events().is_empty(),
            "an oversized record is never partially ingested"
        );
    }

    // 33. version parsing for the modern boundary.
    #[test]
    fn version_tuple_parses_the_verified_shapes() {
        assert_eq!(version_tuple("0.152.1"), Some((0, 152, 1)));
        assert_eq!(version_tuple("0.153.0-alpha.5"), Some((0, 153, 0)));
        assert_eq!(version_tuple("0.154.0"), Some((0, 154, 0)));
        assert_eq!(version_tuple("1.0.0"), Some((1, 0, 0)));
        assert_eq!(version_tuple("garbage"), None);
        assert_eq!(version_tuple(""), None);
        assert!(version_tuple("0.153.0").unwrap() >= MODERN_VERSION);
        assert!(version_tuple("0.152.9").unwrap() < MODERN_VERSION);
    }

    // 34. provider attribution table.
    #[test]
    fn attribute_covers_the_contract_rules() {
        let qualified = attribute("xai/grok-4.6").unwrap();
        assert_eq!(qualified.provider, "xai");
        assert_eq!(qualified.model, "grok-4.6");
        assert_eq!(qualified.source_provider_id, "xai/grok-4.6");
        let nested = attribute("openrouter/z-ai-glm-5.3-flash").unwrap();
        assert_eq!(nested.provider, "openrouter");
        assert_eq!(nested.model, "z-ai-glm-5.3-flash");
        let gpt = attribute("gpt-5.4").unwrap();
        assert_eq!(gpt.provider, "openai-codex");
        assert_eq!(gpt.model, "gpt-5.4");
        let unknown = attribute("mystery-model").unwrap();
        assert_eq!(unknown.provider, "unknown");
        assert!(attribute("").is_none());
        assert!(attribute("/model").is_none());
        assert!(attribute("xai/").is_none());
    }

    // Live proof (task-contract item: bounded local live proof). Run
    // explicitly via `cargo test live_proof -- --ignored --nocapture`;
    // never part of the ordinary suite. Exercises the REAL collector
    // against the REAL Codex rollout tree on this machine with the opt-in
    // enabled only inside this test path, and proves:
    //
    // - the source resolves and the corpus shape is consistent with the
    //   discovery spike (thousands of files, both trees);
    // - the first observation baselines every file at its current EOF and
    //   imports ZERO historical events (no backlog, no fake zero usage);
    // - a second pass over the unchanged corpus is a no-op (Unchanged);
    // - rollout files are untouched by the pass (size + mtime witnesses
    //   identical across every enumerated file);
    // - no model/API request is made and no prompt/response content is
    //   printed or stored.
    #[test]
    #[ignore = "live proof: reads the real local Codex rollout tree (read-only)"]
    fn live_proof_baseline_skips_backlog() {
        let home = std::env::home_dir().expect("a home directory");
        let codex_home = home.join(".codex");
        assert!(
            codex_home.join("sessions").is_dir() || codex_home.join("archived_sessions").is_dir(),
            "the real Codex rollout tree is expected on this machine"
        );

        let dir = tempdir::temp_dir("codex-live-proof");
        let home_path = codex_home.clone();
        let store = crate::usage_intelligence::UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        )
        .with_codex_home_resolver(Box::new(move || Some(home_path.clone())));

        // Witnesses for every rollout file before the pass (metadata
        // only — the reader opens files read-only and must not disturb
        // them in any way).
        let collect_witness = || -> Vec<(String, u64, i64)> {
            let sessions = codex_home.join("sessions");
            let archived = codex_home.join("archived_sessions");
            enumerate_rollouts(&sessions, &archived, BASELINE_FILE_CEILING)
                .expect("the live corpus fits the ceiling")
                .into_iter()
                .map(|entry| (entry.rel_path, entry.size, entry.mtime_ms))
                .collect()
        };
        let before = collect_witness();
        assert!(
            before.len() > 100,
            "the live corpus shape matches discovery ({} files)",
            before.len()
        );

        // The explicit enable exists ONLY inside this test path.
        store.set_enabled(true);
        let outcome = collect(&store);
        let CodexScanOutcome::Baseline { tracked, refused } = outcome else {
            panic!("expected a baseline on the live source, got {outcome:?}");
        };
        assert_eq!(tracked + refused, before.len(), "every file is accounted for");
        assert!(
            store.events().is_empty(),
            "no historical event may be imported on the live baseline"
        );
        let cursor = store
            .cursor(USAGE_SOURCE_CODEX)
            .expect("the live cursor exists");
        assert!(cursor.baselined);
        assert_eq!(cursor.files.len(), tracked + refused);
        assert!(
            store.collection_started_at().is_some(),
            "the disclosure anchor is set by the baseline"
        );

        // An idle corpus costs nothing: the second pass reads nothing. A
        // concurrent Codex writer appending mid-proof legitimately
        // collects instead; either way history is never re-imported.
        let second = collect(&store);
        match second {
            CodexScanOutcome::Unchanged => {}
            other => println!("live proof: concurrent Codex activity produced {other:?}"),
        }

        // The source state is unchanged by the pass: every file's size
        // and mtime witness must be identical, except for files a live
        // Codex writer legitimately appended to between the witnesses
        // (grows only). A shrink would mean the collector disturbed the
        // source — the fixture tests carry the strict byte-identity proof.
        let after = collect_witness();
        let before_map: HashMap<String, (u64, i64)> =
            before.into_iter().map(|(p, s, m)| (p, (s, m))).collect();
        let mut grew = 0usize;
        for (path, size, mtime) in &after {
            let (size, mtime) = (*size, *mtime);
            match before_map.get(path) {
                Some(&(before_size, before_mtime)) => {
                    assert!(
                        size >= before_size && mtime >= before_mtime,
                        "rollout file {path} shrank or moved backwards: {before_size}/{before_mtime} -> {size}/{mtime}"
                    );
                    if size != before_size {
                        grew += 1;
                    }
                }
                None => println!("live proof: file created during the pass: {path}"),
            }
        }
        println!(
            "live proof: baselined {tracked} rollout files at EOF ({refused} refused), imported {} events, {grew} file(s) appended by a live writer, all files metadata-untouched by the collector",
            store.events().len(),
        );
    }
}
