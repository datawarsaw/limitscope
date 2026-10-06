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
//! - The sources are the ZCode local database (`~/.zcode/cli/db/db.sqlite`,
//!   table `model_usage`, read strictly read-only through a temporary
//!   snapshot copy — see `usage_source_zcode.rs`), the Codex rollout logs
//!   (`~/.codex/sessions/**` and `~/.codex/archived_sessions/**`, read
//!   strictly read-only with per-file byte cursors — see
//!   `usage_source_codex.rs`), and the OpenCode local database
//!   (`~/.local/share/opencode/opencode.db`, table `message`, read
//!   strictly read-only through a temporary snapshot copy — see
//!   `usage_source_opencode.rs`). Source files are never opened for
//!   writing, never checkpointed, never mutated. Each source keeps its
//!   own cursor under its own source id; one source failing never blocks
//!   or hides the others.
//! - Collection is strictly opt-in. While the persisted `enabled` flag is
//!   false the collector does not even resolve the source path — no stat,
//!   no existence probe, no open, no copy, no schema inspection
//!   (docs/local-data-controls-v0.7.md §14, rule 7).
//! - Backlog is never imported. On first enable (and after a clear, while
//!   still enabled) the collector establishes a baseline cursor at the
//!   source's current high-water mark and imports nothing; only rows
//!   completed after that point are ever ingested. The baseline writes no
//!   fake zero usage.
//! - Re-ingesting an overlap is idempotent: events dedup by their stable
//!   owned identity, so re-running a collector can never grow totals.
//! - Retention is bounded (35 days, plus a hard event cap) and only ever
//!   deletes LimitScope-owned events in this store's own file.
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
//! A corrupt or foreign-version file degrades to the empty defaults —
//! including `enabled: false`, the fail-closed direction for an opt-in
//! flag — and the sanitized state is written back.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

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
    /// The incremental window held more rows than the scan ceiling; the
    /// scan was abandoned whole and the cursor did not move.
    ScanCeiling,
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

#[derive(Default)]
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
    path: PathBuf,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    zcode_db: ZcodeDbResolver,
    codex_home: CodexHomeResolver,
    opencode_db: OpencodeDbResolver,
    /// Transient in-flight latch for the "collecting" diagnostic.
    pub(crate) collecting: std::sync::atomic::AtomicBool,
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
            path,
            now_ms,
            zcode_db,
            codex_home: production_codex_home_resolver(),
            opencode_db: production_opencode_db_resolver(),
            collecting: std::sync::atomic::AtomicBool::new(false),
        };
        store.load_from_disk();
        store
    }

    /// Overrides the Codex home resolver (tests, fixtures). Chainable.
    pub fn with_codex_home_resolver(mut self, resolver: CodexHomeResolver) -> Self {
        self.codex_home = resolver;
        self
    }

    /// Overrides the OpenCode database resolver (tests, fixtures).
    /// Chainable.
    pub fn with_opencode_db_resolver(mut self, resolver: OpencodeDbResolver) -> Self {
        self.opencode_db = resolver;
        self
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
    pub fn set_enabled(&self, enabled: bool) {
        let changed = {
            let mut state = self.state.lock().unwrap();
            if state.enabled == enabled {
                false
            } else {
                state.enabled = enabled;
                state.revision += 1;
                true
            }
        };
        if changed {
            self.persist_current();
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
        self.state.lock().unwrap().cursors.get(source).cloned()
    }

    /// The owned events, oldest first (canonical store order). Read-only
    /// accessor for diagnostics and tests.
    pub fn events(&self) -> Vec<UsageEvent> {
        self.state.lock().unwrap().events.clone()
    }

    /// The disclosure anchor: when local collection started (the first
    /// baseline), or `None` before any baseline was ever established.
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
        let Ok(raw) = fs::read_to_string(&self.path) else {
            return;
        };
        let Ok(parsed) = serde_json::from_str::<UsageIntelligenceEnvelope>(&raw) else {
            self.persist_current();
            return;
        };
        if parsed.version != BLOB_VERSION {
            self.persist_current();
            return;
        }
        let now_ms = self.now_ms();
        let mut cursors = HashMap::new();
        for cursor in parsed.cursors {
            if !cursor.source.trim().is_empty() {
                cursors.insert(cursor.source.clone(), cursor);
            }
        }
        let mut events = Vec::with_capacity(parsed.events.len());
        for event in parsed.events {
            if let Some(valid) = UsageEvent::validated(event, now_ms) {
                events.push(valid);
            }
        }
        let total_events = events.len();
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
            self.persist_current();
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
    pub fn establish_baseline_with_files(
        &self,
        source: &str,
        watermark_ms: i64,
        fingerprint: Option<SourceFingerprint>,
        files: Vec<SourceFileCursor>,
    ) {
        let now_ms = self.now_ms();
        let changed = {
            let mut state = self.state.lock().unwrap();
            let already_baselined = state
                .cursors
                .get(source)
                .map(|cursor| cursor.baselined)
                .unwrap_or(false);
            if already_baselined {
                false
            } else {
                state.cursors.insert(
                    source.to_string(),
                    SourceCursor {
                        source: source.to_string(),
                        watermark_ms,
                        baseline_watermark_ms: watermark_ms,
                        baselined: true,
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
            self.persist_current();
        }
    }

    /// Marks a non-ok source state (absent / schema / read failure /
    /// ceiling), preserving the watermark and baseline anchors.
    pub fn mark_source_state(&self, source: &str, state: SourceState, detail: Option<String>) {
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

    // ---- scan application ----

    /// Applies one successful scan: validates and dedups the events,
    /// applies retention, advances the cursor watermark and diagnostics,
    /// and persists. Returns the applied stats (accepted counts only
    /// newly stored events; duplicates were already present — in the store
    /// or earlier in the same batch).
    pub fn apply_scan(&self, source: &str, mut scan: SourceScan) -> ScanStats {
        let now_ms = self.now_ms();
        let cutoff = now_ms - USAGE_EVENT_RETENTION_MS;
        let mut batch: Vec<UsageEvent> = Vec::new();
        let mut rejected = scan.rejected;
        for event in scan.events {
            match UsageEvent::validated(event, now_ms) {
                Some(valid_event) => {
                    // Dedup inside the batch too, so `accepted` counts
                    // real insertions even when a scan overlaps itself.
                    if !batch
                        .iter()
                        .any(|existing: &UsageEvent| existing.id == valid_event.id)
                    {
                        batch.push(valid_event);
                    }
                }
                None => rejected += 1,
            }
        }

        let stats = {
            let mut state = self.state.lock().unwrap();
            let existing: std::collections::HashSet<&str> =
                state.events.iter().map(|e| e.id.as_str()).collect();
            let mut accepted = 0;
            let mut duplicates = 0;
            let mut merged = state.events.clone();
            for event in batch {
                if existing.contains(event.id.as_str()) {
                    duplicates += 1;
                    continue;
                }
                if merged.len() >= USAGE_EVENT_MAX {
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
                if cursor.baselined {
                    cursor.watermark_ms = cursor.watermark_ms.max(scan.watermark_ms);
                    // File-based sources report their full tracked-file
                    // state; replacing wholesale also reflects deletions.
                    cursor.files = std::mem::take(&mut scan.files);
                }
                cursor.last_observed_at = now_ms;
                cursor.state = SourceState::Ok;
                cursor.detail = None;
                cursor.fingerprint = scan.fingerprint;
                cursor.last_scan = Some(stats);
            }
            state.revision += 1;
            stats
        };
        self.persist_current();
        stats
    }

    // ---- owned clear ----

    /// Clears everything this plane owns: events, cursors (and their
    /// watermarks), and the disclosure anchor. The opt-in flag is
    /// deliberately preserved — clearing data is not disabling the
    /// feature; while still enabled, the next collection re-baselines at
    /// the source's current high-water mark and does not backfill.
    pub fn clear_owned(&self) -> bool {
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
            if !self.persist_current() {
                let _ = fs::remove_file(&self.path);
            }
        }
        had_data
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

        let (events, enabled, collection_started_at, cursors, collecting) = {
            let state = self.state.lock().unwrap();
            (
                state.events.clone(),
                state.enabled,
                state.collection_started_at,
                state.cursors.values().cloned().collect::<Vec<_>>(),
                self.is_collecting(),
            )
        };

        let in_range: Vec<&UsageEvent> = events
            .iter()
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

        UsageIntelligenceDto {
            schema_version: USAGE_INTELLIGENCE_SCHEMA_VERSION,
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
            // Honest range semantics: the selected range reaches further
            // back than local collection itself does.
            incomplete_history: collection_started_at
                .map(|started| started > range_start)
                .unwrap_or(false),
            sources,
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
        let temp_path = self.path.with_extension("json.tmp");
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&temp_path, json)
            .and_then(|()| fs::rename(&temp_path, &self.path))
            .is_ok()
    }
}

/// Dedups by owned id (first occurrence wins) and sorts by (event_at, id)
/// so the persisted shape is canonical.
fn dedup_and_sort(mut events: Vec<UsageEvent>) -> Vec<UsageEvent> {
    let mut seen = std::collections::HashSet::with_capacity(events.len());
    events.retain(|event| seen.insert(event.id.clone()));
    events.sort_by(|a, b| a.event_at.cmp(&b.event_at).then_with(|| a.id.cmp(&b.id)));
    events
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
            SourceState::Ok => "ok".to_string(),
        }
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
    /// schemaUnsupported | readFailure | scanCeiling | ok.
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageIntelligenceDto {
    pub schema_version: u32,
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
    /// Honesty pin: these are reported tokens collected locally, never
    /// account quota, billing usage, or exact spend.
    pub semantics: &'static str,
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

/// Runs one collection pass over every source under the single shared
/// opt-in: ZCode first, then Codex, then OpenCode. The sources are
/// isolated — each updates only its own cursor and diagnostics, so one
/// source failing never blocks or corrupts the others, and no failure can
/// reach quota-provider health. Blocking filesystem work — call from
/// `spawn_blocking` off the async runtime.
pub fn run_collection_pass(store: &UsageIntelligenceStore) {
    let _ = collect_zcode(store);
    let _ = crate::usage_source_codex::collect(store);
    let _ = crate::usage_source_opencode::collect(store);
}

/// Spawns one detached collection pass (used by the runtime cycle and the
/// enable command). Failures update the store's diagnostics; nothing is
/// propagated to quota-provider health.
pub fn spawn_collection(store: std::sync::Arc<UsageIntelligenceStore>) {
    if !store.enabled() {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        store
            .collecting
            .store(true, std::sync::atomic::Ordering::Relaxed);
        run_collection_pass(&store);
        store
            .collecting
            .store(false, std::sync::atomic::Ordering::Relaxed);
    });
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
pub fn set_usage_intelligence_enabled(handle: tauri::State<RuntimeHandle>, enabled: bool) {
    handle.set_usage_intelligence_enabled(enabled);
}

/// Clears everything Usage Intelligence owns (events, cursors, disclosure
/// anchor) and leaves the source untouched. While the source remains
/// enabled, the next collection re-baselines at the current high-water
/// mark — old usage does not reappear.
#[tauri::command]
pub fn clear_usage_intelligence(handle: tauri::State<RuntimeHandle>) -> LocalDataClearOutcome {
    let removed = handle
        .usage_intelligence_store()
        .map(|store| store.clear_owned())
        .unwrap_or(false);
    LocalDataClearOutcome {
        category: USAGE_INTELLIGENCE_CATEGORY,
        removed,
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW_MS: i64 = 1_791_288_000_000; // 2026-10-06T12:00:00.000Z

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
        }
    }

    // 1. serialization roundtrip: a persisted store reopens with the same
    //    events, cursors, enabled flag, and disclosure anchor.
    #[test]
    fn persisted_state_roundtrips_through_disk() {
        let (store, dir) = temp_store("roundtrip");
        let path = dir.path().join(USAGE_INTELLIGENCE_FILE_NAME);
        store.set_enabled(true);
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
        store.set_enabled(true);
        store.establish_baseline(USAGE_SOURCE_ZCODE, NOW_MS - 60_000, None);
        store.apply_scan(
            USAGE_SOURCE_ZCODE,
            scan_from(
                vec![event("zcode:a", "zai", "GLM-5.3", NOW_MS - 30_000)],
                NOW_MS - 30_000,
            ),
        );
        assert!(store.clear_owned());
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
        assert!(!store.clear_owned());
        assert!(!store.clear_owned());
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
        store.set_enabled(true);
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
        store.set_enabled(true);
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
        store.set_enabled(true);
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
        store.set_enabled(true);
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
        store.set_enabled(true);
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
            USAGE_SOURCE_CODEX,
            NOW_MS - 60_000,
            None,
            files.clone(),
        );
        drop(store);

        let reopened = UsageIntelligenceStore::open_with(path, fixed_clock(), Box::new(|| None));
        let cursor = reopened.cursor(USAGE_SOURCE_CODEX).expect("cursor survives");
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
            USAGE_SOURCE_CODEX,
            SourceScan {
                watermark_ms: NOW_MS - 30_000,
                fingerprint: None,
                rejected: 0,
                usage_not_reported: 0,
                events: Vec::new(),
                files: updated,
            },
        );
        assert_eq!(stats.accepted, 0);
        let cursor = reopened.cursor(USAGE_SOURCE_CODEX).unwrap();
        assert_eq!(cursor.files.len(), 1);
        assert_eq!(cursor.files[0].offset, 200);
        assert_eq!(reopened.cursor(USAGE_SOURCE_ZCODE), None);
    }
}
