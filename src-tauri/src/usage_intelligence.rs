//! Usage Intelligence — the v0.8.9 token-usage data plane.
//!
//! This is a SECOND, parallel local-data plane, deliberately separate from
//! the quota-history plane (`history.rs` / `quota-history-v1.json`):
//!
//! - quota history stores provider quota *percentages* (how full a window
//!   is) and feeds Usage Analytics and predictions;
//! - Usage Intelligence stores *reported token counts* per completed model
//!   request (input / cache read / cache write / output / reasoning /
//!   total) collected from local AI-tool databases, and feeds the
//!   Today / 7d / 30d token view.
//!
//! Nothing here extends or overloads `QuotaObservation`,
//! `quota-history-v1.json`, or any quota percentage. The two planes never
//! read each other's stores.
//!
//! # Ownership and honesty contract
//!
//! - Enabled sources are the ZCode local database (`~/.zcode/cli/db/db.sqlite`,
//!   table `model_usage`, read strictly read-only through a temporary
//!   snapshot copy — see `usage_source_zcode.rs`) and the OpenCode local database
//!   (`~/.local/share/opencode/opencode.db`, table `message`, read
//!   strictly read-only through a temporary snapshot copy — see
//!   `usage_source_opencode.rs`). Source files are never opened for
//!   writing, never checkpointed, never mutated. Each source keeps its
//!   own cursor under its own source id; one source failing never blocks
//!   or hides the others.
//! - Codex token collection is disabled because an owned event cannot be
//!   tied to the active session. Existing Codex events and cursor metadata
//!   remain persisted and inspectable, but every Usage Intelligence read
//!   projection excludes the entire source; new Codex rollouts are never read.
//! - Collection is strictly opt-in. While the persisted `enabled` flag is
//!   false the collector does not even resolve the source path — no stat,
//!   no existence probe, no open, no copy, no schema inspection
//!   (docs/local-data-controls-v0.7.md §14, rule 7).
//! - Backlog is never imported. On first enable (and after a clear, while
//!   still enabled) the collector establishes a baseline cursor at the
//!   source's current high-water mark and imports nothing; only rows
//!   completed after that point are ever ingested. The baseline writes no
//!   fake zero usage. Re-enabling after a disable re-establishes a fresh
//!   baseline per source the same way: usage completed while collection
//!   was disabled is never silently imported, while already-collected
//!   events and totals are preserved.
//! - An incremental window larger than one scan's row bound is processed
//!   in bounded, deterministic batches: each scan commits its prefix
//!   (persisting the composite `(stamp, id)` resume position), surfaces
//!   the deferral through the source's `scanCeiling` diagnostics, and
//!   continues from the saved position on subsequent scans — nothing is
//!   skipped and the cursor never stalls on an oversized backlog.
//! - An enabled source can stop before reading anything. That fail-closed
//!   pause is a separate `collectionPaused` state, never `scanCeiling`: no
//!   new events are collected, so its range coverage is uncertain rather
//!   than complete.
//! - Re-ingesting an overlap is idempotent: events dedup by their stable
//!   owned identity, so re-running a collector can never grow totals.
//! - Retention is bounded (35 days, plus a hard event cap) and only ever
//!   deletes LimitScope-owned events in this store's own file. The window
//!   is a property of the store, not a side effect of a successful scan:
//!   it is applied when the file is opened (startup) and at the start of
//!   every collection cycle, so an idle, disabled, or failing source still
//!   ages the owned history out.
//! - The event model deliberately carries no cost, no project, no prompt
//!   or response text, no conversation titles, no task names, and no tool
//!   call content. The ZCode reader never selects the source's raw usage
//!   or metadata JSON columns.
//!
//! # Token semantics (verified against the live schema 2026-10-06)
//!
//! ZCode's `model_usage` dimensions overlap: `computed_total_tokens`
//! equals `input_tokens + output_tokens` in every completed row observed
//! (28,873 / 28,873), `input_tokens` already contains
//! `cache_read_input_tokens` (input >= cache read everywhere), and
//! `provider_total_tokens` mirrors `computed_total_tokens`. Summing the
//! five raw dimensions would double-count. Normalization therefore stores
//! non-overlapping dimensions:
//!
//! - `input_tokens`        = source input − cache read − cache write
//! - `cache_read_tokens`   = source cache read
//! - `cache_write_tokens`  = source cache creation
//! - `output_tokens`       = source output − reasoning
//! - `reasoning_tokens`    = source reasoning
//! - `total_tokens`        = the source's own canonical
//!   `computed_total_tokens` (verified == input + output)
//!
//! A row whose subtraction would go negative, or whose
//! `computed_total_tokens` differs from `input + output`, is rejected
//! whole (fail closed) rather than stored with guessed numbers. Zero is
//! preserved as measured zero: the source dimensions are NOT NULL, so a
//! stored 0 is a reported 0, never a stand-in for an absent value.
//!
//! # Storage
//!
//! One bounded JSON file (`usage-intelligence-v1.json`) in the Tauri
//! app-data directory, written temp-then-rename like the sibling stores.
//! Malformed supported data is preserved before recovery. Unknown schemas
//! remain untouched and write-blocked, with collection disabled.
//!
//! # Daily activity projection
//!
//! `aggregate` additionally exposes one cell per local calendar day
//! touched by the selected range (`daily`), bucketing the same in-range
//! events the totals aggregate — no second pass over sources, no schema
//! migration, and the daily sums always reconcile with `totals`. Days
//! without recorded events are `observed: false` with `totalTokens`
//! absent on the wire (missing is never a serialized zero), and days
//! wholly before the local collection start carry `beforeCollection`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use chrono::TimeZone;

use crate::local_data::LocalDataClearOutcome;
use crate::runtime::RuntimeHandle;

/// Versioned file name, alongside the other app-data artifacts.
pub const USAGE_INTELLIGENCE_FILE_NAME: &str = "usage-intelligence-v1.json";

const BLOB_VERSION: u32 = 1;

/// Wire schema version of the aggregation DTO.
pub const USAGE_INTELLIGENCE_SCHEMA_VERSION: u32 = 1;

/// Owned retention: events older than 35 days (relative to the injected
/// clock) are pruned. 35 covers the 30-day view with margin, mirroring the
/// bounded-retention rule of the local-source contract.
pub const USAGE_EVENT_RETENTION_MS: i64 = 35 * 24 * 60 * 60_000;

/// Hard cap on stored events — a fail-safe ceiling far above what 35 days
/// of real usage produces (~1k events/day on an active machine). A store
/// beyond the cap refuses new events rather than growing unbounded.
pub const USAGE_EVENT_MAX: usize = 200_000;

/// Fixed wire category of the owned clear.
pub const USAGE_INTELLIGENCE_CATEGORY: &str = "usageIntelligence";

/// The stable source id of the ZCode local database reader.
pub const USAGE_SOURCE_ZCODE: &str = "zcode";

/// The stable source id of the Codex rollout-log reader.
pub const USAGE_SOURCE_CODEX: &str = "codex";

/// The stable source id of the OpenCode database reader.
pub const USAGE_SOURCE_OPENCODE: &str = "opencode";

/// Local database sources whose last persisted `Ok` cursor cannot attest to
/// work attempted after that write. Each restart requires one fresh,
/// durably committed scan before the cursor can be reported healthy again.
const RESTART_REVALIDATION_SOURCES: &[&str] = &[USAGE_SOURCE_ZCODE, USAGE_SOURCE_OPENCODE];

/// Clock-skew tolerance for source event timestamps: a row completed
/// further than this in the future is rejected as implausible.
pub(crate) const EVENT_FUTURE_TOLERANCE_MS: i64 = 5 * 60_000;

/// Sanity floor for source event timestamps (2020-01-01): anything older
/// is treated as an invalid timestamp, not as real history.
pub(crate) const EVENT_EPOCH_FLOOR_MS: i64 = 1_577_836_800_000;

// ---------- event model ----------

/// One completed model request's reported token usage, normalized to
/// non-overlapping dimensions. Source-neutral: the ZCode reader produces
/// it today, and future Codex / OpenCode readers produce the same shape
/// without a schema migration for these fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    /// Stable owned identity: `"{source}:{source_event_id}"`. The dedup
    /// key — re-ingesting the same source row can never add a second copy.
    pub id: String,
    /// Harness identity of the collecting source (`"zcode"` today;
    /// `"codex"` / `"opencode"` are reserved). Internal + diagnostics
    /// only; never a user-facing grouping dimension in the MVP.
    pub source: String,
    /// Normalized provider axis (e.g. `zai`), separate from the model.
    pub provider: String,
    /// The source's raw provider string (e.g.
    /// `account:zai-individual-coding-plan`), kept verbatim for
    /// provenance/diagnostics only.
    pub source_provider_id: String,
    /// Model id exactly as the source reports it (e.g. `GLM-5.3`).
    pub model: String,
    /// When the request completed, epoch milliseconds (source-native).
    pub event_at: i64,
    /// When LimitScope ingested the event, epoch milliseconds (owned
    /// clock). Never later re-derived.
    pub observed_at: i64,
    /// Non-cached input tokens (source input minus cache read and write).
    pub input_tokens: u64,
    /// Tokens served from cache (cache read).
    pub cache_read_tokens: u64,
    /// Tokens written to cache (cache creation).
    pub cache_write_tokens: u64,
    /// Output tokens excluding reasoning.
    pub output_tokens: u64,
    /// Reasoning tokens.
    pub reasoning_tokens: u64,
    /// The source's own canonical total (ZCode: `computed_total_tokens`,
    /// verified == input + output). Never the sum of provider total plus
    /// dimensions.
    pub total_tokens: u64,
}

impl UsageEvent {
    /// Validates a candidate event with the store's final rules: blank
    /// identifiers, implausible timestamps, and identity/source mismatch
    /// reject; the dimensions are unsigned by construction. Returns the
    /// event unchanged or `None`.
    fn validated(event: UsageEvent, now_ms: i64) -> Option<UsageEvent> {
        if event.id.trim().is_empty() || event.model.trim().is_empty() {
            return None;
        }
        if event.provider.trim().is_empty() || event.source.trim().is_empty() {
            return None;
        }
        if !event.id.starts_with(&format!("{}:", event.source)) {
            return None;
        }
        if event.event_at < EVENT_EPOCH_FLOOR_MS
            || event.event_at - now_ms > EVENT_FUTURE_TOLERANCE_MS
        {
            return None;
        }
        if event.observed_at < EVENT_EPOCH_FLOOR_MS
            || event.observed_at > now_ms + EVENT_FUTURE_TOLERANCE_MS
        {
            return None;
        }
        Some(event)
    }
}

// ---------- source cursors and diagnostics ----------

/// Diagnostic state of one source's last collection attempt. `Disabled`
/// and `Collecting` are derived at read time (from the enabled flag and
/// the in-flight latch); the persisted variants are the durable outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceState {
    /// The source database was not present at the expected path.
    SourceAbsent,
    /// The database exists but its schema is not one this reader
    /// understands (missing table or required columns). Never partially
    /// interpreted.
    SchemaUnsupported,
    /// The read failed (I/O, locked, corrupt). Display-safe detail rides
    /// separately.
    ReadFailure,
    /// The incremental window held more rows than one scan's bound; the
    /// scan committed a bounded, deterministic prefix (its events are
    /// stored and its resume position persisted) and deferred the
    /// remainder to subsequent scans. Progress continues; nothing is
    /// skipped.
    ScanCeiling,
    /// The source could not be enumerated safely and the scan stopped
    /// before reading anything (e.g. the Codex rollout tree exceeds its
    /// file ceiling or walk-depth limit). Collection is paused fail-closed:
    /// unlike `ScanCeiling`, no prefix was committed and no progress is
    /// being made, so range coverage is uncertain until a clean pass.
    CollectionPaused,
    /// The last scan completed successfully (at `last_observed_at`).
    Ok,
}

/// Cheap change-detection fingerprint of the source files, so an idle
/// source costs no snapshot copy. Only trusted after a successful scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFingerprint {
    pub db_size: u64,
    pub db_mtime_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_mtime_ms: Option<i64>,
}

/// The composite read position of the last row of a committed batch: the
/// row's cursor-column stamp plus its id, defining a lexicographic
/// `(stamp, id)` order over eligible rows. A scan window admits every row
/// from its floor forward except the already-processed prefix at exactly
/// `(stamp_ms, id <= id)`, so a batch that ends mid-tie (thousands of rows
/// sharing one stamp) still resumes at the exact next row instead of
/// re-reading — or skipping — the tie group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBatchEnd {
    pub stamp_ms: i64,
    pub id: String,
}

/// Per-source incremental cursor + diagnostics, persisted in the owned
/// store file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCursor {
    pub source: String,
    /// High-water mark: the max ingested (or baselined) `completed_at`
    /// epoch-ms. The incremental window re-reads from
    /// `max(baseline_watermark_ms, watermark_ms - lookback)` inclusive, so
    /// rows sharing the boundary timestamp and late commits are picked up
    /// and deduped — while never reaching below the baseline into the
    /// pre-enable backlog.
    ///
    /// Meaningful only together with `baselined`: a cursor record created
    /// purely to surface a pre-baseline failure (source absent, schema
    /// unsupported) has no baseline yet and must not be mistaken for one —
    /// otherwise the first successful observation would import history
    /// from watermark 0 instead of skipping the backlog.
    pub watermark_ms: i64,
    /// The immutable backlog boundary: rows completed strictly before this
    /// epoch-ms are pre-enable history and are never ingested, no matter
    /// how far the lookback would otherwise reach. Equals the watermark at
    /// baseline time and never moves afterwards.
    #[serde(default)]
    pub baseline_watermark_ms: i64,
    /// Whether the backlog-skipping baseline was established for this
    /// source. False only for failure-only cursor records.
    #[serde(default)]
    pub baselined: bool,
    /// Set by the disable → re-enable transition: usage completed while
    /// collection was disabled must not be silently imported, so the next
    /// collection pass must establish a fresh baseline at the source's
    /// then-current high-water mark before any incremental read. Cleared
    /// only by that successful re-baseline; failures leave it set so a
    /// later pass retries the fresh baseline — never a stale watermark.
    #[serde(default)]
    pub pending_rebaseline: bool,
    /// The composite `(stamp, id)` end of the last committed batch
    /// (row-count-bounded sources: ZCode, OpenCode). `None` while every
    /// scan so far drained its whole window (the inclusive lookback floor
    /// alone resumes correctly) or for file-based sources (Codex tracks
    /// per-file byte cursors instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_end: Option<SourceBatchEnd>,
    /// When the baseline (backlog skip) was established, epoch ms. Also
    /// anchors the "collected locally since" disclosure.
    pub baselined_at: i64,
    /// When the last successful observation completed, epoch ms.
    pub last_observed_at: i64,
    /// Outcome of the last attempted scan.
    pub state: SourceState,
    /// Display-safe detail for the last non-ok state, when useful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Fingerprint of the source files at the last successful scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<SourceFingerprint>,
    /// Counters of the last scan (accepted / duplicate / rejected rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan: Option<ScanStats>,
    /// Per-file progress for file-based sources (Codex rollouts today);
    /// empty for snapshot-based sources like ZCode. Replaced wholesale by
    /// each applied scan of the owning source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SourceFileCursor>,
}

/// Row-level counters of one scan, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    /// Events newly stored by this scan.
    pub accepted: usize,
    /// Source rows skipped because their event was already stored.
    pub duplicates: usize,
    /// Source rows rejected by validation (malformed / invariant drift).
    pub rejected: usize,
    /// Source rows dispositioned as "usage not reported": completed,
    /// error-free rows whose every token dimension is zero — the verified
    /// sparse-reporting pattern of some providers, never stored as
    /// trustworthy measured zero. Zero and omitted for sources that never
    /// produce it (ZCode, Codex).
    #[serde(default, skip_serializing_if = "usage_not_reported_is_zero")]
    pub usage_not_reported: usize,
}

fn usage_not_reported_is_zero(value: &usize) -> bool {
    *value == 0
}

/// Carried attribution state for file-based sources (the Codex reader
/// today): the most recent turn identities observed in a rollout file, so
/// a usage record appended after a scan boundary can still join the
/// `turn_context` of its turn. Bounded — the reader keeps the last N
/// entries per file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTurnContext {
    pub turn_id: String,
    pub model: String,
}

/// Per-file progress of a file-based source (the Codex rollout reader
/// today; field semantics are the Codex reader's). Persisted inside the
/// owning source's [`SourceCursor`] so a file cursor survives restarts.
///
/// Files are foreign-owned append-only logs: the cursor records how far
/// into each file this plane has read, plus the witnesses needed to
/// detect truncation and same-path replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFileCursor {
    /// Source-relative path with forward slashes (e.g.
    /// `sessions/2026/10/06/rollout-….jsonl`) — the tracking key.
    pub path: String,
    /// Byte offset just past the last fully processed (accepted,
    /// duplicated, rejected, or deliberately ignored) complete line.
    /// Never advanced past an incomplete trailing line.
    pub offset: u64,
    /// The file's size at the last observation, to detect truncation
    /// (size < offset fails the scan closed) and skip untouched files.
    pub size: u64,
    /// The file's mtime at the last observation, to skip untouched files
    /// cheaply and to notice same-length rewrites.
    pub mtime_ms: i64,
    /// Identity witness read from the file's first line (Codex: the
    /// `session_meta` id). A file whose witness no longer matches fails
    /// the scan closed instead of being re-read as "new" content.
    pub identity: String,
    /// Source-defined record-format policy witness (Codex: the rollout is
    /// modern, `session_meta.cli_version >= 0.153.0`, so top-level
    /// `token_usage_record` lines win and legacy mirrors are ignored).
    pub modern: bool,
    /// The file was observed but could not be identified (no parseable
    /// first-line identity): it is tracked at EOF, never read, and its
    /// usage is never ingested — the fail-closed direction for foreign or
    /// malformed files appearing in the source tree.
    #[serde(default)]
    pub refused: bool,
    /// Carried turn contexts (most recent last), seeding attribution for
    /// records that arrive after the scan that saw their
    /// `turn_context`. Empty for refused files and fresh baselines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<SourceTurnContext>,
}

/// One source reader's successful scan result, applied atomically by the
/// store.
#[derive(Debug, Clone)]
pub struct SourceScan {
    pub watermark_ms: i64,
    pub fingerprint: Option<SourceFingerprint>,
    pub events: Vec<UsageEvent>,
    pub rejected: usize,
    /// Rows dispositioned as usage-not-reported (see
    /// [`ScanStats::usage_not_reported`]); zero for sources without the
    /// pattern.
    pub usage_not_reported: usize,
    /// The full tracked-file state after this scan (file-based sources
    /// only; empty for the ZCode reader). Replaces the cursor's file
    /// list wholesale so deletions are reflected.
    pub files: Vec<SourceFileCursor>,
    /// The committed batch's composite end position (row-count-bounded
    /// sources). `None` leaves the cursor's position untouched (empty
    /// window, or a file-based source).
    pub batch_end: Option<SourceBatchEnd>,
}

// ---------- persisted envelope ----------

/// Shape of the persisted file:
/// `{ "version": 1, "enabled": false, "collectionStartedAt": null,
///    "events": [...], "cursors": [...] }`.
#[derive(Serialize, Deserialize)]
struct UsageIntelligenceEnvelope {
    version: u32,
    #[serde(default)]
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    collection_started_at: Option<i64>,
    #[serde(default)]
    events: Vec<UsageEvent>,
    #[serde(default)]
    cursors: Vec<SourceCursor>,
}

#[derive(Default, Clone)]
struct StoreState {
    enabled: bool,
    collection_started_at: Option<i64>,
    events: Vec<UsageEvent>,
    cursors: HashMap<String, SourceCursor>,
    /// Monotonic revision, bumped on every state change so consumers
    /// re-pull only when the plane actually moved.
    revision: u64,
}

/// Resolves the ZCode database location. Injected so tests can point the
/// collector at fixtures (or prove the disabled path never resolves a
/// path at all). `None` = the source location cannot be determined on
/// this machine (treated as source-absent, never an error surface).
pub type ZcodeDbResolver = Box<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// Resolves the Codex home directory (`~/.codex` in production; the
/// rollout trees `sessions/` and `archived_sessions/` live beneath it).
/// Same injection and disabled-gate rules as [`ZcodeDbResolver`].
pub type CodexHomeResolver = Box<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// Resolves the OpenCode database location (`~/.local/share/opencode/
/// opencode.db` in production, verified against the installed OpenCode).
/// Same injection and disabled-gate rules as [`ZcodeDbResolver`].
pub type OpencodeDbResolver = Box<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// The Usage Intelligence store: bounded state behind a small mutex,
/// persisted to one JSON file, plus the opt-in flag. Locking model: mutate
/// under the lock, release, then write the file — filesystem I/O never
/// holds the lock.
pub struct UsageIntelligenceStore {
    state: Mutex<StoreState>,
    operation: Mutex<()>,
    completion: Option<Box<dyn Fn(u64) + Send + Sync>>,
    committed: Mutex<StoreState>,
    generation: std::sync::atomic::AtomicU64,
    writable: std::sync::atomic::AtomicBool,
    last_write_ok: std::sync::atomic::AtomicBool,
    failed_bounded_sources: Mutex<std::collections::HashSet<String>>,
    // A persisted successful scan cannot attest to work attempted after it.
    // Revalidate each local database source after each restart, even if
    // recording a failure was impossible because storage was unavailable.
    restart_unverified_sources: Mutex<std::collections::HashSet<String>>,
    path: PathBuf,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    zcode_db: ZcodeDbResolver,
    /// The Codex home resolver. The Codex source is disabled for
    /// collection, so nothing resolves it in production; the field and
    /// its accessors stay so the source can be reconsidered without
    /// re-deriving the wiring.
    #[allow(dead_code)]
    codex_home: CodexHomeResolver,
    opencode_db: OpencodeDbResolver,
    /// Transient in-flight latch for the "collecting" diagnostic.
    pub(crate) collecting: std::sync::atomic::AtomicBool,
    /// Latch for the detached retention pass, so a source scan that holds
    /// the collection latch across many cycles cannot spawn one retention
    /// thread per cycle.
    retention_in_flight: std::sync::atomic::AtomicBool,
    /// Test-only crash boundary: invoked at the end of a successful
    /// [`Self::persist_current`], once the durable replacement and the
    /// committed snapshot agree. A hook that panics models a process that
    /// dies between the write and the rest of the scan's bookkeeping.
    #[cfg(test)]
    persist_hook: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
}

impl UsageIntelligenceStore {
    /// Opens the store at the given app-data path with the production
    /// ZCode and Codex resolvers.
    pub fn open(path: PathBuf) -> Self {
        Self::open_with(path, Box::new(epoch_now_ms), production_zcode_db_resolver())
    }

    /// Test constructor with an injectable clock and ZCode resolver. The
    /// Codex home resolver defaults to production; tests that enable the
    /// store attach their own via [`Self::with_codex_home_resolver`] so
    /// unit tests never probe the real Codex tree.
    pub fn open_with(
        path: PathBuf,
        now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
        zcode_db: ZcodeDbResolver,
    ) -> Self {
        let store = Self {
            state: Mutex::new(StoreState::default()),
            committed: Mutex::new(StoreState::default()),
            operation: Mutex::new(()),
            completion: None,
            generation: std::sync::atomic::AtomicU64::new(0),
            writable: std::sync::atomic::AtomicBool::new(true),
            last_write_ok: std::sync::atomic::AtomicBool::new(true),
            failed_bounded_sources: Mutex::new(std::collections::HashSet::new()),
            restart_unverified_sources: Mutex::new(std::collections::HashSet::new()),
            path,
            now_ms,
            zcode_db,
            codex_home: production_codex_home_resolver(),
            opencode_db: production_opencode_db_resolver(),
            collecting: std::sync::atomic::AtomicBool::new(false),
            retention_in_flight: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            persist_hook: Mutex::new(None),
        };
        store.load_from_disk();
        let revalidate_sources = store
            .state
            .lock()
            .unwrap()
            .cursors
            .iter()
            .filter(|(source, cursor)| {
                RESTART_REVALIDATION_SOURCES.contains(&source.as_str())
                    && cursor.baselined
                    && !cursor.pending_rebaseline
                    && cursor.state == SourceState::Ok
            })
            .map(|(source, _)| source.clone())
            .collect::<std::collections::HashSet<_>>();
        *store.restart_unverified_sources.lock().unwrap() = revalidate_sources;
        *store.committed.lock().unwrap() = store.state.lock().unwrap().clone();
        // Startup retention: a plane that was closed for longer than the
        // window ages out before the first cycle runs. The committed
        // snapshot is already in place, so a failed write rolls back to
        // the state that was just loaded.
        store.prune_expired();
        store
    }

    /// Overrides the Codex home resolver (tests, fixtures). Chainable.
    #[allow(dead_code)]
    pub fn with_codex_home_resolver(mut self, resolver: CodexHomeResolver) -> Self {
        self.codex_home = resolver;
        self
    }

    /// Overrides the OpenCode database resolver (tests, fixtures).
    /// Chainable.
    #[allow(dead_code)]
    pub fn with_opencode_db_resolver(mut self, resolver: OpencodeDbResolver) -> Self {
        self.opencode_db = resolver;
        self
    }

    pub fn with_completion(mut self, completion: Box<dyn Fn(u64) + Send + Sync>) -> Self {
        self.completion = Some(completion); self
    }

    pub fn now_ms(&self) -> i64 {
        (self.now_ms)()
    }

    /// The injected ZCode database resolver (production: the ZCode home;
    /// tests: fixtures). Only the collector may call it — and only after
    /// the enabled gate has passed.
    pub(crate) fn zcode_db_resolver(&self) -> &ZcodeDbResolver {
        &self.zcode_db
    }

    /// The injected Codex home resolver (production: `~/.codex`; tests:
    /// fixtures). Only the collector may call it — and only after the
    /// enabled gate has passed.
    #[allow(dead_code)]
    pub(crate) fn codex_home_resolver(&self) -> &CodexHomeResolver {
        &self.codex_home
    }

    /// The injected OpenCode database resolver (production:
    /// `~/.local/share/opencode`; tests: fixtures). Only the collector
    /// may call it — and only after the enabled gate has passed.
    pub(crate) fn opencode_db_resolver(&self) -> &OpencodeDbResolver {
        &self.opencode_db
    }

    /// The persisted opt-in flag. While false, collection must not probe
    /// the source at all.
    pub fn enabled(&self) -> bool {
        self.state.lock().unwrap().enabled
    }

    /// Sets the opt-in flag and persists it. Enabling does not itself
    /// collect; the caller (command or cycle hook) triggers collection so
    /// the first enable baselines exactly once, deliberately.
    ///
    /// Re-enabling after a disable marks every source cursor
    /// `pending_rebaseline`: usage completed while collection was disabled
    /// must not be silently imported, so the next pass establishes a fresh
    /// baseline at each source's then-current high-water mark (the same
    /// backlog skip as a first enable) instead of incrementally importing
    /// the disabled period. Already-collected events, totals, and the
    /// disclosure anchor are untouched.
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        if !self.writable.load(std::sync::atomic::Ordering::SeqCst) {
            self.last_write_ok.store(false, std::sync::atomic::Ordering::SeqCst);
            return Err("Usage Intelligence storage schema is unsupported or unavailable".into());
        }
        let _operation = self.operation.lock().unwrap();
        self.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let changed = {
            let mut state = self.state.lock().unwrap();
            if state.enabled == enabled {
                false
            } else {
                state.enabled = enabled;
                if enabled {
                    for cursor in state.cursors.values_mut() {
                        cursor.pending_rebaseline = true;
                    }
                }
                state.revision += 1;
                true
            }
        };
        if changed || !self.last_write_succeeded() {
            if !self.persist_current() { return Err("Could not persist Usage Intelligence preference".into()); }
        }
        Ok(())
    }

    pub fn generation(&self) -> u64 { self.generation.load(std::sync::atomic::Ordering::SeqCst) }
    pub fn last_write_succeeded(&self) -> bool { self.last_write_ok.load(std::sync::atomic::Ordering::SeqCst) }
    pub fn generation_current(&self, generation: u64) -> bool { self.enabled() && self.generation() == generation }

    fn restart_revalidation_applies(source: &str) -> bool {
        RESTART_REVALIDATION_SOURCES.contains(&source)
    }

    fn restart_unverified(&self, source: &str) -> bool {
        Self::restart_revalidation_applies(source)
            && self
                .restart_unverified_sources
                .lock()
                .unwrap()
                .contains(source)
    }

    fn mark_restart_verified(&self, source: &str) {
        if Self::restart_revalidation_applies(source) {
            self.restart_unverified_sources
                .lock()
                .unwrap()
                .remove(source);
        }
    }

    /// The revision, broadcast through the runtime snapshot.
    pub fn revision(&self) -> u64 {
        self.state.lock().unwrap().revision
    }

    /// Whether a collection pass is currently in flight (transient).
    pub fn is_collecting(&self) -> bool {
        self.collecting.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The persisted cursor of one source, when a baseline exists.
    pub fn cursor(&self, source: &str) -> Option<SourceCursor> {
        let mut cursor = self.state.lock().unwrap().cursors.get(source).cloned();
        if let Some(cursor) = &mut cursor {
            let failed = self.failed_bounded_sources.lock().unwrap().contains(source);
            let restart_unverified = self.restart_unverified(source);
            if failed || (cursor.state == SourceState::Ok && restart_unverified) {
                cursor.state = SourceState::ReadFailure;
                cursor.fingerprint = None;
            }
        }
        cursor
    }

    /// Whether the next collection pass for this source must (re-)establish
    /// its backlog-skipping baseline before any incremental read: true when
    /// no cursor exists yet, when the cursor only ever carried a failure,
    /// or when a disable → re-enable transition marked the source for a
    /// fresh baseline. Collectors gate their baseline branch on this, not
    /// on `baselined` alone.
    pub fn cursor_needs_baseline(&self, source: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .cursors
            .get(source)
            .map(|cursor| !cursor.baselined || cursor.pending_rebaseline)
            .unwrap_or(true)
    }

    /// The query-visible owned events, oldest first. Codex records remain in
    /// the persisted store, but the source is disabled for collection and is
    /// excluded from every read projection.
    #[allow(dead_code)]
    pub fn events(&self) -> Vec<UsageEvent> {
        self.state.lock().unwrap().events.clone()
            .into_iter()
            .filter(|event| event.source != USAGE_SOURCE_CODEX)
            .collect()
    }

    /// Raw stored events for collector unit tests. Production readers use
    /// [`Self::events`] or [`Self::aggregate`], which exclude Codex.
    #[cfg(test)]
    pub(crate) fn stored_events(&self) -> Vec<UsageEvent> {
        self.state.lock().unwrap().events.clone()
    }

    /// The disclosure anchor: when local collection started (the first
    /// baseline), or `None` before any baseline was ever established.
    #[allow(dead_code)]
    pub fn collection_started_at(&self) -> Option<i64> {
        self.state.lock().unwrap().collection_started_at
    }

    // ---- load / self-heal ----

    /// Loads the persisted blob. A missing file is a healthy empty store;
    /// corruption or a foreign version degrades to the empty defaults —
    /// including `enabled: false`, the fail-closed direction for an opt-in
    /// flag — and the sanitized state is written back. Never crashes
    /// startup.
    fn load_from_disk(&self) {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound { self.writable.store(false, std::sync::atomic::Ordering::SeqCst); }
                return;
            }
        };
        if serde_json::from_str::<serde_json::Value>(&raw).ok()
            .is_some_and(|value| value.get("version").and_then(serde_json::Value::as_u64) != Some(BLOB_VERSION as u64)) {
            self.writable.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        let Ok(parsed) = serde_json::from_str::<UsageIntelligenceEnvelope>(&raw) else {
            let backup = self.path.with_extension(format!("recovery-{}.json", uuid::Uuid::new_v4()));
            if crate::durable_file::replace(&backup, raw.as_bytes()).is_err() {
                self.writable.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            return;
        };
        if parsed.version != BLOB_VERSION {
            self.writable.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        let now_ms = self.now_ms();
        let mut cursors = HashMap::new();
        for cursor in parsed.cursors {
            if !cursor.source.trim().is_empty() {
                cursors.insert(cursor.source.clone(), cursor);
            }
        }
        let original_event_count = parsed.events.len();
        let mut events = Vec::with_capacity(original_event_count);
        for event in parsed.events {
            if let Some(valid) = UsageEvent::validated(event, now_ms) {
                events.push(valid);
            }
        }
        let total_events = original_event_count;
        let deduped = dedup_and_sort(events);
        let degraded = deduped.len() != total_events;
        {
            let mut state = self.state.lock().unwrap();
            state.enabled = parsed.enabled;
            state.collection_started_at = parsed.collection_started_at;
            state.events = deduped;
            state.cursors = cursors;
            state.revision += 1;
        }
        if degraded {
            let backup = self.path.with_extension(format!("recovery-{}.json", uuid::Uuid::new_v4()));
            if crate::durable_file::replace(&backup, raw.as_bytes()).is_err() {
                self.writable.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    // ---- baseline (backlog skip) ----

    /// Establishes a source baseline at the given high-water mark WITHOUT
    /// importing any historical rows, and anchors the disclosure start
    /// time. Idempotent: an existing baseline is never re-anchored by a
    /// later call. A cursor record that only ever carried a failure (never
    /// baselined) is replaced by the real baseline. The fingerprint (the
    /// source files' change signature at baseline time) makes an idle
    /// source cost no snapshot copy.
    pub fn establish_baseline(
        &self,
        source: &str,
        watermark_ms: i64,
        fingerprint: Option<SourceFingerprint>,
    ) {
        self.establish_baseline_with_files(source, watermark_ms, fingerprint, Vec::new());
    }

    /// [`Self::establish_baseline`] for file-based sources: the baseline
    /// also persists the initial per-file cursors (for the Codex reader:
    /// every existing rollout tracked at its current EOF, zero events
    /// imported).
    fn establish_baseline_with_files_inner(
        &self,
        source: &str,
        watermark_ms: i64,
        fingerprint: Option<SourceFingerprint>,
        files: Vec<SourceFileCursor>,
    ) {
        let now_ms = self.now_ms();
        let changed = {
            let mut state = self.state.lock().unwrap();
            let existing = state.cursors.get(source);
            let already_baselined = existing
                .map(|cursor| cursor.baselined)
                .unwrap_or(false);
            let pending_rebaseline = existing
                .map(|cursor| cursor.pending_rebaseline)
                .unwrap_or(false);
            // An ordinary baselined cursor is never re-anchored
            // (idempotence); a pending-rebaseline cursor (marked by the
            // disable → re-enable transition) is replaced by the fresh
            // baseline at the current high-water mark, importing nothing
            // from the disabled period.
            if already_baselined && !pending_rebaseline {
                false
            } else {
                state.cursors.insert(
                    source.to_string(),
                    SourceCursor {
                        source: source.to_string(),
                        watermark_ms,
                        baseline_watermark_ms: watermark_ms,
                        baselined: true,
                        pending_rebaseline: false,
                        batch_end: None,
                        baselined_at: now_ms,
                        last_observed_at: now_ms,
                        state: SourceState::Ok,
                        detail: None,
                        fingerprint,
                        last_scan: Some(ScanStats {
                            accepted: 0,
                            duplicates: 0,
                            rejected: 0,
                            usage_not_reported: 0,
                        }),
                        files,
                    },
                );
                if state.collection_started_at.is_none() {
                    state.collection_started_at = Some(now_ms);
                }
                state.revision += 1;
                true
            }
        };
        if changed {
            if self.persist_current() {
                self.mark_restart_verified(source);
                self.failed_bounded_sources.lock().unwrap().remove(source);
            }
        }
    }

    /// Marks a non-ok source state (absent / schema / read failure /
    /// ceiling), preserving the watermark and baseline anchors.
    fn mark_source_state_inner(&self, source: &str, state: SourceState, detail: Option<String>) {
        let now_ms = self.now_ms();
        let changed = {
            let mut locked = self.state.lock().unwrap();
            let entry = locked.cursors.entry(source.to_string()).or_insert_with(|| {
                // A failure observed before any baseline still needs a
                // cursor record to be visible in diagnostics; it is not a
                // baseline (the next successful observation establishes
                // the real one via establish_baseline).
                SourceCursor {
                    source: source.to_string(),
                    watermark_ms: 0,
                    baseline_watermark_ms: 0,
                    baselined: false,
                    pending_rebaseline: false,
                    batch_end: None,
                    baselined_at: now_ms,
                    last_observed_at: 0,
                    state: SourceState::Ok,
                    detail: None,
                    fingerprint: None,
                    last_scan: None,
                    files: Vec::new(),
                }
            });
            if entry.state == state && entry.detail == detail {
                false
            } else {
                entry.state = state;
                entry.detail = detail;
                entry.last_scan = None;
                locked.revision += 1;
                true
            }
        };
        if changed {
            self.persist_current();
        }
    }

    pub fn establish_baseline_with_files(&self, source: &str, watermark: i64, fingerprint: Option<SourceFingerprint>, files: Vec<SourceFileCursor>) {
        let _operation = self.operation.lock().unwrap();
        self.establish_baseline_with_files_inner(source, watermark, fingerprint, files);
    }
    pub fn establish_baseline_at(&self, generation: u64, source: &str, watermark: i64, fingerprint: Option<SourceFingerprint>) {
        let _operation = self.operation.lock().unwrap();
        if self.generation_current(generation) { self.establish_baseline_with_files_inner(source, watermark, fingerprint, Vec::new()); }
    }
    pub fn mark_source_state(&self, source: &str, state: SourceState, detail: Option<String>) {
        let _operation = self.operation.lock().unwrap();
        self.mark_source_state_inner(source, state, detail);
    }
    pub fn mark_source_state_at(&self, generation: u64, source: &str, state: SourceState, detail: Option<String>) {
        let _operation = self.operation.lock().unwrap();
        if self.generation_current(generation) { self.mark_source_state_inner(source, state, detail); }
    }
    pub fn apply_scan(&self, source: &str, scan: SourceScan) -> ScanStats {
        let _operation = self.operation.lock().unwrap();
        self.apply_scan_inner(source, scan, None)
    }
    pub fn apply_scan_at(&self, generation: u64, source: &str, scan: SourceScan) -> Result<ScanStats, String> {
        let _operation = self.operation.lock().unwrap();
        if !self.generation_current(generation) { return Err("Collection invalidated".into()); }
        let stats = self.apply_scan_inner(source, scan, None);
        if !self.last_write_succeeded() {
            if Self::restart_revalidation_applies(source) {
                self.failed_bounded_sources.lock().unwrap().insert(source.to_string());
            }
            return Err("Collection could not be persisted; cursor retained".into());
        }
        self.failed_bounded_sources.lock().unwrap().remove(source);
        self.mark_restart_verified(source);
        Ok(stats)
    }

    pub fn apply_bounded_scan_at(
        &self,
        generation: u64,
        source: &str,
        scan: SourceScan,
        detail: String,
    ) -> Result<ScanStats, String> {
        let _operation = self.operation.lock().unwrap();
        if !self.generation_current(generation) {
            return Err("Collection invalidated".into());
        }
        let stats = self.apply_scan_inner(source, scan, Some(detail));
        if !self.last_write_succeeded() {
            self.failed_bounded_sources
                .lock()
                .unwrap()
                .insert(source.to_string());
            // The rollback keeps the old cursor, but its previous `ok`
            // verdict cannot describe the newly discovered backlog.
            if let Some(cursor) = self.state.lock().unwrap().cursors.get_mut(source) {
                cursor.state = SourceState::ReadFailure;
                cursor.detail = Some("Collection could not be persisted; cursor retained".into());
                cursor.fingerprint = None;
            }
            return Err("Collection could not be persisted; cursor retained".into());
        }
        self.failed_bounded_sources.lock().unwrap().remove(source);
        self.mark_restart_verified(source);
        Ok(stats)
    }

    // ---- scan application ----

    /// Applies one successful scan: validates and dedups the events,
    /// applies retention, advances the cursor watermark and diagnostics,
    /// and persists. Returns the applied stats (accepted counts only
    /// newly stored events; duplicates were already present — in the store
    /// or earlier in the same batch).
    fn apply_scan_inner(
        &self,
        source: &str,
        mut scan: SourceScan,
        ceiling_detail: Option<String>,
    ) -> ScanStats {
        let now_ms = self.now_ms();
        let cutoff = retention_cutoff(now_ms);
        let mut batch: Vec<UsageEvent> = Vec::new();
        let mut batch_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut rejected = scan.rejected;
        for event in scan.events {
            match UsageEvent::validated(event, now_ms) {
                Some(valid_event) => {
                    // Dedup inside the batch too, so `accepted` counts
                    // real insertions even when a scan overlaps itself.
                    if batch_ids.insert(valid_event.id.clone()) {
                        batch.push(valid_event);
                    }
                }
                None => rejected += 1,
            }
        }

        let stats = {
            let mut state = self.state.lock().unwrap();
            retain_within_retention(&mut state.events, now_ms);
            let existing: std::collections::HashSet<&str> =
                state.events.iter().map(|e| e.id.as_str()).collect();
            let mut accepted = 0;
            let mut duplicates = 0;
            let mut merged = state.events.clone();
            let mut capacity_limited = false;
            for event in batch {
                if existing.contains(event.id.as_str()) {
                    duplicates += 1;
                    continue;
                }
                if merged.len() >= USAGE_EVENT_MAX {
                    capacity_limited = true;
                    break;
                }
                merged.push(event);
                accepted += 1;
            }
            let retained: Vec<UsageEvent> = dedup_and_sort(merged)
                .into_iter()
                .filter(|event| event.event_at >= cutoff)
                .collect();
            let stats = ScanStats {
                accepted,
                duplicates,
                rejected,
                usage_not_reported: scan.usage_not_reported,
            };
            state.events = retained;
            if let Some(cursor) = state.cursors.get_mut(source) {
                if cursor.baselined && !capacity_limited {
                    cursor.watermark_ms = cursor.watermark_ms.max(scan.watermark_ms);
                    // The committed batch's composite end position — the
                    // resume point for a bounded (deferred) backlog.
                    if let Some(end) = scan.batch_end.take() {
                        cursor.batch_end = Some(end);
                    }
                    // File-based sources report their full tracked-file
                    // state; replacing wholesale also reflects deletions.
                    cursor.files = std::mem::take(&mut scan.files);
                }
                cursor.last_observed_at = now_ms;
                cursor.state = if capacity_limited || ceiling_detail.is_some() {
                    SourceState::ScanCeiling
                } else {
                    SourceState::Ok
                };
                cursor.detail = if capacity_limited {
                    Some(
                        "Event capacity reached; uncommitted backlog retained at source"
                            .to_string(),
                    )
                } else {
                    ceiling_detail
                };
                cursor.fingerprint = if cursor.state == SourceState::ScanCeiling {
                    None
                } else {
                    scan.fingerprint
                };
                cursor.last_scan = Some(stats);
            }
            state.revision += 1;
            stats
        };
        self.persist_current();
        stats
    }

    // ---- retention ----

    /// Applies the owned 35-day retention window without running a scan.
    ///
    /// Retention is a store property rather than a scan side effect: it has
    /// to hold after a restart, while a source is idle (unchanged
    /// fingerprint, nothing to read), while collection is disabled (no
    /// source is probed at all), and after a scan that failed, paused, or
    /// panicked before it could commit. The scan path keeps pruning inline
    /// for the same reason it always did; this pass applies the identical
    /// rule from startup and from every cycle.
    ///
    /// Safeguards preserved: the pass takes the same `operation`
    /// serialization as every other mutation, writes through
    /// [`Self::persist_current`] (temp file + rename), and leaves a
    /// write-blocked store alone — an unknown or unreadable schema is
    /// never rewritten. When the write fails, `persist_current` restores
    /// the last committed snapshot, so no event leaves memory without a
    /// durable file that matches.
    ///
    /// Returns `true` only when events were actually removed and the
    /// pruned state was durably written.
    pub fn prune_expired(&self) -> bool {
        if !self.writable.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let _operation = self.operation.lock().unwrap();
        let removed = {
            let mut state = self.state.lock().unwrap();
            let removed = retain_within_retention(&mut state.events, self.now_ms());
            if removed > 0 {
                state.revision += 1;
            }
            removed
        };
        if removed == 0 {
            return false;
        }
        self.persist_current()
    }

    /// Whether the owned window is due: the oldest stored event is past
    /// the retention boundary. The events are kept in the canonical
    /// `(event_at, id)` order, so this reads the oldest event only.
    pub fn retention_due(&self) -> bool {
        let cutoff = retention_cutoff(self.now_ms());
        let state = self.state.lock().unwrap();
        state
            .events
            .first()
            .is_some_and(|event| event.event_at < cutoff)
    }

    // ---- owned clear ----

    /// Clears everything this plane owns: events, cursors (and their
    /// watermarks), and the disclosure anchor. The opt-in flag is
    /// deliberately preserved — clearing data is not disabling the
    /// feature; while still enabled, the next collection re-baselines at
    /// the source's current high-water mark and does not backfill.
    pub fn clear_owned(&self) -> Result<bool, String> {
        let _operation = self.operation.lock().unwrap();
        self.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (changed, had_data) = {
            let mut state = self.state.lock().unwrap();
            let had_data = !state.events.is_empty()
                || !state.cursors.is_empty()
                || state.collection_started_at.is_some();
            state.events.clear();
            state.cursors.clear();
            state.collection_started_at = None;
            state.revision += 1;
            (true, had_data)
        };
        if changed {
            self.persist_current();
        }
        if !self.last_write_succeeded() { return Err("Could not durably clear Usage Intelligence".into()); }
        self.failed_bounded_sources.lock().unwrap().clear();
        self.restart_unverified_sources.lock().unwrap().clear();
        Ok(had_data)
    }

    // ---- aggregation ----

    /// Aggregates the owned events for one range. `today_start_ms` is the
    /// caller's local midnight for the Today range (validated for
    /// sanity); the rolling ranges are timezone-free.
    pub fn aggregate(
        &self,
        range: UsageRange,
        today_start_ms: Option<i64>,
    ) -> UsageIntelligenceDto {
        let now_ms = self.now_ms();
        let (range_start, range_label) = match range {
            UsageRange::Today => {
                let start = today_start_ms.unwrap_or(now_ms);
                // Sanity: a local midnight supplied by the webview must be
                // within the last 48 hours to be trusted; otherwise the
                // range degrades to the last 24 hours rather than
                // silently aggregating a wrong window.
                let clamped = if now_ms - start > 48 * 60 * 60_000 || start > now_ms {
                    now_ms - 24 * 60 * 60_000
                } else {
                    start
                };
                (clamped, "today")
            }
            UsageRange::SevenDays => (now_ms - 7 * 24 * 60 * 60_000, "7d"),
            UsageRange::ThirtyDays => (now_ms - 30 * 24 * 60 * 60_000, "30d"),
        };

        let (events, enabled, collection_started_at, mut cursors, collecting) = {
            let state = self.state.lock().unwrap();
            (
                state.events.clone(),
                state.enabled,
                state.collection_started_at,
                state
                    .cursors
                    .values()
                    .filter(|cursor| cursor.source != USAGE_SOURCE_CODEX)
                    .cloned()
                    .collect::<Vec<_>>(),
                self.is_collecting(),
            )
        };

        let failed = self.failed_bounded_sources.lock().unwrap();
        let restart_unverified = self.restart_unverified_sources.lock().unwrap();
        for cursor in &mut cursors {
            if failed.contains(&cursor.source) {
                cursor.state = SourceState::ReadFailure;
                cursor.detail = Some("Collection could not be persisted; cursor retained".into());
            } else if cursor.state == SourceState::Ok
                && RESTART_REVALIDATION_SOURCES.contains(&cursor.source.as_str())
                && restart_unverified.contains(&cursor.source)
            {
                cursor.state = SourceState::ReadFailure;
                let label = if cursor.source == USAGE_SOURCE_ZCODE {
                    "ZCode"
                } else {
                    "OpenCode"
                };
                cursor.detail = Some(format!(
                    "{label} coverage requires a successful scan after restart"
                ));
            }
        }
        drop((failed, restart_unverified));

        // Codex rollout records predate a trustworthy owner binding. Keep
        // them in the store so existing data is not destroyed, but exclude
        // the entire source from every current read projection.
        let in_range: Vec<&UsageEvent> = events
            .iter()
            .filter(|event| event.source != USAGE_SOURCE_CODEX)
            .filter(|event| event.event_at >= range_start && event.event_at <= now_ms)
            .collect();

        // provider -> model -> totals
        let mut providers: HashMap<String, HashMap<String, TokenSums>> = HashMap::new();
        let mut totals = TokenSums::default();
        let mut total_events = 0usize;
        for event in &in_range {
            let models = providers.entry(event.provider.clone()).or_default();
            let sums = models.entry(event.model.clone()).or_default();
            sums.add(event);
            totals.add(event);
            total_events += 1;
        }

        let mut groups: Vec<ProviderBreakdownDto> = providers
            .into_iter()
            .map(|(provider, models)| {
                let mut models: Vec<ModelBreakdownDto> = models
                    .into_iter()
                    .map(|(model, sums)| ModelBreakdownDto {
                        model,
                        events: sums.events,
                        input_tokens: sums.input,
                        cache_read_tokens: sums.cache_read,
                        cache_write_tokens: sums.cache_write,
                        output_tokens: sums.output,
                        reasoning_tokens: sums.reasoning,
                        total_tokens: sums.total,
                    })
                    .collect();
                models.sort_by(|a, b| {
                    b.total_tokens
                        .cmp(&a.total_tokens)
                        .then_with(|| a.model.cmp(&b.model))
                });
                let provider_totals = models.iter().fold(TokenSums::default(), |mut acc, m| {
                    acc.input += m.input_tokens;
                    acc.cache_read += m.cache_read_tokens;
                    acc.cache_write += m.cache_write_tokens;
                    acc.output += m.output_tokens;
                    acc.reasoning += m.reasoning_tokens;
                    acc.total += m.total_tokens;
                    acc.events += m.events;
                    acc
                });
                ProviderBreakdownDto {
                    provider,
                    input_tokens: provider_totals.input,
                    cache_read_tokens: provider_totals.cache_read,
                    cache_write_tokens: provider_totals.cache_write,
                    output_tokens: provider_totals.output,
                    reasoning_tokens: provider_totals.reasoning,
                    total_tokens: provider_totals.total,
                    events: provider_totals.events,
                    models,
                }
            })
            .collect();
        groups.sort_by(|a, b| {
            b.total_tokens
                .cmp(&a.total_tokens)
                .then_with(|| a.provider.cmp(&b.provider))
        });

        // Per-source coverage uses the exact cursor universe shown in
        // diagnostics. A source with no cursor yet stays invisible: nothing
        // has been collected from it, so there is no persisted fact to
        // qualify (the first collection pass creates its cursor).
        let source_coverage: Vec<SourceCoverageDto> = cursors
            .iter()
            .map(|cursor| source_coverage_for(cursor, enabled, range_start))
            .collect();
        let sources = cursors
            .into_iter()
            .map(|cursor| SourceDiagnosticsDto {
                source: cursor.source,
                state: rendered_source_state(cursor.state, enabled, collecting),
                watermark_at: canonical_timestamp(cursor.watermark_ms),
                baselined_at: canonical_timestamp(cursor.baselined_at),
                last_observed_at: if cursor.last_observed_at > 0 {
                    Some(canonical_timestamp(cursor.last_observed_at))
                } else {
                    None
                },
                detail: cursor.detail,
                last_scan: cursor.last_scan,
            })
            .collect();

        let daily = build_daily_days(&in_range, range_start, now_ms, collection_started_at);

        // The range is complete only when the original collection anchor
        // covers it AND every visible source reports complete coverage.
        // Before any baseline, an enabled plane is conservatively
        // incomplete; a disabled empty plane keeps the historical false
        // empty-state verdict (nothing was ever claimed to be collected).
        let incomplete_history = collection_started_at
            .map(|started| started > range_start)
            .unwrap_or(enabled)
            || source_coverage
                .iter()
                .any(|coverage| coverage.coverage != "complete");

        UsageIntelligenceDto {
            schema_version: USAGE_INTELLIGENCE_SCHEMA_VERSION,
            persistence_available: self.writable.load(std::sync::atomic::Ordering::SeqCst),
            last_write_succeeded: self.last_write_succeeded(),
            range: range_label.to_string(),
            range_start: canonical_timestamp(range_start),
            range_end: canonical_timestamp(now_ms),
            generated_at: canonical_timestamp(now_ms),
            enabled,
            events_in_range: total_events,
            groups,
            totals: TotalsDto {
                events: total_events,
                input_tokens: totals.input,
                cache_read_tokens: totals.cache_read,
                cache_write_tokens: totals.cache_write,
                output_tokens: totals.output,
                reasoning_tokens: totals.reasoning,
                total_tokens: totals.total,
            },
            collection_started_at: collection_started_at.map(canonical_timestamp),
            incomplete_history,
            sources,
            source_coverage,
            daily,
            semantics: "reportedTokenUsageCollectedLocally",
        }
    }

    // ---- persistence ----

    /// Serializes the state and replaces the file atomically-ish: write to
    /// a temp file in the same directory, then rename over the target.
    fn persist_current(&self) -> bool {
        let (enabled, collection_started_at, events, cursors) = {
            let state = self.state.lock().unwrap();
            (
                state.enabled,
                state.collection_started_at,
                state.events.clone(),
                state.cursors.values().cloned().collect::<Vec<_>>(),
            )
        };
        let mut sorted_cursors = cursors;
        sorted_cursors.sort_by(|a, b| a.source.cmp(&b.source));
        let envelope = UsageIntelligenceEnvelope {
            version: BLOB_VERSION,
            enabled,
            collection_started_at,
            events,
            cursors: sorted_cursors,
        };
        let Ok(json) = serde_json::to_string(&envelope) else {
            return false;
        };
        let persisted = self.writable.load(std::sync::atomic::Ordering::SeqCst)
            && crate::durable_file::replace(&self.path, json.as_bytes()).is_ok();
        self.last_write_ok.store(persisted, std::sync::atomic::Ordering::SeqCst);
        if persisted {
            *self.committed.lock().unwrap() = self.state.lock().unwrap().clone();
        } else {
            let mut state = self.state.lock().unwrap();
            let revision = state.revision;
            *state = self.committed.lock().unwrap().clone();
            state.revision = revision;
        }
        // The durable file and the in-memory state agree from here on; a
        // test hook that panics now models a process dying before the
        // scan's remaining bookkeeping (return value, source state marks)
        // could run.
        #[cfg(test)]
        self.run_persist_hook();
        persisted
    }

    /// Installs the test-only crash boundary fired after a successful
    /// durable replacement. Passing `None` disarms it.
    #[cfg(test)]
    pub(crate) fn set_persist_hook(
        &self,
        hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    ) {
        *self.persist_hook.lock().unwrap() = hook;
    }

    /// Runs the test-only crash boundary. The closure is cloned out from
    /// under the lock first so a panicking hook cannot poison the slot and
    /// a later pass can still observe it.
    #[cfg(test)]
    fn run_persist_hook(&self) {
        let hook = self.persist_hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Test-only recovery after a simulated interruption: the aborted
    /// scan unwound while holding the operation lock, so its mutexes are
    /// poisoned. A real crash discards the process; a test that keeps it
    /// alive must clear the poison before driving the next scan.
    #[cfg(test)]
    pub(crate) fn recover_after_interrupted_persist(&self) {
        self.operation.clear_poison();
        self.state.clear_poison();
        self.committed.clear_poison();
        self.failed_bounded_sources.clear_poison();
        self.restart_unverified_sources.clear_poison();
        self.persist_hook.clear_poison();
    }
}

/// The inclusive lower bound of the owned retention window: an event
/// stamped exactly at the cutoff is still inside the window.
fn retention_cutoff(now_ms: i64) -> i64 {
    now_ms - USAGE_EVENT_RETENTION_MS
}

/// Drops owned events older than the retention boundary, in place.
/// Returns how many were removed. The boundary is inclusive: an event
/// stamped exactly `USAGE_EVENT_RETENTION_MS` before `now_ms` stays.
/// This is the one retention rule, shared by the scan path and the
/// independent startup / cycle passes so they can never disagree.
fn retain_within_retention(events: &mut Vec<UsageEvent>, now_ms: i64) -> usize {
    let cutoff = retention_cutoff(now_ms);
    let before = events.len();
    events.retain(|event| event.event_at >= cutoff);
    before - events.len()
}

/// Dedups by owned id (first occurrence wins) and sorts by (event_at, id)
/// so the persisted shape is canonical.
fn dedup_and_sort(mut events: Vec<UsageEvent>) -> Vec<UsageEvent> {
    let mut seen = std::collections::HashSet::with_capacity(events.len());
    events.retain(|event| seen.insert(event.id.clone()));
    events.sort_by(|a, b| a.event_at.cmp(&b.event_at).then_with(|| a.id.cmp(&b.id)));
    events
}

/// The local calendar day of an epoch-ms instant, on the collecting
/// machine's timezone (`chrono::Local`, same convention as the quota
/// plane's heatmap in `usage_analytics.rs`). An instant-to-local
/// conversion is unambiguous by construction; the fallback keeps a
/// malformed system clock from panicking the read path.
fn local_day_key(ms: i64) -> chrono::NaiveDate {
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(|| chrono::Local.timestamp_millis_opt(0).single().unwrap())
        .date_naive()
}

/// Buckets the range's events into one cell per local calendar day touched
/// by `[range_start, now_ms]`. The input is the already-filtered `in_range`
/// set, so the daily sums reconcile exactly with the range totals; dedup
/// and retention have already been applied upstream. Days without events
/// are not observed — never serialized as zero — and days wholly before
/// the local collection start carry `before_collection`.
fn build_daily_days(
    in_range: &[&UsageEvent],
    range_start: i64,
    now_ms: i64,
    collection_started_at: Option<i64>,
) -> Vec<UsageDailyDayDto> {
    let mut buckets: HashMap<chrono::NaiveDate, (usize, u64)> = HashMap::new();
    for event in in_range {
        let entry = buckets.entry(local_day_key(event.event_at)).or_default();
        entry.0 += 1;
        entry.1 += event.total_tokens;
    }

    let collection_day = collection_started_at.map(local_day_key);
    let mut days = Vec::new();
    let mut date = local_day_key(range_start);
    let end_day = local_day_key(now_ms);
    while date <= end_day {
        let bucket = buckets.get(&date);
        days.push(UsageDailyDayDto {
            date: date.format("%Y-%m-%d").to_string(),
            observed: bucket.is_some(),
            events: bucket.map(|(events, _)| *events).unwrap_or(0),
            total_tokens: bucket.map(|(_, tokens)| *tokens),
            before_collection: collection_day.map(|started| date < started).unwrap_or(false),
        });
        date = date.succ_opt().expect("calendar date cannot overflow");
    }
    days
}

#[derive(Default, Clone, Copy)]
struct TokenSums {
    events: usize,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}

impl TokenSums {
    fn add(&mut self, event: &UsageEvent) {
        self.events += 1;
        self.input += event.input_tokens;
        self.cache_read += event.cache_read_tokens;
        self.cache_write += event.cache_write_tokens;
        self.output += event.output_tokens;
        self.reasoning += event.reasoning_tokens;
        self.total += event.total_tokens;
    }
}

fn epoch_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// RFC-3339 UTC rendering of an epoch-ms stamp (same convention as the
/// history store).
fn canonical_timestamp(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The production ZCode database location:
/// `~/.zcode/cli/db/db.sqlite` (verified against the installed ZCode
/// CLI). Resolving it is the collector's business — never called while
/// disabled.
fn production_zcode_db_resolver() -> ZcodeDbResolver {
    Box::new(|| {
        let home = std::env::home_dir()?;
        let path = home.join(".zcode").join("cli").join("db").join("db.sqlite");
        Some(path)
    })
}

/// The production Codex home: `~/.codex` (the rollout trees beneath it
/// are the collector's business). Resolving it is the collector's
/// business — never called while disabled.
fn production_codex_home_resolver() -> CodexHomeResolver {
    Box::new(|| {
        let home = std::env::home_dir()?;
        Some(home.join(".codex"))
    })
}

/// The production OpenCode database:
/// `~/.local/share/opencode/opencode.db` (verified against the installed
/// OpenCode CLI). Resolving it is the collector's business — never called
/// while disabled.
fn production_opencode_db_resolver() -> OpencodeDbResolver {
    Box::new(|| {
        let home = std::env::home_dir()?;
        Some(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
        )
    })
}

/// The derived, wire-facing state of a source: `disabled` and
/// `collecting` are runtime verdicts; the rest persist.
fn rendered_source_state(persisted: SourceState, enabled: bool, collecting: bool) -> String {
    if !enabled {
        "disabled".to_string()
    } else if collecting {
        "collecting".to_string()
    } else {
        match persisted {
            SourceState::SourceAbsent => "sourceAbsent".to_string(),
            SourceState::SchemaUnsupported => "schemaUnsupported".to_string(),
            SourceState::ReadFailure => "readFailure".to_string(),
            SourceState::ScanCeiling => "scanCeiling".to_string(),
            SourceState::CollectionPaused => "collectionPaused".to_string(),
            SourceState::Ok => "ok".to_string(),
        }
    }
}

/// The honest coverage verdict for one source over `[range_start, now]`.
///
/// A disabled plane cannot collect, a source without a baseline has no
/// observation at all, and a pending re-baseline spans an unknowable gap
/// while collection was off. Only a baseline at or before the range start
/// can support a complete verdict; a later baseline is continuous from its
/// own instant, while a paused or failed last scan may have missed later
/// events.
///
/// The coverage boundary is the later of the store-clock baseline instant
/// and the source-time baseline watermark. Taking the maximum is
/// conservative under clock skew in either direction: a source watermark
/// ahead of the store clock means the source itself had not reached the
/// range start at baseline time, even though `baselined_at` (the store
/// clock) says it had.
fn source_coverage_for(
    cursor: &SourceCursor,
    enabled: bool,
    range_start: i64,
) -> SourceCoverageDto {
    if !enabled {
        return SourceCoverageDto {
            source: cursor.source.clone(),
            coverage: "uncertain".to_string(),
            reason: Some("monitoringDisabled".to_string()),
            covered_from: None,
            last_observed_at: None,
        };
    }
    if !cursor.baselined {
        return SourceCoverageDto {
            source: cursor.source.clone(),
            coverage: "uncertain".to_string(),
            reason: Some("noBaseline".to_string()),
            covered_from: None,
            last_observed_at: None,
        };
    }
    if cursor.pending_rebaseline {
        return SourceCoverageDto {
            source: cursor.source.clone(),
            coverage: "incomplete".to_string(),
            reason: Some("pendingRebaseline".to_string()),
            covered_from: None,
            last_observed_at: None,
        };
    }
    let coverage_start = cursor.baselined_at.max(cursor.baseline_watermark_ms);
    if coverage_start > range_start {
        return SourceCoverageDto {
            source: cursor.source.clone(),
            coverage: "incomplete".to_string(),
            reason: Some("lateBaseline".to_string()),
            covered_from: Some(canonical_timestamp(coverage_start)),
            last_observed_at: None,
        };
    }

    let reason = match cursor.state {
        SourceState::Ok => {
            return SourceCoverageDto {
                source: cursor.source.clone(),
                coverage: "complete".to_string(),
                reason: None,
                covered_from: None,
                last_observed_at: None,
            };
        }
        SourceState::ScanCeiling => "scanCeiling",
        SourceState::SourceAbsent => "sourceAbsent",
        SourceState::SchemaUnsupported => "schemaUnsupported",
        SourceState::ReadFailure => "readFailure",
        SourceState::CollectionPaused => "collectionPaused",
    };
    SourceCoverageDto {
        source: cursor.source.clone(),
        coverage: "uncertain".to_string(),
        reason: Some(reason.to_string()),
        covered_from: None,
        last_observed_at: (cursor.last_observed_at > 0)
            .then(|| canonical_timestamp(cursor.last_observed_at)),
    }
}

// ---------- aggregation DTOs (camelCase on the wire) ----------

/// Supported aggregation ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageRange {
    #[serde(rename = "today")]
    Today,
    #[serde(rename = "7d")]
    SevenDays,
    #[serde(rename = "30d")]
    ThirtyDays,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdownDto {
    pub model: String,
    pub events: usize,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBreakdownDto {
    pub provider: String,
    pub events: usize,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
    pub models: Vec<ModelBreakdownDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalsDto {
    pub events: usize,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDiagnosticsDto {
    pub source: String,
    /// Derived wire state: disabled | collecting | sourceAbsent |
    /// schemaUnsupported | readFailure | scanCeiling | collectionPaused |
    /// ok.
    pub state: String,
    pub watermark_at: String,
    pub baselined_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_scan: Option<ScanStats>,
}

/// Per-source range coverage. Additive to the original single-anchor
/// `incomplete_history`; a missing baseline, a disabled collection period,
/// and a failed last scan are all distinct from measured zero usage.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCoverageDto {
    pub source: String,
    /// `complete` | `incomplete` | `uncertain`.
    pub coverage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageIntelligenceDto {
    pub schema_version: u32,
    pub persistence_available: bool,
    pub last_write_succeeded: bool,
    pub range: String,
    pub range_start: String,
    pub range_end: String,
    pub generated_at: String,
    pub enabled: bool,
    pub events_in_range: usize,
    pub groups: Vec<ProviderBreakdownDto>,
    pub totals: TotalsDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_started_at: Option<String>,
    pub incomplete_history: bool,
    pub sources: Vec<SourceDiagnosticsDto>,
    /// One verdict per existing source cursor, using the same source
    /// universe and order as `sources`. Always present; possibly empty.
    pub source_coverage: Vec<SourceCoverageDto>,
    /// One cell per local calendar day touched by the selected range,
    /// bucketing the same events the totals aggregate — the daily sums
    /// always reconcile with `totals` and `events_in_range`.
    pub daily: Vec<UsageDailyDayDto>,
    /// Honesty pin: these are reported tokens collected locally, never
    /// account quota, billing usage, or exact spend.
    pub semantics: &'static str,
}

/// One local calendar day of the Daily activity projection. A day without
/// recorded events is `observed: false` with `total_tokens` absent on the
/// wire — missing observations are never serialized as a confirmed zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDailyDayDto {
    /// Local calendar date of the collecting machine, `YYYY-MM-DD`.
    pub date: String,
    /// The day holds at least one recorded event within the range.
    pub observed: bool,
    /// Recorded events bucketed to this day (0 when not observed).
    pub events: usize,
    /// Reported token total for the day. `None` when the day has no
    /// recorded events; `Some(0)` is a measured, reported zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// The whole day precedes the local collection start: no observation
    /// can exist (no historical backfill), distinct in wording from a day
    /// inside the collection window that simply recorded nothing.
    pub before_collection: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageIntelligenceQuery {
    pub range: UsageRange,
    /// Local midnight (epoch ms) from the webview for the Today range;
    /// ignored (and optional) for the rolling ranges.
    #[serde(default)]
    pub today_start_ms: Option<i64>,
}

// ---------- collection entry point ----------

/// Runs one ZCode collection pass against this store, honoring the opt-in
/// flag (a disabled store returns before resolving any path). Blocking
/// filesystem work — call from `spawn_blocking` off the async runtime.
pub fn collect_zcode(
    store: &UsageIntelligenceStore,
) -> crate::usage_source_zcode::ZcodeScanOutcome {
    crate::usage_source_zcode::collect(store)
}

/// Runs one collection pass over the sources with a trustworthy ownership
/// boundary: ZCode and OpenCode. Codex rollout ingestion is deliberately
/// disabled because those records cannot be bound to the active session;
/// its existing cursor and events remain untouched for inspection and are
/// excluded by [`UsageIntelligenceStore::aggregate`]. The enabled sources
/// are isolated — one failing never blocks the other. Blocking filesystem
/// work — call from `spawn_blocking` off the async runtime.
pub fn run_collection_pass(store: &UsageIntelligenceStore) {
    if store.collecting.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_err() { return; }
    let _reset = CollectionReset(store);
    // Before the scans, so a source that is absent, idle, paused, failed,
    // or disabled still ages the owned history out on this cycle.
    store.prune_expired();
    collect_generation(store, store.generation());
}

struct CollectionReset<'a>(&'a UsageIntelligenceStore);
impl Drop for CollectionReset<'_> {
    fn drop(&mut self) {
        self.0.collecting.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(completion) = &self.0.completion { completion(self.0.revision()); }
    }
}
fn collect_generation(store: &UsageIntelligenceStore, generation: u64) {
    let _ = crate::usage_source_zcode::collect_at(store, generation);
    if store.generation_current(generation) {
        let _ = crate::usage_source_opencode::collect_at(store, generation);
    }
}

/// Spawns one detached collection pass (used by the runtime cycle and the
/// enable command). Failures update the store's diagnostics; nothing is
/// propagated to quota-provider health.
///
/// Retention is applied on two paths, both independent of a scan:
/// a disabled plane spawns the owned-retention pass only, so the store
/// still ages out while collection is off and no source is probed; an
/// enabled plane prunes at the start of the collection task, so a scan
/// that fails or panics cannot defer the window either.
///
/// The spawn must go through `tauri::async_runtime`, not `tokio::task`:
/// the enable command is synchronous and runs on the WebView2 IPC callback
/// thread with no entered Tokio runtime, where `tokio::task::spawn_blocking`
/// panics ("there is no reactor running") and the unwind aborts the whole
/// process across the FFI boundary.
pub fn spawn_collection(store: std::sync::Arc<UsageIntelligenceStore>) {
    if !store.enabled() {
        spawn_retention(store);
        return;
    }
    if store
        .collecting
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        // A pass is already in flight. It applied retention when it
        // started, but a slow source can hold it across many cycles, so
        // the window still gets a fresh pass here.
        spawn_retention(store);
        return;
    }
    let generation = store.generation();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let _reset = CollectionReset(&store);
        store.prune_expired();
        if store.generation_current(generation) { collect_generation(&store, generation); }
    });
}

/// Spawns one detached owned-retention pass. Used while collection is
/// disabled: it touches only this store's own file and never resolves,
/// stats, or opens a source.
///
/// The pass is skipped when the window is not due and coalesced with an
/// already running one, so a cycle that finds the collection latch busy
/// never leaves a thread behind per cycle. It runs under the store's own
/// serialization point, so it cannot interleave with a scan's write.
pub fn spawn_retention(store: std::sync::Arc<UsageIntelligenceStore>) {
    if !store.retention_due() {
        return;
    }
    if store
        .retention_in_flight
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return;
    }
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let _reset = RetentionReset(&store);
        if store.prune_expired() {
            if let Some(completion) = &store.completion {
                completion(store.revision());
            }
        }
    });
}

/// Releases the retention latch, also when the pass unwinds.
struct RetentionReset<'a>(&'a UsageIntelligenceStore);

impl Drop for RetentionReset<'_> {
    fn drop(&mut self) {
        self.0
            .retention_in_flight
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

// ---------- tauri commands ----------

/// The minimal aggregation API: Today / 7d / 30d, grouped by provider and
/// model, with the collection-start disclosure and per-source
/// diagnostics. Read-only over the owned store; never probes a source.
#[tauri::command]
pub fn get_usage_intelligence(
    handle: tauri::State<RuntimeHandle>,
    query: UsageIntelligenceQuery,
) -> UsageIntelligenceDto {
    match handle.usage_intelligence_store() {
        Some(store) => store.aggregate(query.range, query.today_start_ms),
        None => UsageIntelligenceDto {
            schema_version: USAGE_INTELLIGENCE_SCHEMA_VERSION,
            persistence_available: false,
            last_write_succeeded: false,
            range: match query.range {
                UsageRange::Today => "today",
                UsageRange::SevenDays => "7d",
                UsageRange::ThirtyDays => "30d",
            }
            .to_string(),
            range_start: canonical_timestamp(0),
            range_end: canonical_timestamp(0),
            generated_at: canonical_timestamp(epoch_now_ms()),
            enabled: false,
            events_in_range: 0,
            groups: Vec::new(),
            totals: TotalsDto {
                events: 0,
                input_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 0,
                reasoning_tokens: 0,
                total_tokens: 0,
            },
            collection_started_at: None,
            incomplete_history: false,
            sources: Vec::new(),
            source_coverage: Vec::new(),
            daily: Vec::new(),
            semantics: "reportedTokenUsageCollectedLocally",
        },
    }
}

/// The Usage Intelligence opt-in toggle (settings drawer). Persisted in
/// the owned store so the startup cycle respects it before any webview
/// attaches. Enabling triggers exactly one immediate collection pass so
/// the first baseline is established without waiting for a cycle;
/// disabling stops all probing.
#[tauri::command]
pub fn set_usage_intelligence_enabled(handle: tauri::State<RuntimeHandle>, enabled: bool) -> Result<(), String> {
    handle.set_usage_intelligence_enabled(enabled)
}

/// Clears everything Usage Intelligence owns (events, cursors, disclosure
/// anchor) and leaves the source untouched. While the source remains
/// enabled, the next collection re-baselines at the current high-water
/// mark — old usage does not reappear.
#[tauri::command]
pub fn clear_usage_intelligence(handle: tauri::State<RuntimeHandle>) -> Result<LocalDataClearOutcome, String> {
    let store = handle.usage_intelligence_store().ok_or("Usage Intelligence persistence unavailable")?;
    let removed = store.clear_owned()?;
    Ok(LocalDataClearOutcome { category: USAGE_INTELLIGENCE_CATEGORY, removed })
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW_MS: i64 = 1_791_288_000_000; // 2026-10-06T12:00:00.000Z
    const DAY_MS: i64 = 24 * 60 * 60_000;

    fn fixed_clock() -> Box<dyn Fn() -> i64 + Send + Sync> {
        Box::new(|| NOW_MS)
    }

    /// A clock a test can advance, so "collection started N days ago" is
    /// exercised the way production experiences it (the disclosure anchor
    /// is the LimitScope clock at baseline time, not the source watermark).
    fn manual_clock(
        start: i64,
    ) -> (
        std::sync::Arc<std::sync::Mutex<i64>>,
        Box<dyn Fn() -> i64 + Send + Sync>,
    ) {
        let now = std::sync::Arc::new(std::sync::Mutex::new(start));
        let clock = {
            let now = now.clone();
            Box::new(move || *now.lock().unwrap()) as Box<dyn Fn() -> i64 + Send + Sync>
        };
        (now, clock)
    }

    fn temp_store_with_clock(
        tag: &str,
        clock: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> (UsageIntelligenceStore, tempdir::TempDir) {
        let dir = tempdir::temp_dir(tag);
        let store = UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
            clock,
            Box::new(|| None),
        );
        (store, dir)
    }

    /// A second clock handle over the same instant, so a test can reopen a
    /// store after advancing time.
    fn clock_from(
        now: std::sync::Arc<std::sync::Mutex<i64>>,
    ) -> Box<dyn Fn() -> i64 + Send + Sync> {
        Box::new(move || *now.lock().unwrap())
    }

    /// Stored event ids in the store's canonical (event_at, id) order.
    fn retained_ids(store: &UsageIntelligenceStore) -> Vec<String> {
        store
            .stored_events()
            .into_iter()
            .map(|event| event.id)
            .collect()
    }

    /// Event ids actually present in the durable file, sorted.
    fn persisted_ids(path: &std::path::Path) -> Vec<String> {
        let raw = fs::read_to_string(path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut ids: Vec<String> = value["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    }

    /// Source ids present in the durable file's cursor list, sorted.
    fn persisted_cursor_sources(path: &std::path::Path) -> Vec<String> {
        let raw = fs::read_to_string(path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut sources: Vec<String> = value["cursors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cursor| cursor["source"].as_str().unwrap().to_string())
            .collect();
        sources.sort();
        sources
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
                "limitscope-usage-intel-test-{}-{}-{}",
                tag,
                std::process::id(),
                unique
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }
    }

    fn temp_store(tag: &str) -> (UsageIntelligenceStore, tempdir::TempDir) {
        let dir = tempdir::temp_dir(tag);
        let store = UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        );
        (store, dir)
    }

    #[test]
    fn bounded_scan_commit_is_atomic_when_replacement_fails() {
        let (store, dir) = temp_store("bounded-write-failure");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        let before = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let backup = dir.path().join("saved.json");
        fs::rename(&path, &backup).unwrap();
        fs::create_dir(&path).unwrap();
        let scan = || {
            scan_from(
                vec![event("zcode:new", "zai", "model", NOW_MS - 1_000)],
                NOW_MS - 1_000,
            )
        };
        assert!(store
            .apply_bounded_scan_at(
                store.generation(),
                USAGE_SOURCE_ZCODE,
                scan(),
                "backlog".into()
            )
            .is_err());
        let failed = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert_eq!(failed.watermark_ms, before.watermark_ms);
        assert_eq!(failed.state, SourceState::ReadFailure);
        assert!(store.stored_events().is_empty());
        assert!(store.set_enabled(false).is_err());
        assert_eq!(
            store.cursor(USAGE_SOURCE_ZCODE).unwrap().state,
            SourceState::ReadFailure
        );
        fs::remove_dir(&path).unwrap();
        fs::rename(&backup, &path).unwrap();
        store
            .apply_bounded_scan_at(
                store.generation(),
                USAGE_SOURCE_ZCODE,
                scan(),
                "backlog".into(),
            )
            .unwrap();
        let reopened = UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        let persisted = reopened.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert_eq!(persisted.state, SourceState::ScanCeiling);
        assert_eq!(persisted.watermark_ms, NOW_MS - 1_000);
        assert_eq!(reopened.stored_events().len(), 1);
    }

    #[test]
    fn failed_unbounded_zcode_write_revokes_complete_until_retry() {
        let (store, dir) = temp_store("unbounded-write-failure");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let saved = dir.path().join("saved.json");
        let coverage = |store: &UsageIntelligenceStore| {
            dto_coverage(&store.aggregate(UsageRange::Today, Some(NOW_MS)), USAGE_SOURCE_ZCODE)
                .coverage.clone()
        };
        assert_eq!(coverage(&store), "complete");
        fs::rename(&path, &saved).unwrap();
        fs::create_dir(&path).unwrap();
        let scan = || scan_from(vec![event("zcode:new", "zai", "model", NOW_MS - 1_000)], NOW_MS - 1_000);
        assert!(store.apply_scan_at(store.generation(), USAGE_SOURCE_ZCODE, scan()).is_err());
        assert_eq!(coverage(&store), "uncertain");
        assert!(store.stored_events().is_empty());
        fs::remove_dir(&path).unwrap();
        fs::rename(&saved, &path).unwrap();
        store.apply_scan_at(store.generation(), USAGE_SOURCE_ZCODE, scan()).unwrap();
        assert_eq!(coverage(&store), "complete");
        assert_eq!(store.stored_events().len(), 1);
    }

    fn event(id: &str, provider: &str, model: &str, event_at: i64) -> UsageEvent {
        UsageEvent {
            id: id.to_string(),
            source: USAGE_SOURCE_ZCODE.to_string(),
            provider: provider.to_string(),
            source_provider_id: format!("account:{provider}-plan"),
            model: model.to_string(),
            event_at,
            observed_at: NOW_MS,
            input_tokens: 100,
            cache_read_tokens: 50,
            cache_write_tokens: 10,
            output_tokens: 200,
            reasoning_tokens: 20,
            total_tokens: 380,
        }
    }

    fn scan_from(events: Vec<UsageEvent>, watermark: i64) -> SourceScan {
        SourceScan {
            watermark_ms: watermark,
            fingerprint: None,
            rejected: 0,
            usage_not_reported: 0,
            events,
            files: Vec::new(),
            batch_end: None,
        }
    }

    fn dto_coverage<'a>(dto: &'a UsageIntelligenceDto, source: &str) -> &'a SourceCoverageDto {
        dto.source_coverage
            .iter()
            .find(|coverage| coverage.source == source)
            .unwrap_or_else(|| panic!("missing coverage for {source}"))
    }

    // 1. serialization roundtrip: a persisted store reopens with the same
    //    events, cursors, enabled flag, and disclosure anchor.
    #[tokio::test]
    async fn coordinator_bounds_work_and_quota_finishes_while_scan_is_blocked() {
        let dir = tempdir::temp_dir("coordinator");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let store = std::sync::Arc::new(UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME), fixed_clock(), Box::new(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                entered_tx.send(()).unwrap();
                let _ = release_rx.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5));
                None
            })).with_opencode_db_resolver(Box::new(|| None)));
        store.set_enabled(true).unwrap();
        spawn_collection(store.clone());
        entered_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        for _ in 0..8 { spawn_collection(store.clone()); }
        let core = std::sync::Arc::new(crate::runtime::RuntimeCore::with_injections(
            Vec::new(), 5, Box::new(|| 0), Box::new(|| chrono::DateTime::from_timestamp_millis(NOW_MS).unwrap()))
            .with_usage_intelligence_store(Some(store.clone())));
        let quota = tokio::time::timeout(std::time::Duration::from_secs(1), core.run_cycle()).await;
        store.clear_owned().unwrap();
        store.set_enabled(false).unwrap();
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while store.is_collecting() { tokio::time::sleep(std::time::Duration::from_millis(5)).await; }
        }).await.unwrap();
        assert!(quota.is_ok(), "quota publication must not await the blocked collector");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(store.cursor(USAGE_SOURCE_ZCODE).is_none());
        assert!(store.cursor(USAGE_SOURCE_OPENCODE).is_none());
    }

    #[test]
    fn invalidated_scan_cannot_restore_cleared_or_disabled_state() {
        let (store, _dir) = temp_store("generation-fence");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, 0, None);
        let generation = store.generation();
        store.clear_owned().unwrap();
        assert!(store.apply_scan_at(generation, USAGE_SOURCE_ZCODE, scan_from(vec![event("zcode:late", "zai", "model", NOW_MS)], NOW_MS)).is_err());
        store.establish_baseline_at(generation, USAGE_SOURCE_ZCODE, NOW_MS, None);
        assert!(store.cursor(USAGE_SOURCE_ZCODE).is_none());
        assert!(store.events().is_empty());
        let generation = store.generation();
        store.set_enabled(false).unwrap();
        assert!(store.apply_scan_at(generation, USAGE_SOURCE_ZCODE, scan_from(vec![], NOW_MS)).is_err());
    }

    #[test]
    fn failed_scan_rolls_back_cursor_and_retry_commits_identical_batch() {
        let (store, dir) = temp_store("retry-scan");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, 0, None);
        let backup = dir.path().join("saved.json");
        fs::rename(&store.path, &backup).unwrap();
        fs::create_dir(&store.path).unwrap();
        let scan = || scan_from(vec![event("zcode:retry", "zai", "model", NOW_MS)], NOW_MS);
        assert!(store.apply_scan_at(store.generation(), USAGE_SOURCE_ZCODE, scan()).is_err());
        assert_eq!(store.cursor(USAGE_SOURCE_ZCODE).unwrap().watermark_ms, 0);
        assert!(store.events().is_empty());
        fs::remove_dir(&store.path).unwrap();
        fs::rename(backup, &store.path).unwrap();
        assert_eq!(store.apply_scan_at(store.generation(), USAGE_SOURCE_ZCODE, scan()).unwrap().accepted, 1);
        assert_eq!(store.cursor(USAGE_SOURCE_ZCODE).unwrap().watermark_ms, NOW_MS);
    }

    #[test]
    fn capacity_retains_backlog_then_retention_releases_it() {
        let (clock, now) = manual_clock(NOW_MS);
        let (store, _dir) = temp_store_with_clock("capacity", now);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, 0, None);
        store.state.lock().unwrap().events = (0..USAGE_EVENT_MAX).map(|i| event(&format!("zcode:full-{i}"), "zai", "model", NOW_MS-USAGE_EVENT_RETENTION_MS)).collect();
        let batch = || scan_from(vec![event("zcode:backlog", "zai", "model", NOW_MS)], NOW_MS);
        assert_eq!(store.apply_scan(USAGE_SOURCE_ZCODE, batch()).accepted, 0);
        let cursor = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert_eq!(cursor.watermark_ms, 0);
        assert_eq!(cursor.state, SourceState::ScanCeiling);
        *clock.lock().unwrap() += 1;
        assert_eq!(store.apply_scan(USAGE_SOURCE_ZCODE, batch()).accepted, 1);
        assert_eq!(store.events().len(), 1);
        assert_eq!(store.cursor(USAGE_SOURCE_ZCODE).unwrap().watermark_ms, NOW_MS);
    }

    #[test]
    fn invalid_supported_event_is_backed_up_before_later_save() {
        let (store, dir) = temp_store("invalid-event-recovery");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, 0, None);
        store.apply_scan(USAGE_SOURCE_ZCODE, scan_from(vec![event("zcode:valid", "zai", "model", NOW_MS)], NOW_MS));
        let mut blob: serde_json::Value = serde_json::from_str(&fs::read_to_string(&store.path).unwrap()).unwrap();
        blob["events"][0]["id"] = serde_json::json!("");
        let raw = serde_json::to_vec(&blob).unwrap();
        fs::write(&store.path, &raw).unwrap();
        let reopened = UsageIntelligenceStore::open_with(store.path.clone(), fixed_clock(), Box::new(|| None));
        assert!(reopened.events().is_empty());
        reopened.set_enabled(false).unwrap();
        assert!(fs::read_dir(dir.path()).unwrap().filter_map(Result::ok).any(|entry| {
            entry.file_name().to_string_lossy().contains("recovery-") && fs::read(entry.path()).unwrap() == raw
        }));
    }

    #[test]
    fn future_schema_survives_every_mutation_attempt() {
        let (store, _dir) = temp_store("future-preserved");
        let raw = r#"{"version":999,"enabled":true,"events":[],"cursors":[],"future":"keep"}"#;
        fs::write(&store.path, raw).unwrap();
        let reopened = UsageIntelligenceStore::open_with(store.path.clone(), fixed_clock(), Box::new(|| None));
        assert!(!reopened.enabled());
        assert!(reopened.set_enabled(true).is_err());
        assert!(reopened.clear_owned().is_err());
        assert!(!reopened.last_write_succeeded());
        assert_eq!(fs::read_to_string(&store.path).unwrap(), raw);
    }

    #[test]
    fn persisted_state_roundtrips_through_disk() {
        let (store, dir) = temp_store("roundtrip");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        let stats = store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![event("zcode:a", "zai", "GLM-5.3", NOW_MS - 30_000)],
                NOW_MS - 30_000,
            ),
        );
        assert_eq!(stats.accepted, 1);
        drop(store);

        let reopened = UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        assert!(reopened.enabled(), "the opt-in flag survives restart");
        assert_eq!(reopened.collection_started_at(), Some(NOW_MS));
        let dto = reopened.aggregate(UsageRange::SevenDays, None);
        assert_eq!(dto.events_in_range, 1);
        assert_eq!(dto.groups.len(), 1);
        assert_eq!(dto.groups[0].provider, "zai");
        assert_eq!(dto.groups[0].models[0].model, "GLM-5.3");
        assert_eq!(dto.sources.len(), 1, "the cursor survives restart");
    }

    // 2. retention: events older than 35 days are pruned on the next
    //    write; the boundary (exactly 35 days) is retained.
    #[test]
    fn retention_prunes_events_older_than_35_days() {
        let (store, _dir) = temp_store("retention");
        let edge = event(
            "zcode:edge",
            "zai",
            "GLM-5.3",
            NOW_MS - USAGE_EVENT_RETENTION_MS,
        );
        let old = event(
            "zcode:old",
            "zai",
            "GLM-5.3",
            NOW_MS - USAGE_EVENT_RETENTION_MS - 1,
        );
        let fresh = event("zcode:fresh", "zai", "GLM-5.3", NOW_MS - 1_000);
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(vec![edge, old, fresh], NOW_MS - 1_000),
        );
        // Retention is a store property (35 days), distinct from any view
        // range: assert on the stored events, not a 30d aggregation.
        assert_eq!(store.events().len(), 2, "age == 35d keeps, older drops");
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.events_in_range, 1, "only the fresh event is inside 30d");
    }

    // 2b. Retention is a store property, not a scan side effect: reopening
    //     the plane after the window has elapsed prunes the owned history at
    //     startup — no collection pass runs — and the pruned file is written.
    #[test]
    fn startup_retention_prunes_and_persists_without_any_scan() {
        let dir = tempdir::temp_dir("startup-retention");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let now = std::sync::Arc::new(std::sync::Mutex::new(NOW_MS));
        let store = UsageIntelligenceStore::open_with(
            path.clone(),
            clock_from(now.clone()),
            Box::new(|| None),
        );
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:expired", "zai", "GLM-5.3", NOW_MS - 34 * DAY_MS),
                    event("zcode:edge", "zai", "GLM-5.3", NOW_MS - 33 * DAY_MS),
                    event("zcode:recent", "zai", "GLM-5.3", NOW_MS - DAY_MS),
                ],
                NOW_MS - 1_000,
            ),
        );
        assert_eq!(
            retained_ids(&store),
            vec!["zcode:expired", "zcode:edge", "zcode:recent"],
            "every seeded event is inside the window when it is stored"
        );
        drop(store);

        // Nothing collects: the clock moves two days on and the plane is
        // simply reopened.
        *now.lock().unwrap() = NOW_MS + 2 * DAY_MS;
        let reopened = UsageIntelligenceStore::open_with(
            path.clone(),
            clock_from(now.clone()),
            Box::new(|| None),
        );
        assert_eq!(
            retained_ids(&reopened),
            vec!["zcode:edge", "zcode:recent"],
            "startup retention drops the event past the window and keeps the boundary"
        );
        assert_eq!(
            persisted_ids(&path),
            vec!["zcode:edge", "zcode:recent"],
            "the startup pass writes the pruned store, not just the in-memory state"
        );
        let cursor = reopened
            .cursor(USAGE_SOURCE_ZCODE)
            .expect("the cursor survives the startup prune");
        assert!(cursor.baselined);
        assert_eq!(cursor.watermark_ms, NOW_MS - 1_000);
        assert_eq!(
            persisted_cursor_sources(&path),
            vec!["zcode"],
            "the pruned file keeps the source cursor and its diagnostics"
        );
        assert!(reopened.last_write_succeeded());
    }

    // 2i. The stored events stay ordered by event time, so the retention
    //     check reads the oldest event instead of scanning the whole list.
    #[test]
    fn stored_events_stay_ordered_and_the_retention_check_reads_the_oldest() {
        let dir = tempdir::temp_dir("retention-order");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let now = std::sync::Arc::new(std::sync::Mutex::new(NOW_MS));
        let store = UsageIntelligenceStore::open_with(
            path.clone(),
            clock_from(now.clone()),
            Box::new(|| None),
        );
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:c", "zai", "GLM-5.3", NOW_MS - 3_000),
                    event("zcode:a", "zai", "GLM-5.3", NOW_MS - 9_000),
                    event("zcode:b", "zai", "GLM-5.3", NOW_MS - 6_000),
                ],
                NOW_MS - 1_000,
            ),
        );
        let times: Vec<i64> = store.stored_events().iter().map(|e| e.event_at).collect();
        assert_eq!(times, vec![NOW_MS - 9_000, NOW_MS - 6_000, NOW_MS - 3_000]);
        assert!(!store.retention_due(), "nothing is past the window yet");

        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![event("zcode:old", "zai", "GLM-5.3", NOW_MS - 34 * DAY_MS)],
                NOW_MS - 1_000,
            ),
        );
        let times: Vec<i64> = store.stored_events().iter().map(|e| e.event_at).collect();
        assert_eq!(
            times[0],
            NOW_MS - 34 * DAY_MS,
            "the oldest event stays first"
        );
        assert!(
            !store.retention_due(),
            "an event inside the window is not due yet"
        );

        *now.lock().unwrap() = NOW_MS + 2 * DAY_MS;
        assert!(store.retention_due(), "the oldest event is past the window");
    }

    // 2c. A disabled plane never resolves a source path, yet the 35-day
    //     window still runs: the cycle entry point used by the runtime
    //     prunes on its disabled branch instead of returning early.
    #[test]
    fn disabled_cycle_prunes_retention_and_never_probes_a_source() {
        let dir = tempdir::temp_dir("disabled-retention");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let now = std::sync::Arc::new(std::sync::Mutex::new(NOW_MS));
        let probes = std::sync::Arc::new(AtomicUsize::new(0));
        let zcode_probes = probes.clone();
        let opencode_probes = probes.clone();
        let (changed_tx, changed_rx) = std::sync::mpsc::channel();
        let store = std::sync::Arc::new(
            UsageIntelligenceStore::open_with(
                path.clone(),
                clock_from(now.clone()),
                Box::new(move || {
                    zcode_probes.fetch_add(1, Ordering::SeqCst);
                    None
                }),
            )
            .with_opencode_db_resolver(Box::new(move || {
                opencode_probes.fetch_add(1, Ordering::SeqCst);
                None
            }))
            .with_completion(Box::new(move |revision| {
                let _ = changed_tx.send(revision);
            })),
        );
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:idle", "zai", "GLM-5.3", NOW_MS - 34 * DAY_MS),
                    event("zcode:fresh", "zai", "GLM-5.3", NOW_MS - 1_000),
                ],
                NOW_MS - 1_000,
            ),
        );
        store.set_enabled(false).unwrap();
        let probes_before = probes.load(Ordering::SeqCst);

        *now.lock().unwrap() = NOW_MS + 2 * DAY_MS;
        spawn_collection(store.clone());
        changed_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the disabled retention pass reports its own completion");

        assert_eq!(
            probes.load(Ordering::SeqCst),
            probes_before,
            "a disabled plane resolves no source path, even for retention"
        );
        assert!(!store.enabled(), "retention never turns collection back on");
        assert_eq!(
            retained_ids(&store),
            vec!["zcode:fresh"],
            "the idle window still ages out while collection is off"
        );
        assert_eq!(persisted_ids(&path), vec!["zcode:fresh"]);
    }

    // 2d. Retention survives a failing source: the cycle prunes before the
    //     source is read, so a scan that can never succeed cannot hold the
    //     window open.
    #[test]
    fn failing_source_cycle_still_applies_retention() {
        let dir = tempdir::temp_dir("failing-retention");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let now = std::sync::Arc::new(std::sync::Mutex::new(NOW_MS));
        let store = UsageIntelligenceStore::open_with(
            path.clone(),
            clock_from(now.clone()),
            Box::new(|| None),
        )
        .with_opencode_db_resolver(Box::new(|| None));
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:stale", "zai", "GLM-5.3", NOW_MS - 34 * DAY_MS),
                    event("zcode:fresh", "zai", "GLM-5.3", NOW_MS - 1_000),
                ],
                NOW_MS - 1_000,
            ),
        );

        *now.lock().unwrap() = NOW_MS + 2 * DAY_MS;
        run_collection_pass(&store);

        assert_eq!(
            store.cursor(USAGE_SOURCE_ZCODE).map(|cursor| cursor.state),
            Some(SourceState::SourceAbsent),
            "the scan itself failed on this cycle"
        );
        assert_eq!(
            retained_ids(&store),
            vec!["zcode:fresh"],
            "the failed scan did not defer the retention window"
        );
        assert_eq!(persisted_ids(&path), vec!["zcode:fresh"]);
    }

    // 2e. A write-blocked store is never rewritten by retention: a foreign
    //     schema keeps its exact bytes.
    #[test]
    fn retention_never_rewrites_a_foreign_schema_blob() {
        let dir = tempdir::temp_dir("retention-foreign");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let raw = serde_json::json!({
            "version": 7,
            "enabled": true,
            "events": [{
                "id": "zcode:ancient",
                "source": USAGE_SOURCE_ZCODE,
                "provider": "zai",
                "model": "GLM-5.3",
                "eventAt": NOW_MS - 400 * DAY_MS,
                "observedAt": NOW_MS,
                "totalTokens": 380,
            }],
        })
        .to_string();
        fs::write(&path, &raw).unwrap();

        let store =
            UsageIntelligenceStore::open_with(path.clone(), fixed_clock(), Box::new(|| None));
        assert!(
            !store.prune_expired(),
            "a write-blocked store prunes nothing"
        );
        assert!(store.stored_events().is_empty());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            raw,
            "an unknown schema is preserved byte for byte"
        );
    }

    // 2f. Retention and scans share one serialization point: interleaving
    //     them never leaves memory and the durable file disagreeing.
    #[test]
    fn retention_and_scans_are_serialized_onto_one_consistent_file() {
        let dir = tempdir::temp_dir("retention-serialized");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let now = std::sync::Arc::new(std::sync::Mutex::new(NOW_MS));
        let store = std::sync::Arc::new(UsageIntelligenceStore::open_with(
            path.clone(),
            clock_from(now.clone()),
            Box::new(|| None),
        ));
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![event("zcode:seed", "zai", "GLM-5.3", NOW_MS - 34 * DAY_MS)],
                NOW_MS - 1_000,
            ),
        );
        *now.lock().unwrap() = NOW_MS + 2 * DAY_MS;

        let mut workers = Vec::new();
        for index in 0..4 {
            let store = store.clone();
            workers.push(std::thread::spawn(move || {
                if index % 2 == 0 {
                    store.prune_expired();
                } else {
                    let id = format!("zcode:live-{index}");
                    let at = NOW_MS + DAY_MS;
                    store.apply_scan(
                        USAGE_SOURCE_ZCODE,
                        scan_from(vec![event(&id, "zai", "GLM-5.3", at)], at),
                    );
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }

        let in_memory = retained_ids(&store);
        assert!(
            !in_memory.contains(&"zcode:seed".to_string()),
            "the expired seed is gone in every interleaving"
        );
        assert_eq!(in_memory.len(), 2, "both live scans are retained");
        assert_eq!(
            persisted_ids(&path),
            in_memory,
            "the durable file matches the committed memory state"
        );
    }

    /// Clears the Windows read-only attribute on drop so the temp directory
    /// can be removed even when an assertion fails mid-test.
    #[cfg(windows)]
    struct ReadOnlyStoreFile(std::path::PathBuf);

    #[cfg(windows)]
    impl Drop for ReadOnlyStoreFile {
        fn drop(&mut self) {
            if let Ok(metadata) = fs::metadata(&self.0) {
                let mut permissions = metadata.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                let _ = fs::set_permissions(&self.0, permissions);
            }
        }
    }

    // 2g. The failure path keeps the committed state: when the prune cannot
    //     be written, the events stay in memory and the file is untouched.
    #[cfg(windows)]
    #[test]
    fn failed_retention_write_restores_the_committed_events() {
        let dir = tempdir::temp_dir("retention-write-failure");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let now = std::sync::Arc::new(std::sync::Mutex::new(NOW_MS));
        let store = UsageIntelligenceStore::open_with(
            path.clone(),
            clock_from(now.clone()),
            Box::new(|| None),
        );
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:expired", "zai", "GLM-5.3", NOW_MS - 34 * DAY_MS),
                    event("zcode:fresh", "zai", "GLM-5.3", NOW_MS - 1_000),
                ],
                NOW_MS - 1_000,
            ),
        );
        let before = fs::read_to_string(&path).unwrap();

        let guard = ReadOnlyStoreFile(path.clone());
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();

        *now.lock().unwrap() = NOW_MS + 2 * DAY_MS;
        assert!(
            !store.prune_expired(),
            "renaming onto a read-only target fails"
        );
        assert_eq!(
            retained_ids(&store),
            vec!["zcode:expired", "zcode:fresh"],
            "a failed prune never drops events without a durable file to match"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            before,
            "the durable file is untouched"
        );
        assert!(
            !store.last_write_succeeded(),
            "the failed write is surfaced to the caller"
        );

        drop(guard);
        assert!(
            store.prune_expired(),
            "the next pass succeeds once the target is writable again"
        );
        assert_eq!(retained_ids(&store), vec!["zcode:fresh"]);
        assert_eq!(persisted_ids(&path), vec!["zcode:fresh"]);
    }

    // 2h. A slow source that holds the collection latch across many cycles
    //     does not defer the window: the busy cycle path prunes anyway, and
    //     the store's locks are free while the scan is blocked.
    #[test]
    fn busy_cycles_still_prune_while_a_source_scan_is_blocked() {
        let dir = tempdir::temp_dir("busy-retention");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let now = std::sync::Arc::new(std::sync::Mutex::new(NOW_MS));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        let (changed_tx, changed_rx) = std::sync::mpsc::channel();
        let store = std::sync::Arc::new(
            UsageIntelligenceStore::open_with(
                path.clone(),
                clock_from(now.clone()),
                Box::new(move || {
                    entered_tx.send(()).unwrap();
                    let _ = release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(std::time::Duration::from_secs(10));
                    None
                }),
            )
            .with_opencode_db_resolver(Box::new(|| None))
            .with_completion(Box::new(move |revision| {
                let _ = changed_tx.send(revision);
            })),
        );
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:expired", "zai", "GLM-5.3", NOW_MS - 34 * DAY_MS),
                    event("zcode:fresh", "zai", "GLM-5.3", NOW_MS - 1_000),
                ],
                NOW_MS - 1_000,
            ),
        );

        spawn_collection(store.clone());
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the scan is now blocked inside the source resolver");
        assert!(store.is_collecting(), "the collection latch is held");

        *now.lock().unwrap() = NOW_MS + 2 * DAY_MS;
        // Five cycles find the latch busy. The first prunes; the rest are
        // coalesced, so the store is written once, not once per cycle.
        for _ in 0..5 {
            spawn_collection(store.clone());
        }
        changed_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the busy cycle path applied the window");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while retained_ids(&store) != vec!["zcode:fresh"] {
            assert!(
                std::time::Instant::now() < deadline,
                "the expired event must be pruned while the scan is still blocked"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            persisted_ids(&path),
            vec!["zcode:fresh"],
            "the pruned store is durable while the scan is blocked"
        );
        assert!(
            changed_rx.try_recv().is_err(),
            "the coalesced cycles do not each write the store"
        );

        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while store.is_collecting() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            store.last_write_succeeded(),
            "the finished pass leaves the store writable"
        );
        assert_eq!(
            retained_ids(&store),
            vec!["zcode:fresh"],
            "the released scan cannot re-add an expired event"
        );
    }

    // 3. dedup: re-applying an overlapping scan never grows totals.
    #[test]
    fn overlapping_scans_are_idempotent() {
        let (store, _dir) = temp_store("dedup");
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 120_000, None);
        let events = vec![
            event("zcode:a", "zai", "GLM-5.3", NOW_MS - 90_000),
            event("zcode:b", "zai", "GLM-5.3-Flash", NOW_MS - 60_000),
        ];
        let first = store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(events.clone(), NOW_MS - 60_000),
        );
        assert_eq!(first.accepted, 2);
        let again = store.apply_scan(USAGE_SOURCE_ZCODE, scan_from(events, NOW_MS - 60_000));
        assert_eq!(again.accepted, 0, "no event is stored twice");
        assert_eq!(again.duplicates, 2);
        let dto = store.aggregate(UsageRange::Today, Some(NOW_MS - 6 * 60 * 60_000));
        assert_eq!(
            dto.totals.total_tokens,
            2 * 380,
            "totals are unchanged by the overlap"
        );
    }

    // 4. clear: owned data and cursors vanish, the disclosure anchor
    //    resets, but the enabled flag survives (clearing is not
    //    disabling); a later baseline re-anchors the disclosure.
    #[test]
    fn clear_owned_data_keeps_the_opt_in_and_rebaselines_cleanly() {
        let (store, _dir) = temp_store("clear");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![event("zcode:a", "zai", "GLM-5.3", NOW_MS - 30_000)],
                NOW_MS - 30_000,
            ),
        );
        assert!(store.clear_owned().unwrap());
        assert!(
            store.enabled(),
            "clearing data does not disable the feature"
        );
        assert_eq!(store.cursor(USAGE_SOURCE_ZCODE), None, "cursors are reset");
        assert_eq!(store.collection_started_at(), None);

        // While still enabled, the next collection re-baselines at the
        // current high-water mark — and a fresh baseline imports nothing.
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 5_000, None);
        assert_eq!(store.collection_started_at(), Some(NOW_MS));
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(
            dto.events_in_range, 0,
            "no old usage reappears after a clear"
        );
        assert!(dto.sources[0].last_scan.unwrap().accepted == 0);
    }

    // 5. clearing an empty store reports nothing removed and succeeds.
    #[test]
    fn clear_empty_store_is_an_idempotent_success() {
        let (store, _dir) = temp_store("clear-empty");
        assert!(!store.clear_owned().unwrap());
        assert!(!store.clear_owned().unwrap());
    }

    // 6. corrupt owned store: degrade to empty defaults — and the
    //    fail-closed direction for the opt-in flag is disabled.
    #[test]
    fn corrupt_store_degrades_to_empty_and_disabled() {
        let dir = tempdir::temp_dir("corrupt");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        std::fs::write(&path, "{not json at all").unwrap();
        let store =
            UsageIntelligenceStore::open_with(path.clone(), fixed_clock(), Box::new(|| None));
        assert!(
            !store.enabled(),
            "an unreadable opt-in flag must fail closed"
        );
        assert_eq!(store.collection_started_at(), None);
        // The store still functions afterwards.
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        assert!(store.cursor(USAGE_SOURCE_ZCODE).is_some());
        drop(store);
        let reopened = UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        assert!(reopened.enabled(), "the healed file roundtrips");
    }

    #[test]
    fn foreign_version_blob_is_discarded_whole() {
        let dir = tempdir::temp_dir("version");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 99,
                "enabled": true,
                "events": [ { "id": "zcode:x" } ]
            })
            .to_string(),
        )
        .unwrap();
        let store = UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        assert!(!store.enabled(), "unknown layouts never blend in");
    }

    // 6b. a failure-only cursor record is not a baseline: when the source
    //     later appears, the real backlog-skipping baseline replaces it —
    //     history before that point is still never imported.
    #[test]
    fn pre_baseline_failure_does_not_become_a_baseline() {
        let (store, _dir) = temp_store("baseline-after-failure");
        store.set_enabled(true).unwrap();
        store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::SourceAbsent, None);
        let cursor = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert!(!cursor.baselined, "a failure record carries no baseline");
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        let cursor = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert!(cursor.baselined);
        assert_eq!(cursor.watermark_ms, NOW_MS - 60_000);
        assert_eq!(store.collection_started_at(), Some(NOW_MS));
    }

    // 7. aggregation: Today / 7d / 30d windows, provider and model
    //    grouping, dimension sums, and the incomplete-history disclosure.
    #[test]
    fn aggregation_groups_by_provider_and_model_with_correct_windows() {
        // Baseline 31 days ago (clock-wise), so the 30d view's history is
        // complete; events arrive across the window afterwards.
        let (now, clock) = manual_clock(NOW_MS - 31 * 24 * 60 * 60_000);
        let (store, _dir) = temp_store_with_clock("aggregate", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 31 * 24 * 60 * 60_000, None);
        *now.lock().unwrap() = NOW_MS;
        let midnight = NOW_MS - 8 * 60 * 60_000;
        let events = vec![
            event("zcode:t1", "zai", "GLM-5.3", midnight + 60_000),
            event("zcode:t2", "zai", "GLM-5.3", NOW_MS - 2 * 24 * 60 * 60_000),
            event(
                "zcode:t3",
                "openai-codex",
                "gpt-5.4",
                NOW_MS - 2 * 24 * 60 * 60_000,
            ),
            // Outside even the 30d window (and outside 35d retention, so it
            // is pruned from the store entirely).
            event("zcode:t4", "zai", "GLM-5.3", NOW_MS - 36 * 24 * 60 * 60_000),
        ];
        store.apply_scan(USAGE_SOURCE_ZCODE, scan_from(events, NOW_MS - 1_000));

        let today = store.aggregate(UsageRange::Today, Some(midnight));
        assert_eq!(today.range, "today");
        assert_eq!(today.events_in_range, 1);
        assert_eq!(today.groups[0].models[0].model, "GLM-5.3");

        let seven = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(seven.range, "7d");
        assert_eq!(seven.events_in_range, 3);
        assert_eq!(
            seven.groups.len(),
            2,
            "zai and openai-codex are separate providers"
        );
        let codex = seven
            .groups
            .iter()
            .find(|g| g.provider == "openai-codex")
            .unwrap();
        assert_eq!(codex.models.len(), 1);
        assert_eq!(codex.models[0].model, "gpt-5.4");
        assert_eq!(codex.models[0].total_tokens, 380);
        assert_eq!(codex.models[0].input_tokens, 100);
        assert_eq!(codex.models[0].cache_read_tokens, 50);
        assert_eq!(codex.models[0].cache_write_tokens, 10);
        assert_eq!(codex.models[0].output_tokens, 200);
        assert_eq!(codex.models[0].reasoning_tokens, 20);
        assert_eq!(seven.totals.total_tokens, 3 * 380);

        let thirty = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(
            thirty.events_in_range, 3,
            "the out-of-window event is excluded"
        );
        // The disclosure: collection (31 days) is older than the 30d range
        // start → history is complete for this range.
        assert!(!thirty.incomplete_history);
        assert!(thirty.collection_started_at.is_some());
    }

    #[test]
    fn incomplete_history_disclosure_flags_ranges_older_than_collection() {
        // Collection (baseline) 3 days ago; the 7d view reaches further
        // back than that, the Today view does not.
        let (now, clock) = manual_clock(NOW_MS - 3 * 24 * 60 * 60_000);
        let (store, _dir) = temp_store_with_clock("incomplete", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 3 * 24 * 60 * 60_000, None);
        *now.lock().unwrap() = NOW_MS;
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![event("zcode:a", "zai", "GLM-5.3", NOW_MS - 1_000)],
                NOW_MS - 1_000,
            ),
        );
        let seven = store.aggregate(UsageRange::SevenDays, None);
        assert!(
            seven.incomplete_history,
            "7d reaches further back than collection"
        );
        let today = store.aggregate(UsageRange::Today, Some(NOW_MS - 8 * 60 * 60_000));
        assert!(!today.incomplete_history);
    }

    #[test]
    fn enabled_plane_without_a_baseline_does_not_claim_complete_history() {
        let (_now, clock) = manual_clock(NOW_MS);
        let (store, _dir) = temp_store_with_clock("coverage-never-started", clock);
        store.set_enabled(true).unwrap();

        let waiting = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(waiting.collection_started_at, None);
        assert!(waiting.source_coverage.is_empty());
        assert_eq!(waiting.events_in_range, 0);
        assert!(
            waiting.incomplete_history,
            "enabled collection without a baseline has no known coverage"
        );

        // The disabled empty plane keeps the historical empty-state verdict.
        store.set_enabled(false).unwrap();
        let off = store.aggregate(UsageRange::SevenDays, None);
        assert!(!off.incomplete_history);
        assert!(off.source_coverage.is_empty());
    }

    #[test]
    fn per_source_coverage_is_complete_when_every_baseline_covers_the_range() {
        let range_start = NOW_MS - 7 * DAY_MS;
        let (now, clock) = manual_clock(range_start - DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-complete", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        store.establish_baseline(USAGE_SOURCE_OPENCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS;

        let dto = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(dto.source_coverage.len(), 2);
        for source in [USAGE_SOURCE_ZCODE, USAGE_SOURCE_OPENCODE] {
            let coverage = dto_coverage(&dto, source);
            assert_eq!(coverage.coverage, "complete");
            assert_eq!(coverage.reason, None);
            assert_eq!(coverage.covered_from, None);
        }
        assert!(!dto.incomplete_history);

        // A bounded scan is documented progress, not a coverage gap.
        store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::ScanCeiling, None);
        let ceiling = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(
            dto_coverage(&ceiling, USAGE_SOURCE_ZCODE).coverage,
            "uncertain"
        );
        assert!(ceiling.incomplete_history);
    }

    #[test]
    fn later_source_baseline_is_incomplete_from_its_baseline_instant() {
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-late", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS - 3 * DAY_MS;
        store.establish_baseline(USAGE_SOURCE_OPENCODE, NOW_MS - 4 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS;
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![event("zcode:late", "zai", "GLM-5.3", NOW_MS - DAY_MS)],
                NOW_MS - DAY_MS,
            ),
        );

        let dto = store.aggregate(UsageRange::SevenDays, None);
        let zcode = dto_coverage(&dto, USAGE_SOURCE_ZCODE);
        let codex = dto_coverage(&dto, USAGE_SOURCE_OPENCODE);
        assert_eq!(zcode.coverage, "complete");
        assert_eq!(codex.coverage, "incomplete");
        assert_eq!(codex.reason.as_deref(), Some("lateBaseline"));
        assert_eq!(
            codex.covered_from,
            Some(canonical_timestamp(NOW_MS - 3 * DAY_MS))
        );
        assert!(dto.incomplete_history);
        assert_eq!(dto.events_in_range, 1);
        assert_eq!(dto.totals.total_tokens, 380);
    }

    #[test]
    fn global_anchor_before_range_does_not_hide_a_later_source_baseline() {
        let range_start = NOW_MS - 7 * DAY_MS;
        let (now, clock) = manual_clock(NOW_MS - 12 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-regression", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 12 * DAY_MS, None);
        let global_start = store.collection_started_at().unwrap();
        assert!(global_start < range_start);
        *now.lock().unwrap() = NOW_MS - 2 * DAY_MS;
        store.establish_baseline(USAGE_SOURCE_OPENCODE, NOW_MS - 3 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS;

        let dto = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(
            dto.collection_started_at,
            Some(canonical_timestamp(global_start))
        );
        assert!(dto.incomplete_history);
        let late = dto_coverage(&dto, USAGE_SOURCE_OPENCODE);
        assert_eq!(late.coverage, "incomplete");
        assert_eq!(late.reason.as_deref(), Some("lateBaseline"));
    }

    #[test]
    fn failure_only_source_has_no_baseline_and_is_uncertain() {
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-no-baseline", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS;
        store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::ReadFailure, None);

        let dto = store.aggregate(UsageRange::SevenDays, None);
        let codex = dto_coverage(&dto, USAGE_SOURCE_OPENCODE);
        assert_eq!(codex.coverage, "uncertain");
        assert_eq!(codex.reason.as_deref(), Some("noBaseline"));
        assert_eq!(codex.covered_from, None);
        assert_eq!(codex.last_observed_at, None);
        assert!(dto.incomplete_history);
    }

    #[test]
    fn baselined_failure_is_uncertain_with_its_last_observed_anchor() {
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-failure", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        let baseline = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        *now.lock().unwrap() = NOW_MS;
        store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::SourceAbsent, None);

        let dto = store.aggregate(UsageRange::SevenDays, None);
        let zcode = dto_coverage(&dto, USAGE_SOURCE_ZCODE);
        assert_eq!(zcode.coverage, "uncertain");
        assert_eq!(zcode.reason.as_deref(), Some("sourceAbsent"));
        assert_eq!(
            zcode.last_observed_at,
            Some(canonical_timestamp(baseline.last_observed_at))
        );
        assert!(dto.incomplete_history);
    }

    #[test]
    fn paused_collection_after_an_old_baseline_is_uncertain() {
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-paused", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_OPENCODE, NOW_MS - 10 * DAY_MS, None);
        let baseline = store.cursor(USAGE_SOURCE_OPENCODE).unwrap();
        *now.lock().unwrap() = NOW_MS;
        // The selected source reader pauses fail-closed when the rollout tree cannot
        // be enumerated safely. An old baseline must not turn that into a
        // completeness claim.
        store.mark_source_state(
            USAGE_SOURCE_OPENCODE,
            SourceState::CollectionPaused,
            Some("the rollout tree holds too many files; collection is paused fail-closed"
                .to_string()),
        );

        let dto = store.aggregate(UsageRange::SevenDays, None);
        let coverage = dto_coverage(&dto, USAGE_SOURCE_OPENCODE);
        assert_eq!(coverage.coverage, "uncertain");
        assert_eq!(coverage.reason.as_deref(), Some("collectionPaused"));
        assert_eq!(
            coverage.last_observed_at,
            Some(canonical_timestamp(baseline.last_observed_at))
        );
        assert!(dto.incomplete_history);
        let diagnostics = dto
            .sources
            .iter()
            .find(|source| source.source == USAGE_SOURCE_OPENCODE)
            .unwrap();
        assert_eq!(diagnostics.state, "collectionPaused");

        // A bounded-progress ceiling is uncertain: it committed only a prefix
        // and continues on the next scan. Only the fail-closed pause is a
        // coverage gap.
        store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::ScanCeiling, None);
        let progressing = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(
            dto_coverage(&progressing, USAGE_SOURCE_OPENCODE).coverage,
            "uncertain"
        );
        assert!(progressing.incomplete_history);
    }

    #[test]
    fn persisted_source_states_remain_backward_compatible() {
        // The new variant is additive: blobs written by older code can only
        // contain the pre-existing states, which keep their wire spelling.
        assert_eq!(BLOB_VERSION, 1);
        let old: SourceState = serde_json::from_str("\"scanCeiling\"").unwrap();
        assert_eq!(old, SourceState::ScanCeiling);
        let ok: SourceState = serde_json::from_str("\"ok\"").unwrap();
        assert_eq!(ok, SourceState::Ok);
        let paused: SourceState = serde_json::from_str("\"collectionPaused\"").unwrap();
        assert_eq!(paused, SourceState::CollectionPaused);
    }

    #[test]
    fn legacy_blob_without_the_paused_variant_still_loads() {
        // A blob written before `collectionPaused` existed carries only the
        // old states. The additive variant must not bump BLOB_VERSION nor
        // disturb deserialization of the envelope.
        let dir = tempdir::temp_dir("legacy-blob");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let baseline = NOW_MS - 10 * DAY_MS;
        let watermark = NOW_MS - 9 * DAY_MS;
        // Envelope fields are snake_case (the nested cursors are camelCase);
        // this is the exact shape an older build persisted.
        let legacy = format!(
            r#"{{"version":1,"enabled":true,"collection_started_at":{baseline},"events":[],"cursors":[{{"source":"opencode","watermarkMs":{watermark},"baselineWatermarkMs":{watermark},"baselined":true,"baselinedAt":{baseline},"lastObservedAt":{watermark},"state":"scanCeiling"}}]}}"#
        );
        std::fs::write(&path, legacy).unwrap();

        let store = UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        assert_eq!(store.collection_started_at(), Some(baseline));
        let dto = store.aggregate(UsageRange::SevenDays, None);
        let diagnostics = dto
            .sources
            .iter()
            .find(|source| source.source == USAGE_SOURCE_OPENCODE)
            .unwrap();
        assert_eq!(diagnostics.state, "scanCeiling");
        // A bounded-progress ceiling keeps its meaning under the new rules.
        assert_eq!(dto_coverage(&dto, USAGE_SOURCE_OPENCODE).coverage, "uncertain");
        assert!(dto.incomplete_history);
    }

    #[test]
    fn source_watermark_ahead_of_the_store_clock_bounds_coverage() {
        let range_start = NOW_MS - 7 * DAY_MS;
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-skew", clock);
        store.set_enabled(true).unwrap();
        // The source clock is ahead of the store clock: the baseline instant
        // looks early, but the authoritative source watermark says the
        // source itself had not reached the range start yet.
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 3 * DAY_MS, None);
        let cursor = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert!(cursor.baselined_at <= range_start);
        assert!(cursor.baseline_watermark_ms > range_start);
        *now.lock().unwrap() = NOW_MS;

        let dto = store.aggregate(UsageRange::SevenDays, None);
        let coverage = dto_coverage(&dto, USAGE_SOURCE_ZCODE);
        assert_eq!(coverage.coverage, "incomplete");
        assert_eq!(coverage.reason.as_deref(), Some("lateBaseline"));
        assert_eq!(
            coverage.covered_from,
            Some(canonical_timestamp(NOW_MS - 3 * DAY_MS))
        );
        assert!(dto.incomplete_history);
    }

    #[test]
    fn disable_reenable_then_rebaseline_reports_the_unknown_gap() {
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-reenable", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS - 5 * DAY_MS;
        store.set_enabled(false).unwrap();
        *now.lock().unwrap() = NOW_MS - 3 * DAY_MS;
        store.set_enabled(true).unwrap();

        let pending = store.aggregate(UsageRange::SevenDays, None);
        let coverage = dto_coverage(&pending, USAGE_SOURCE_ZCODE);
        assert_eq!(coverage.coverage, "incomplete");
        assert_eq!(coverage.reason.as_deref(), Some("pendingRebaseline"));
        assert_eq!(coverage.covered_from, None);
        assert!(pending.incomplete_history);

        *now.lock().unwrap() = NOW_MS - 2 * DAY_MS;
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 3 * DAY_MS, None);
        let rebaselined = store.aggregate(UsageRange::SevenDays, None);
        let coverage = dto_coverage(&rebaselined, USAGE_SOURCE_ZCODE);
        assert_eq!(coverage.coverage, "incomplete");
        assert_eq!(coverage.reason.as_deref(), Some("lateBaseline"));
        assert_eq!(
            coverage.covered_from,
            Some(canonical_timestamp(NOW_MS - 2 * DAY_MS))
        );
        assert!(rebaselined.incomplete_history);
    }

    #[test]
    fn source_coverage_never_changes_observed_token_totals() {
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-totals", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS;
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:a", "zai", "GLM-5.3", NOW_MS - DAY_MS),
                    event("zcode:b", "zai", "GLM-5.3", NOW_MS - 1_000),
                ],
                NOW_MS - 1_000,
            ),
        );
        let before = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(before.events_in_range, 2);
        assert_eq!(before.totals.total_tokens, 760);

        store.establish_baseline(USAGE_SOURCE_OPENCODE, NOW_MS - DAY_MS, None);
        let after = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(after.events_in_range, before.events_in_range);
        assert_eq!(after.totals.total_tokens, before.totals.total_tokens);
        assert_eq!(
            serde_json::to_value(&after.groups).unwrap(),
            serde_json::to_value(&before.groups).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.daily).unwrap(),
            serde_json::to_value(&before.daily).unwrap(),
            "coverage verdicts must not re-anchor the daily projection"
        );
        assert_eq!(
            dto_coverage(&after, USAGE_SOURCE_OPENCODE).coverage,
            "incomplete"
        );
        assert!(after.incomplete_history);
    }

    #[test]
    fn missing_history_is_not_measured_zero_usage() {
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-missing-zero", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS;

        let complete = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(complete.totals.events, 0);
        assert_eq!(
            dto_coverage(&complete, USAGE_SOURCE_ZCODE).coverage,
            "complete"
        );
        assert!(!complete.incomplete_history);

        store.establish_baseline(USAGE_SOURCE_OPENCODE, NOW_MS - DAY_MS, None);
        let incomplete = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(incomplete.totals.events, 0);
        assert_eq!(
            dto_coverage(&incomplete, USAGE_SOURCE_OPENCODE).coverage,
            "incomplete"
        );
        assert!(
            incomplete.incomplete_history,
            "a zero-event uncovered source is missing history, not confirmed zero usage"
        );
    }

    #[test]
    fn disabled_sources_are_uncertain_even_with_an_old_global_anchor() {
        let (now, clock) = manual_clock(NOW_MS - 10 * DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-disabled", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS;
        store.set_enabled(false).unwrap();

        let dto = store.aggregate(UsageRange::SevenDays, None);
        let coverage = dto_coverage(&dto, USAGE_SOURCE_ZCODE);
        assert_eq!(coverage.coverage, "uncertain");
        assert_eq!(coverage.reason.as_deref(), Some("monitoringDisabled"));
        assert!(dto.incomplete_history);
    }

    #[test]
    fn dto_compatibility_boundaries_and_additive_coverage_wire_shape() {
        let range_start = NOW_MS - 7 * DAY_MS;
        let today_start = NOW_MS - 8 * 60 * 60_000;
        let (now, clock) = manual_clock(range_start - DAY_MS);
        let (store, _dir) = temp_store_with_clock("coverage-wire", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = range_start;
        // Equality is covered from the range start.
        store.establish_baseline(USAGE_SOURCE_OPENCODE, NOW_MS - 10 * DAY_MS, None);
        *now.lock().unwrap() = NOW_MS;

        let dto = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(dto.range_start, canonical_timestamp(range_start));
        assert_eq!(dto_coverage(&dto, USAGE_SOURCE_OPENCODE).coverage, "complete");
        assert!(!dto.incomplete_history);

        let wire = serde_json::to_value(&dto).unwrap();
        for key in [
            "schemaVersion",
            "range",
            "rangeStart",
            "rangeEnd",
            "generatedAt",
            "enabled",
            "eventsInRange",
            "groups",
            "totals",
            "collectionStartedAt",
            "incompleteHistory",
            "sources",
            "daily",
            "semantics",
            "sourceCoverage",
        ] {
            assert!(wire.get(key).is_some(), "missing wire field {key}");
        }
        assert!(wire["groups"].is_array());
        assert!(wire["daily"].is_array());
        assert!(wire["sources"].is_array());
        for key in [
            "events",
            "inputTokens",
            "cacheReadTokens",
            "cacheWriteTokens",
            "outputTokens",
            "reasoningTokens",
            "totalTokens",
        ] {
            assert!(
                wire["totals"].get(key).is_some(),
                "missing totals field {key}"
            );
        }
        for source in wire["sources"].as_array().unwrap() {
            for key in ["source", "state", "watermarkAt", "baselinedAt"] {
                assert!(source.get(key).is_some(), "missing source field {key}");
            }
        }
        let coverage = wire["sourceCoverage"].as_array().unwrap();
        assert_eq!(coverage.len(), 2);
        for entry in coverage {
            assert_eq!(entry["coverage"], "complete");
            assert!(entry.get("reason").is_none());
            assert!(entry.get("coveredFrom").is_none());
            assert!(entry.get("lastObservedAt").is_none());
        }

        let today = store.aggregate(UsageRange::Today, Some(today_start));
        assert_eq!(today.range_start, canonical_timestamp(today_start));
        assert!(!today.incomplete_history);
    }

    // 8. today boundaries: an implausible webview midnight degrades to
    //    the trailing 24 hours instead of aggregating a wrong window.
    #[test]
    fn implausible_today_boundary_degrades_to_trailing_day() {
        let (store, _dir) = temp_store("today-boundary");
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 3 * 24 * 60 * 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:a", "zai", "GLM-5.3", NOW_MS - 30 * 60 * 60_000),
                    event("zcode:b", "zai", "GLM-5.3", NOW_MS - 1_000),
                ],
                NOW_MS - 1_000,
            ),
        );
        let dto = store.aggregate(UsageRange::Today, Some(NOW_MS - 10 * 24 * 60 * 60_000));
        assert_eq!(
            dto.events_in_range, 1,
            "a 10-day-old 'midnight' is not trusted"
        );
    }

    // 9. validation: invalid events are rejected, not stored.
    #[test]
    fn invalid_events_reject_rather_than_guess() {
        let (store, _dir) = temp_store("invalid");
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        let mut blank_model = event("zcode:m", "zai", "   ", NOW_MS - 30_000);
        blank_model.model = "   ".to_string();
        let mut future = event("zcode:f", "zai", "GLM-5.3", NOW_MS + 60 * 60_000);
        future.event_at = NOW_MS + 60 * 60_000;
        let mut wrong_source_prefix = event("codex:x", "zai", "GLM-5.3", NOW_MS - 30_000);
        wrong_source_prefix.source = USAGE_SOURCE_ZCODE.to_string();
        wrong_source_prefix.id = "codex:x".to_string();
        let stats = store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![blank_model, future, wrong_source_prefix],
                NOW_MS - 30_000,
            ),
        );
        assert_eq!(stats.accepted, 0);
        assert_eq!(stats.rejected, 3);
    }

    // 10. disabled state renders as "disabled" in diagnostics, and an
    //     enabled healthy cursor renders its persisted state.
    #[test]
    fn source_state_renders_disabled_before_the_persisted_state() {
        assert_eq!(
            rendered_source_state(SourceState::Ok, false, false),
            "disabled"
        );
        assert_eq!(rendered_source_state(SourceState::Ok, true, false), "ok");
        assert_eq!(
            rendered_source_state(SourceState::ReadFailure, true, true),
            "collecting"
        );
        assert_eq!(
            rendered_source_state(SourceState::SchemaUnsupported, true, false),
            "schemaUnsupported"
        );
    }

    // 11. the disabled store performs zero resolver calls — the opt-in
    //     gate sits before any path resolution or probe.
    #[test]
    fn disabled_store_never_resolves_the_source_path() {
        let dir = tempdir::temp_dir("disabled-probe");
        let probes = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = probes.clone();
        let store = UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                None
            }),
        );
        assert!(!store.enabled());
        let outcome = collect_zcode(&store);
        assert!(matches!(
            outcome,
            crate::usage_source_zcode::ZcodeScanOutcome::Disabled
        ));
        assert_eq!(probes.load(Ordering::SeqCst), 0, "no probe while disabled");
    }

    // 12. file-based source cursors (Codex) roundtrip through the
    //     persisted store and survive restarts intact.
    #[test]
    fn file_cursors_roundtrip_through_disk() {
        let (store, dir) = temp_store("file-cursors");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        store.set_enabled(true).unwrap();
        let files = vec![
            SourceFileCursor {
                path: "sessions/2026/10/06/rollout-a.jsonl".to_string(),
                offset: 100,
                size: 100,
                mtime_ms: 123,
                identity: "session-a".to_string(),
                modern: true,
                refused: false,
                turns: Vec::new(),
            },
            SourceFileCursor {
                path: "sessions/2026/10/06/rollout-junk.jsonl".to_string(),
                offset: 50,
                size: 50,
                mtime_ms: 123,
                identity: String::new(),
                modern: false,
                refused: true,
                turns: Vec::new(),
            },
        ];
        store.establish_baseline_with_files(
            USAGE_SOURCE_OPENCODE,
            NOW_MS - 60_000,
            None,
            files.clone(),
        );
        drop(store);

        let reopened = UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        let cursor = reopened.cursor(USAGE_SOURCE_OPENCODE).expect("cursor survives");
        assert!(cursor.baselined);
        assert_eq!(cursor.files, files, "per-file cursors survive restart");
        assert_eq!(reopened.collection_started_at(), Some(NOW_MS));

        // A scan from the file-based source replaces the file list
        // wholesale (reflecting deletions) without disturbing ZCode.
        let mut updated = files.clone();
        updated[0].offset = 200;
        updated[0].size = 200;
        updated.remove(1); // the junk file was deleted by its owner
        let stats = reopened.apply_scan(
            USAGE_SOURCE_OPENCODE,
            SourceScan {
                watermark_ms: NOW_MS - 30_000,
                fingerprint: None,
                rejected: 0,
                usage_not_reported: 0,
                events: Vec::new(),
                files: updated,
                batch_end: None,
            },
        );
        assert_eq!(stats.accepted, 0);
        let cursor = reopened.cursor(USAGE_SOURCE_OPENCODE).unwrap();
        assert_eq!(cursor.files.len(), 1);
        assert_eq!(cursor.files[0].offset, 200);
        assert_eq!(reopened.cursor(USAGE_SOURCE_ZCODE), None);
    }

    // 13. regression (v0.8.9 crash): the enable path spawns the first
    //     collection pass from the synchronous Tauri command — a plain IPC
    //     callback thread with no entered Tokio runtime. The 0.8.9
    //     implementation used `tokio::task::spawn_blocking`, which panics
    //     in exactly that context ("there is no reactor running"), and the
    //     unwind across the FFI boundary aborted the whole process. The
    //     runtime bridge (`tauri::async_runtime::spawn_blocking`, which
    //     lazily initializes and uses the app-global runtime) must survive
    //     the same context, and the detached pass must run to completion.
    #[test]
    fn spawn_collection_survives_plain_thread_without_tokio_runtime() {
        let (store, _dir) = temp_store("spawn_plain_thread");
        // Absent resolvers for every source: the pass never probes a real
        // tree, and on completion it marks all three sources sourceAbsent
        // — a deterministic completion signal for the detached pass.
        let store = std::sync::Arc::new(
            store
                .with_codex_home_resolver(Box::new(|| None))
                .with_opencode_db_resolver(Box::new(|| None)),
        );
        store.set_enabled(true).unwrap();

        // The enable command executes on a plain thread outside any Tokio
        // runtime; reproduce exactly that context. The join asserts the
        // call itself does not panic — on the 0.8.9 code it surfaces the
        // "no reactor running" panic as a test failure instead of letting
        // it abort the test process.
        let handle = std::thread::spawn({
            let store = store.clone();
            move || spawn_collection(store)
        });
        handle
            .join()
            .expect("spawn_collection must not panic on a thread with no Tokio runtime");

        // The pass is detached: wait (bounded) for it to finish rather
        // than assuming it, via the only state a fully-absent pass leaves
        // behind — every source reporting sourceAbsent.
        let sources = [
            USAGE_SOURCE_ZCODE,
            USAGE_SOURCE_OPENCODE,
            USAGE_SOURCE_OPENCODE,
        ];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let all_absent = sources.iter().all(|source| {
                store
                    .cursor(source)
                    .is_some_and(|cursor| cursor.state == SourceState::SourceAbsent)
            });
            if all_absent {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the detached collection pass never completed (no sourceAbsent diagnostics)"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // ---------- disable → re-enable (fresh-baseline) semantics ----------

    // 19. the disable → re-enable transition marks every existing
    //     cursor for a fresh baseline; a first enable (no cursors) and
    //     the disable itself mark nothing; the fresh baseline replaces
    //     the pending cursor, clears the flag, and keeps the disclosure
    //     anchor; an ordinary baselined cursor stays idempotent.
    #[test]
    fn reenable_marks_existing_cursors_pending_rebaseline() {
        let (store, _dir) = temp_store("pending");
        store.set_enabled(true).unwrap();
        assert!(
            store.cursor_needs_baseline(USAGE_SOURCE_ZCODE),
            "no cursor yet: the first pass must baseline"
        );
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        assert!(!store.cursor_needs_baseline(USAGE_SOURCE_ZCODE));
        assert!(!store.cursor(USAGE_SOURCE_ZCODE).unwrap().pending_rebaseline);

        store.set_enabled(false).unwrap();
        assert!(
            !store
                .cursor(USAGE_SOURCE_ZCODE)
                .unwrap()
                .pending_rebaseline,
            "disabling alone does not mark anything"
        );
        store.set_enabled(false).unwrap(); // idempotent: still no transition
        store.set_enabled(true).unwrap(); // the transition marks every cursor
        assert!(store.cursor(USAGE_SOURCE_ZCODE).unwrap().pending_rebaseline);
        assert!(store.cursor_needs_baseline(USAGE_SOURCE_ZCODE));

        // The fresh baseline replaces the pending cursor, clears the
        // flag, and keeps the original disclosure anchor.
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 5_000, None);
        let cursor = store.cursor(USAGE_SOURCE_ZCODE).unwrap();
        assert!(!cursor.pending_rebaseline);
        assert_eq!(cursor.watermark_ms, NOW_MS - 5_000);
        assert_eq!(cursor.baseline_watermark_ms, NOW_MS - 5_000);
        assert_eq!(store.collection_started_at(), Some(NOW_MS));

        // An ordinary baselined cursor is still never re-anchored.
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 1_000, None);
        assert_eq!(
            store.cursor(USAGE_SOURCE_ZCODE).unwrap().watermark_ms,
            NOW_MS - 5_000
        );
    }

    // 20. the pending re-baseline survives a persisted restart: the flag
    //     roundtrips through disk, so a quit between re-enable and the
    //     first pass still skips the disabled period later.
    #[test]
    fn pending_rebaseline_survives_restart() {
        let (store, dir) = temp_store("pending-restart");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.set_enabled(false).unwrap();
        store.set_enabled(true).unwrap();
        drop(store);

        let reopened =
            UsageIntelligenceStore::open_with(dir.path().join(USAGE_INTELLIGENCE_FILE_NAME), fixed_clock(), Box::new(|| None));
        assert!(reopened.enabled());
        assert!(reopened.cursor_needs_baseline(USAGE_SOURCE_ZCODE));
        assert!(reopened.cursor(USAGE_SOURCE_ZCODE).unwrap().pending_rebaseline);
    }

    // 21. batch positions: a scan's committed end position persists on
    //     the cursor, survives restart, is left untouched by scans
    //     without one, and is replaced by a later batch's position.
    #[test]
    fn batch_positions_apply_and_roundtrip_through_disk() {
        let (store, dir) = temp_store("batch-pos");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        let first = SourceBatchEnd {
            stamp_ms: NOW_MS - 30_000,
            id: "row-a".to_string(),
        };
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            SourceScan {
                watermark_ms: NOW_MS - 30_000,
                fingerprint: None,
                rejected: 0,
                usage_not_reported: 0,
                events: vec![event("zcode:row-a", "zai", "GLM-5.3", NOW_MS - 30_000)],
                files: Vec::new(),
                batch_end: Some(first.clone()),
            },
        );
        assert_eq!(
            store.cursor(USAGE_SOURCE_ZCODE).unwrap().batch_end,
            Some(first.clone())
        );
        drop(store);

        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        let reopened =
            UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        assert_eq!(
            reopened.cursor(USAGE_SOURCE_ZCODE).unwrap().batch_end,
            Some(first),
            "the resume position survives restart"
        );

        // A scan without a batch end (idle or file-based) leaves the
        // stored position untouched.
        reopened.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(vec![event("zcode:row-b", "zai", "GLM-5.3", NOW_MS - 10_000)], NOW_MS - 10_000),
        );
        assert!(reopened.cursor(USAGE_SOURCE_ZCODE).unwrap().batch_end.is_some());

        // A later committed batch replaces the position.
        let second = SourceBatchEnd {
            stamp_ms: NOW_MS - 10_000,
            id: "row-b".to_string(),
        };
        reopened.apply_scan(
            USAGE_SOURCE_ZCODE,
            SourceScan {
                watermark_ms: NOW_MS - 10_000,
                fingerprint: None,
                rejected: 0,
                usage_not_reported: 0,
                events: Vec::new(),
                files: Vec::new(),
                batch_end: Some(second.clone()),
            },
        );
        assert_eq!(
            reopened.cursor(USAGE_SOURCE_ZCODE).unwrap().batch_end,
            Some(second)
        );
    }

    // ---------- daily activity projection ----------

    /// The local midnight of the fixed test clock's day, computed the same
    /// way production buckets days (machine-local `chrono::Local`), so the
    /// straddle test holds on any machine timezone.
    fn local_midnight_ms(of_ms: i64) -> i64 {
        let naive = local_day_key(of_ms).and_hms_opt(0, 0, 0).unwrap();
        match chrono::Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(dt) => dt.timestamp_millis(),
            chrono::LocalResult::Ambiguous(dt, _) => dt.timestamp_millis(),
            chrono::LocalResult::None => {
                panic!("the test machine's timezone has no local midnight")
            }
        }
    }

    fn daily_date_key_of_offset(ms: i64) -> String {
        local_day_key(ms).format("%Y-%m-%d").to_string()
    }

    // 14. daily bucketing: events either side of local midnight land on
    //     their own local calendar days, the Today range is exactly one
    //     cell, and the 7d cells reconcile with the range totals.
    #[test]
    fn daily_buckets_events_by_local_midnight_and_reconciles_with_totals() {
        let midnight = local_midnight_ms(NOW_MS);
        let (store, _dir) = temp_store("daily-midnight");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 3 * 24 * 60 * 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:d1", "zai", "GLM-5.3", midnight - 1),
                    event("zcode:d2", "zai", "GLM-5.3", midnight + 1),
                ],
                NOW_MS - 1_000,
            ),
        );

        let today = store.aggregate(UsageRange::Today, Some(midnight));
        assert_eq!(today.events_in_range, 1, "only the after-midnight event");
        assert_eq!(today.daily.len(), 1, "Today is exactly one local day");
        assert!(today.daily[0].observed);
        assert_eq!(today.daily[0].events, 1);
        assert_eq!(today.daily[0].total_tokens, Some(380));
        assert!(!today.daily[0].before_collection);

        let seven = store.aggregate(UsageRange::SevenDays, None);
        assert_eq!(seven.daily.len(), 8, "a rolling 7d touches 8 local days");
        let day_of = |date: &str| {
            seven
                .daily
                .iter()
                .find(|day| day.date == date)
                .expect("each event day is a cell")
        };
        let before = day_of(&daily_date_key_of_offset(midnight - 1));
        let after = day_of(&daily_date_key_of_offset(midnight + 1));
        assert_eq!(before.events, 1);
        assert_eq!(after.events, 1);
        let daily_events: usize = seven.daily.iter().map(|day| day.events).sum();
        let daily_tokens: u64 = seven
            .daily
            .iter()
            .filter_map(|day| day.total_tokens)
            .sum();
        assert_eq!(daily_events, seven.events_in_range, "events reconcile");
        assert_eq!(
            daily_tokens,
            seven.totals.total_tokens,
            "reported tokens reconcile with the range totals"
        );
    }

    // 15. missing days: a day without recorded events is not observed and
    //     serializes no token total (missing is never a wire zero), days
    //     wholly before the local collection start carry the
    //     before-collection flag, and a failed source fabricates nothing.
    #[test]
    fn daily_missing_days_are_not_observed_and_flag_before_collection() {
        let (now, clock) = manual_clock(NOW_MS - 3 * 24 * 60 * 60_000);
        let (store, _dir) = temp_store_with_clock("daily-missing", clock);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 3 * 24 * 60 * 60_000, None);
        // A second source failed today — its failure must not invent data.
        store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::ReadFailure, None);
        *now.lock().unwrap() = NOW_MS;
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![event("zcode:a", "zai", "GLM-5.3", NOW_MS - 1_000)],
                NOW_MS - 1_000,
            ),
        );

        let seven = store.aggregate(UsageRange::SevenDays, None);
        let collection_day = daily_date_key_of_offset(NOW_MS - 3 * 24 * 60 * 60_000);
        let observed_days: Vec<&UsageDailyDayDto> =
            seven.daily.iter().filter(|day| day.observed).collect();
        assert_eq!(
            observed_days.len(),
            1,
            "exactly one day carries the recorded event"
        );
        assert_eq!(observed_days[0].total_tokens, Some(380));
        assert!(!observed_days[0].before_collection);

        let wire = serde_json::to_value(&seven).unwrap();
        for (index, day) in seven.daily.iter().enumerate() {
            let cell = &wire["daily"][index];
            assert_eq!(cell["observed"], day.observed);
            if day.observed {
                assert!(cell["totalTokens"].is_number(), "observed day serializes tokens");
            } else {
                assert!(
                    cell.get("totalTokens").is_none(),
                    "a day without events never serializes a zero"
                );
            }
            let before_flag = day.date < collection_day;
            assert_eq!(
                day.before_collection, before_flag,
                "only days wholly before the local collection start are flagged"
            );
            if day.before_collection {
                assert!(!day.observed, "pre-collection days have no data");
            }
        }
        assert!(
            seven.daily.iter().any(|day| !day.observed && !day.before_collection),
            "an in-window day with no events stays explicitly not observed"
        );
        assert!(seven.incomplete_history, "7d reaches before collection");
    }

    // 16. dedup interplay: re-ingesting an overlap never inflates a day.
    #[test]
    fn daily_is_idempotent_under_overlapping_scans() {
        let midnight = local_midnight_ms(NOW_MS);
        let (store, _dir) = temp_store("daily-dedup");
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 120_000, None);
        let events = vec![
            event("zcode:a", "zai", "GLM-5.3", midnight - 1),
            event("zcode:b", "zai", "GLM-5.3-Flash", midnight + 1),
        ];
        store.apply_scan(USAGE_SOURCE_ZCODE, scan_from(events.clone(), midnight + 1));
        store.apply_scan(USAGE_SOURCE_ZCODE, scan_from(events, midnight + 1));

        let today = store.aggregate(UsageRange::Today, Some(midnight));
        assert_eq!(today.events_in_range, 1);
        assert_eq!(today.daily[0].events, 1, "the duplicate is not re-bucketed");
        assert_eq!(today.totals.total_tokens, 380);
        assert_eq!(today.daily[0].total_tokens, Some(380));
    }

    // 17. an implausible webview midnight degrades to the trailing day and
    //     the daily cells stay consistent with whatever range was used.
    #[test]
    fn daily_tracks_the_degraded_today_range() {
        let (store, _dir) = temp_store("daily-degraded");
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 3 * 24 * 60 * 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![
                    event("zcode:a", "zai", "GLM-5.3", NOW_MS - 30 * 60 * 60_000),
                    event("zcode:b", "zai", "GLM-5.3", NOW_MS - 1_000),
                ],
                NOW_MS - 1_000,
            ),
        );
        let dto = store.aggregate(UsageRange::Today, Some(NOW_MS - 10 * 24 * 60 * 60_000));
        assert!(
            (1..=2).contains(&dto.daily.len()),
            "a trailing 24h touches at most two local days"
        );
        let daily_events: usize = dto.daily.iter().map(|day| day.events).sum();
        assert_eq!(daily_events, dto.events_in_range, "cells reconcile");
    }

    // 18. empty store: the projection covers the range with unobserved
    //     cells and claims no collection window.
    #[test]
    fn daily_empty_store_has_only_unobserved_cells() {
        let (store, _dir) = temp_store("daily-empty");
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert!(!dto.daily.is_empty());
        for day in &dto.daily {
            assert!(!day.observed);
            assert_eq!(day.total_tokens, None);
            assert!(!day.before_collection, "no anchor, no window claim");
        }
        let wire = serde_json::to_value(&dto).unwrap();
        assert!(
            wire["daily"]
                .as_array()
                .unwrap()
                .iter()
                .all(|cell| cell.get("totalTokens").is_none()),
            "no cell serializes a fabricated zero"
        );
    }

    #[test]
    fn production_collection_pass_never_resolves_or_collects_codex() {
        let dir = tempdir::temp_dir("codex-collection-stopped");
        let probes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = probes.clone();
        let store = UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        )
        .with_codex_home_resolver(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            None
        }))
        .with_opencode_db_resolver(Box::new(|| None));
        store.set_enabled(true).unwrap();

        run_collection_pass(&store);

        assert_eq!(probes.load(Ordering::SeqCst), 0);
        assert!(store.cursor(USAGE_SOURCE_CODEX).is_none());
        assert!(!store.events().iter().any(|event| event.source == USAGE_SOURCE_CODEX));
    }

    #[test]
    fn legacy_codex_events_remain_persisted_but_are_absent_from_aggregates() {
        let (store, dir) = temp_store("legacy-codex-filter");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        store.set_enabled(true).unwrap();
        store.establish_baseline(USAGE_SOURCE_CODEX, NOW_MS - 120_000, None);
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 120_000, None);

        let mut codex = event("codex:legacy", "openai", "gpt-5", NOW_MS - 30_000);
        codex.source = USAGE_SOURCE_CODEX.to_string();
        codex.source_provider_id = "openai".to_string();
        let other = event("zcode:current", "zai", "GLM-5.3", NOW_MS - 20_000);
        store.apply_scan(USAGE_SOURCE_CODEX, scan_from(vec![codex.clone()], NOW_MS - 30_000));
        store.apply_scan(USAGE_SOURCE_ZCODE, scan_from(vec![other.clone()], NOW_MS - 20_000));
        drop(store);

        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(persisted["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["id"] == codex.id));

        let reopened = UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        let stored = reopened.stored_events();
        assert!(stored.iter().any(|event| event.id == codex.id));
        let visible = reopened.events();
        assert!(!visible.iter().any(|event| event.source == USAGE_SOURCE_CODEX));
        assert!(visible.iter().any(|event| event.id == other.id));

        let dto = reopened.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.events_in_range, 1);
        assert_eq!(dto.groups.len(), 1);
        assert_eq!(dto.groups[0].provider, "zai");
        assert_eq!(dto.totals.total_tokens, other.total_tokens);
        assert_eq!(
            dto.daily.iter().map(|day| day.events).sum::<usize>(),
            dto.events_in_range
        );
        assert!(!serde_json::to_string(&dto).unwrap().contains("codex:legacy"));
        assert!(dto
            .sources
            .iter()
            .all(|source| source.source != USAGE_SOURCE_CODEX));
        assert!(dto
            .source_coverage
            .iter()
            .all(|coverage| coverage.source != USAGE_SOURCE_CODEX));
    }
}
