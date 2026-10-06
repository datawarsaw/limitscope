//! OpenCode local usage source — the third Usage Intelligence collector.
//!
//! Reads OpenCode's local database (`~/.local/share/opencode/opencode.db`,
//! table `message`; schema verified against the live corpus on 2026-10-06:
//! 3,034 message rows / 2,890 assistant rows) and normalizes completed
//! assistant requests into the source-neutral [`UsageEvent`] model.
//!
//! # Accounting layer (the hard boundary)
//!
//! The canonical accounting layer is the per-assistant-row
//! `message.data.tokens` JSON. OpenCode derives several duplicate/derived
//! accounting layers from it — step-finish parts (table `part`), the
//! session aggregate, and the event replication log — and summing any of
//! them on top would multiply counts. This reader touches **only** the
//! `message` table; fixtures include deliberately inflated `part` /
//! `session` rows to prove the other layers are never summed. Sessions
//! are never used for attribution either (they can be multi-model);
//! attribution comes from the per-message `providerID` / `modelID`.
//!
//! # Read-only access (the hard boundary)
//!
//! The live database and its `-wal` / `-shm` sidecars are never opened by
//! this module. Every collection pass copies the database plus its `-wal`
//! sidecar into a fresh temporary directory and queries only the copy —
//! the same snapshot pattern the ZCode reader uses. The source files are
//! only ever touched by the copy calls themselves (read handles), never
//! locked, never written, never checkpointed, never migrated; the `-shm`
//! is a transient index the copy rebuilds for itself. A WAL copied
//! mid-append replays only the committed, checksummed prefix, so the
//! snapshot is a coherent committed state; a torn copy fails closed at
//! open or query time.
//!
//! # Opt-in and probing
//!
//! Collection is gated by the store's persisted `enabled` flag. While
//! disabled, [`collect`] returns before the database path is even
//! resolved — no stat, no existence test, no open, no copy, no schema
//! inspection (docs/local-data-controls-v0.7.md §14, rule 7).
//!
//! # Incremental contract
//!
//! - First successful observation establishes a baseline at the current
//!   high-water mark (`MAX(time_updated)` over assistant rows, plus one
//!   millisecond) and imports nothing — no historical backlog, no fake
//!   zero usage.
//! - The cursor column is `time_updated` — the row's last-write stamp.
//!   That is the commit-tracking column: a completion, a late usage
//!   write, or a fork copy each bump it, so the window
//!   `time_updated >= watermark - LOOKBACK` (inclusive at the floor,
//!   ordered by `(time_updated, id)`, capped at `SCAN_CEILING + 1` rows)
//!   catches every newly-committed accounting state regardless of how old
//!   the request itself is. `time_created` (the request-start stamp,
//!   preserved verbatim by forks) is deliberately not the cursor: a long
//!   request committing far behind younger rows would otherwise fall
//!   through the floor. Message ids are not monotonic (one id-ordered
//!   timestamp inversion per ~2,900 live rows), so the composite
//!   `(time_updated, id)` ordering is used; ids never gate the window.
//! - Hitting the ceiling fails closed: the whole scan is abandoned, the
//!   cursor does not move, and the source state reports `scanCeiling`.
//!   Nothing is silently skipped.
//! - The watermark advances only through **dispositioned** rows: accepted
//!   events, and all-zero rows dispositioned as usage-not-reported (a
//!   deterministic, content-bound verdict — re-reading them can never
//!   change it). Rows rejected as malformed never advance it; the
//!   lookback keeps them revisitable. Boundary honesty: accounting
//!   re-written more than one lookback behind a watermark that has already
//!   moved past it is not re-read — the earlier disposition stands (the
//!   same bounded boundary the ZCode reader documents for late commits).
//!
//! # Completion predicate (verified against the live corpus 2026-10-06)
//!
//! `finish` alone is not a completion signal (the dominant live value is
//! literally `"unknown"`, on healthy rows). The verified predicate is:
//! `time.completed` present AND `error` absent AND `finish` present. Live
//! census: 2,846 rows completed without error (every one carrying a
//! non-null finish), 33 error rows (all reporting all-zero tokens), 11
//! in-flight rows without `time.completed` (also all-zero). Error and
//! in-flight rows are filtered out in SQL — their JSON is never fetched
//! or parsed. Anything not matching the predicate fails closed.
//!
//! # Fork deduplication (verified against the live corpus 2026-10-06)
//!
//! Forking an OpenCode session duplicates already-executed request rows
//! under new message ids (and a new session id) while preserving
//! `time_created`, `time.completed`, and the whole accounting payload —
//! re-verified live: 10 duplicate fingerprint groups, all cross-session,
//! all pairs, ~2% of token volume. `message.id` therefore cannot be the
//! event identity. The owned identity is the **content address** of the
//! discovery-verified fingerprint
//! `(time_created, providerID, modelID, input, output, reasoning,
//! cache_read, cache_write, cost)`, SHA-256 hashed: both copies of one
//! executed request map to the same owned id, so the store's id dedup
//! collapses them across rescans and restarts, and repeated scans choose
//! the same event. Residual collision risk (documented, not zero): two
//! genuinely distinct requests sharing the full fingerprint — same
//! millisecond timestamp, model, dimensions, and cost — collapse into one
//! event, indistinguishable from forks by content; and a SHA-256
//! collision, negligible at ~2⁻⁶⁵ for realistic store sizes and
//! indistinguishable in effect. The fingerprint deliberately excludes
//! mutable-ish bookkeeping (`finish`, `time_updated`): if the source ever
//! amends a row in place, an amended field outside the fingerprint cannot
//! split one executed request into two events.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::usage_intelligence::{
    SourceFingerprint, SourceScan, SourceState, UsageEvent, UsageIntelligenceStore,
    EVENT_EPOCH_FLOOR_MS, EVENT_FUTURE_TOLERANCE_MS, USAGE_SOURCE_OPENCODE,
};

/// Columns this reader requires on the `message` table. The verified live
/// schema carries `session_id` too (plus the JSON payload's own fields);
/// additive columns are compatible drift, a missing required column is
/// not. `session_id` is deliberately not required: it is never read.
const REQUIRED_COLUMNS: &[&str] = &["id", "time_created", "time_updated", "data"];

/// The one table this reader understands (the canonical accounting layer).
const USAGE_TABLE: &str = "message";

/// Maximum rows one incremental window may hold. Fetching one more than
/// the ceiling detects the overflow; an over-ceiling window abandons the
/// scan whole (fail closed) rather than chunking silently.
pub const SCAN_CEILING: usize = 20_000;

/// How far behind the watermark the window still reads, absorbing rows
/// committed slightly after a watermark that younger rows already moved.
const LOOKBACK_MS: i64 = 10 * 60_000;

/// Outcome of one collection pass, for diagnostics and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpencodeScanOutcome {
    /// The store is disabled; nothing was probed.
    Disabled,
    /// The baseline was established at the given high-water mark;
    /// nothing was imported.
    Baseline { watermark_ms: i64 },
    /// An incremental scan ran; rows were dispositioned.
    Collected {
        accepted: usize,
        duplicates: usize,
        rejected: usize,
        /// Completed rows whose every dimension is zero — the verified
        /// "usage not reported" pattern, never stored as measured zero.
        usage_not_reported: usize,
        watermark_ms: i64,
    },
    /// The source fingerprint is unchanged since the last successful
    /// scan; the snapshot copy was skipped.
    Unchanged,
    /// The database does not exist at the expected path.
    Absent,
    /// The database exists but the schema is not understood.
    SchemaUnsupported,
    /// The incremental window exceeded the scan ceiling; nothing was
    /// ingested and the cursor did not move.
    CeilingExceeded { rows: usize },
    /// The read failed; the detail is display-safe.
    ReadFailure(String),
}

/// Runs one collection pass. The disabled gate is first: a disabled store
/// returns without resolving the source path.
pub fn collect(store: &UsageIntelligenceStore) -> OpencodeScanOutcome {
    if !store.enabled() {
        return OpencodeScanOutcome::Disabled;
    }
    let Some(db_path) = (store.opencode_db_resolver())() else {
        store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::SourceAbsent, None);
        return OpencodeScanOutcome::Absent;
    };
    // Existence check (the first and only probe, post opt-in): a missing
    // database is a normal state — OpenCode not installed or not yet run.
    let db_meta = match fs::metadata(&db_path) {
        Ok(meta) => meta,
        Err(_) => {
            store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::SourceAbsent, None);
            return OpencodeScanOutcome::Absent;
        }
    };
    let wal_path = sibling(&db_path, "-wal");
    let wal_meta = fs::metadata(&wal_path).ok();

    // Cheap change detection: an unchanged, previously-healthy source
    // costs no snapshot copy. Only a fingerprint from a successful scan
    // is trusted, so a failed pass always retries.
    let fingerprint = fingerprint_of(&db_meta, wal_meta.as_ref());
    if let Some(cursor) = store.cursor(USAGE_SOURCE_OPENCODE) {
        if cursor.baselined
            && cursor.state == SourceState::Ok
            && cursor.fingerprint.as_ref() == Some(&fingerprint)
        {
            return OpencodeScanOutcome::Unchanged;
        }
    }

    let fingerprint = current_fingerprint(&db_path, &wal_path);

    collect_from_live(&db_path, &wal_path, store, fingerprint)
}

/// Snapshot-copies the database (plus WAL) and runs the baseline or
/// incremental read against the copy.
fn collect_from_live(
    db_path: &Path,
    wal_path: &Path,
    store: &UsageIntelligenceStore,
    fingerprint: Option<SourceFingerprint>,
) -> OpencodeScanOutcome {
    let guard = match snapshot_database(db_path, wal_path) {
        Ok(guard) => guard,
        Err(detail) => {
            store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::ReadFailure, Some(detail));
            return OpencodeScanOutcome::ReadFailure(read_failure_detail());
        }
    };
    let snapshot_db = guard.path.join("opencode.db");
    let connection = match rusqlite::Connection::open(&snapshot_db) {
        Ok(connection) => connection,
        Err(error) => {
            let detail = format!("could not open the snapshot: {error}");
            store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::ReadFailure, Some(detail));
            return OpencodeScanOutcome::ReadFailure(read_failure_detail());
        }
    };
    if let Err(detail) = verify_schema(&connection) {
        store.mark_source_state(
            USAGE_SOURCE_OPENCODE,
            SourceState::SchemaUnsupported,
            Some(detail),
        );
        return OpencodeScanOutcome::SchemaUnsupported;
    }

    // Baseline first: no cursor (or a failure-only record) means this is
    // the first successful observation — establish the backlog skip. The
    // watermark sits one millisecond ABOVE the current high-water mark
    // (the newest assistant row's last-write stamp), so every row already
    // in the database — including any sharing the max stamp — is strictly
    // backlog and never imported; the +1 keeps the incremental window's
    // inclusive floor from re-admitting them on the next scan.
    let baselined = store
        .cursor(USAGE_SOURCE_OPENCODE)
        .map(|cursor| cursor.baselined)
        .unwrap_or(false);
    if !baselined {
        // An empty source (Ok(None)) is a valid zero-history baseline; a
        // failed query is not. A MAX error must never read as empty and
        // anchor the baseline at zero — that would admit the whole backlog
        // as history on the next pass. Fail closed: record ReadFailure,
        // leave the cursor unbaselined, and let a later pass retry.
        let max_updated: Option<i64> = match connection.query_row(
            &format!(
                "SELECT MAX(time_updated) FROM {USAGE_TABLE} \
                 WHERE json_extract(data, '$.role') = 'assistant'"
            ),
            [],
            |row| row.get(0),
        ) {
            Ok(max) => max,
            Err(error) => {
                let detail = format!("baseline high-water query failed: {error}");
                store.mark_source_state(
                    USAGE_SOURCE_OPENCODE,
                    SourceState::ReadFailure,
                    Some(detail),
                );
                return OpencodeScanOutcome::ReadFailure(read_failure_detail());
            }
        };
        let watermark_ms = max_updated.map(|max| max + 1).unwrap_or(0);
        store.establish_baseline(USAGE_SOURCE_OPENCODE, watermark_ms, fingerprint);
        return OpencodeScanOutcome::Baseline { watermark_ms };
    }

    // Incremental window over the commit-tracking column. The floor
    // re-reads a bounded lookback behind the watermark but never reaches
    // below the immutable baseline watermark — rows written before the
    // baseline are pre-enable backlog and stay unimported. The WHERE
    // clause applies the verified completion predicate in SQL, so user
    // rows, in-flight rows, and error rows (error payload included) are
    // never fetched or parsed.
    let cursor = store.cursor(USAGE_SOURCE_OPENCODE);
    let watermark_ms = cursor.as_ref().map(|cursor| cursor.watermark_ms).unwrap_or(0);
    let window_start = cursor
        .as_ref()
        .map(|cursor| (watermark_ms.saturating_sub(LOOKBACK_MS)).max(cursor.baseline_watermark_ms))
        .unwrap_or(0);
    let mut statement = match connection.prepare(&format!(
        "SELECT id, time_created, time_updated, data FROM {USAGE_TABLE} \
         WHERE time_updated >= ?1 \
           AND json_extract(data, '$.role') = 'assistant' \
           AND json_extract(data, '$.time.completed') IS NOT NULL \
           AND json_extract(data, '$.error') IS NULL \
           AND json_extract(data, '$.finish') IS NOT NULL \
         ORDER BY time_updated ASC, id ASC \
         LIMIT ?2"
    )) {
        Ok(statement) => statement,
        Err(error) => {
            let detail = format!("could not prepare the scan: {error}");
            store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::ReadFailure, Some(detail));
            return OpencodeScanOutcome::ReadFailure(read_failure_detail());
        }
    };
    let rows = statement.query_map(
        rusqlite::params![window_start, (SCAN_CEILING + 1) as i64],
        |row| {
            Ok(RawMessageRow {
                id: row.get(0)?,
                time_created: row.get(1)?,
                time_updated: row.get(2)?,
                data: row.get(3)?,
            })
        },
    );
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            let detail = format!("could not query the snapshot: {error}");
            store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::ReadFailure, Some(detail));
            return OpencodeScanOutcome::ReadFailure(read_failure_detail());
        }
    };

    let mut raw_rows = Vec::new();
    let mut query_error: Option<String> = None;
    for row in rows {
        match row {
            Ok(raw) => raw_rows.push(raw),
            Err(error) => {
                query_error = Some(format!("{error}"));
                break;
            }
        }
    }
    if let Some(error) = query_error {
        let detail = format!("could not read a row: {error}");
        store.mark_source_state(USAGE_SOURCE_OPENCODE, SourceState::ReadFailure, Some(detail));
        return OpencodeScanOutcome::ReadFailure(read_failure_detail());
    }

    // Fail closed on the scan ceiling: an over-ceiling window is
    // abandoned whole — no partial ingest, no cursor movement.
    if raw_rows.len() > SCAN_CEILING {
        store.mark_source_state(
            USAGE_SOURCE_OPENCODE,
            SourceState::ScanCeiling,
            Some(format!(
                "incremental window held {} rows above the {} row ceiling",
                raw_rows.len(),
                SCAN_CEILING
            )),
        );
        return OpencodeScanOutcome::CeilingExceeded {
            rows: raw_rows.len(),
        };
    }

    let now_ms = store.now_ms();
    let mut events = Vec::with_capacity(raw_rows.len());
    let mut rejected = 0usize;
    let mut usage_not_reported = 0usize;
    let mut new_watermark = watermark_ms;
    for raw in raw_rows {
        // The watermark tracks the source's own commit stamps (the
        // cursor column), not the events. Event rows and
        // usage-not-reported rows are terminal, deterministic
        // dispositions: their commit stamps may gate later reads.
        // Malformed rows never advance the watermark — their data was
        // not understood, so the lookback keeps them revisitable until
        // they age out.
        let commit_stamp = raw.time_updated;
        match raw.disposition(now_ms) {
            RowDisposition::Event(event) => {
                new_watermark = new_watermark.max(commit_stamp);
                events.push(event);
            }
            RowDisposition::UsageNotReported => {
                usage_not_reported += 1;
                new_watermark = new_watermark.max(commit_stamp);
            }
            RowDisposition::Rejected => rejected += 1,
        }
    }

    let stats = store.apply_scan(
        USAGE_SOURCE_OPENCODE,
        SourceScan {
            watermark_ms: new_watermark,
            fingerprint,
            rejected,
            usage_not_reported,
            events,
            files: Vec::new(),
        },
    );
    OpencodeScanOutcome::Collected {
        accepted: stats.accepted,
        duplicates: stats.duplicates,
        rejected: stats.rejected,
        usage_not_reported: stats.usage_not_reported,
        watermark_ms: new_watermark,
    }
}

/// One raw row of the closed SELECT list.
struct RawMessageRow {
    id: String,
    time_created: i64,
    time_updated: i64,
    data: String,
}

/// What one raw row became: a stored event, a terminal
/// usage-not-reported disposition, or a rejection.
enum RowDisposition {
    Event(UsageEvent),
    UsageNotReported,
    Rejected,
}

/// The assistant message's accounting slice. Serde ignores every other
/// field of the payload (`agent`, `mode`, `parentID`, `path`, `variant`)
/// without materializing it; the `error` object is filtered out in SQL
/// and has no field here at all — prompt/response/tool content is never
/// deserialized into a structure, never retained, never logged.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssistantMessage {
    role: Option<String>,
    #[serde(rename = "providerID")]
    provider_id: Option<String>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
    #[serde(default)]
    tokens: Option<MessageTokens>,
    #[serde(default)]
    cost: Option<f64>,
    #[serde(default)]
    finish: Option<String>,
    #[serde(default)]
    time: Option<MessageTime>,
}

#[derive(Debug, Deserialize)]
struct MessageTime {
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    completed: Option<i64>,
}

/// The token dimensions are a disjoint partition: `input` is already
/// non-cached input, `reasoning` is disjoint from `output`, and the
/// optional `total` equals the sum of all five (verified 428/428 live).
#[derive(Debug, Deserialize)]
struct MessageTokens {
    #[serde(default)]
    input: Option<i64>,
    #[serde(default)]
    output: Option<i64>,
    #[serde(default)]
    reasoning: Option<i64>,
    #[serde(default)]
    cache: Option<CacheReads>,
    #[serde(default)]
    total: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CacheReads {
    #[serde(default)]
    read: Option<i64>,
    #[serde(default)]
    write: Option<i64>,
}

impl RawMessageRow {
    /// Validates and normalizes one completed assistant row into an
    /// event, a usage-not-reported disposition, or a rejection. Every
    /// rule here is fail-closed: a value this reader cannot verify is
    /// never guessed.
    fn disposition(self, now_ms: i64) -> RowDisposition {
        let id = self.id.trim();
        if id.is_empty() {
            return RowDisposition::Rejected;
        }
        let Ok(message) = serde_json::from_str::<AssistantMessage>(&self.data) else {
            return RowDisposition::Rejected;
        };
        if message.role.as_deref() != Some("assistant") {
            return RowDisposition::Rejected;
        }
        let Some(time) = message.time else {
            return RowDisposition::Rejected;
        };
        // The JSON creation stamp must agree with the table column; a
        // disagreement is drift, not data to reinterpret.
        if time.created != Some(self.time_created) {
            return RowDisposition::Rejected;
        }
        // SQL already guaranteed a non-null completion; re-verify here
        // and plausibility-check both stamps against the owned clock.
        let Some(completed_at) = time.completed else {
            return RowDisposition::Rejected;
        };
        if completed_at < EVENT_EPOCH_FLOOR_MS
            || completed_at - now_ms > EVENT_FUTURE_TOLERANCE_MS
        {
            return RowDisposition::Rejected;
        }
        if self.time_updated < self.time_created
            || self.time_updated < EVENT_EPOCH_FLOOR_MS
            || self.time_updated - now_ms > EVENT_FUTURE_TOLERANCE_MS
        {
            return RowDisposition::Rejected;
        }
        let Some(finish) = message.finish.as_deref().map(str::trim) else {
            return RowDisposition::Rejected;
        };
        if finish.is_empty() {
            return RowDisposition::Rejected;
        }
        let Some(tokens) = message.tokens else {
            return RowDisposition::Rejected;
        };
        let Some(cache) = tokens.cache else {
            return RowDisposition::Rejected;
        };
        // Every dimension of the verified disjoint partition must be
        // present and non-negative; a missing or negative dimension is
        // drift, never a guessed zero.
        let Some(input) = tokens.input.filter(|v| *v >= 0) else {
            return RowDisposition::Rejected;
        };
        let Some(output) = tokens.output.filter(|v| *v >= 0) else {
            return RowDisposition::Rejected;
        };
        let Some(reasoning) = tokens.reasoning.filter(|v| *v >= 0) else {
            return RowDisposition::Rejected;
        };
        let Some(cache_read) = cache.read.filter(|v| *v >= 0) else {
            return RowDisposition::Rejected;
        };
        let Some(cache_write) = cache.write.filter(|v| *v >= 0) else {
            return RowDisposition::Rejected;
        };
        // The verified disjoint identity: when the source total exists it
        // equals the sum of the five dimensions (428/428 live). Derive
        // the total from the dimensions; a source total that disagrees is
        // drift. The total is never added on top of the components.
        let derived_total = input + output + reasoning + cache_read + cache_write;
        if let Some(total) = tokens.total {
            if total != derived_total {
                return RowDisposition::Rejected;
            }
        }
        // The verified sparse-reporting pattern: a completed, error-free
        // row whose every dimension is zero means the provider never
        // reported usage (dominated by Google / Antigravity Claude
        // thinking models live). Never stored as trustworthy measured
        // zero — dispositioned as usage-not-reported instead.
        if input == 0 && output == 0 && reasoning == 0 && cache_read == 0 && cache_write == 0 {
            return RowDisposition::UsageNotReported;
        }
        // Provider and model come directly from the source fields —
        // never inferred from model names. Provider must be an
        // identifier-shaped family; the model is verbatim (slashes are
        // observed and legitimate, e.g. openrouter's `deepseek/v4`).
        let Some(provider) = normalize_provider(message.provider_id.as_deref()) else {
            return RowDisposition::Rejected;
        };
        let Some(model) = normalize_model(message.model_id.as_deref()) else {
            return RowDisposition::Rejected;
        };
        // Cost feeds the fork-dedup fingerprint only; it is never stored
        // (the event model carries no cost). A negative or non-finite
        // cost is drift.
        if let Some(cost) = message.cost {
            if !cost.is_finite() || cost < 0.0 {
                return RowDisposition::Rejected;
            }
        }
        // normalize_provider/normalize_model validated these were present
        // and well-formed; the raw provider string rides along verbatim
        // as provenance.
        let source_provider_id = message.provider_id.clone().unwrap_or_default();
        let fingerprint = event_fingerprint(
            self.time_created,
            &source_provider_id,
            &model,
            input,
            output,
            reasoning,
            cache_read,
            cache_write,
            message.cost,
        );
        RowDisposition::Event(UsageEvent {
            id: format!("{USAGE_SOURCE_OPENCODE}:{}", sha256_hex(&fingerprint)),
            source: USAGE_SOURCE_OPENCODE.to_string(),
            provider,
            source_provider_id,
            model,
            event_at: completed_at,
            observed_at: now_ms,
            input_tokens: input as u64,
            cache_read_tokens: cache_read as u64,
            cache_write_tokens: cache_write as u64,
            output_tokens: output as u64,
            reasoning_tokens: reasoning as u64,
            total_tokens: derived_total as u64,
        })
    }
}

/// Canonical fork-dedup fingerprint. Length-prefixes the free-form
/// provider/model strings so no payload can spoof a field boundary; the
/// cost rides as its exact IEEE bit pattern so the hash cannot drift with
/// float formatting.
fn event_fingerprint(
    time_created: i64,
    provider_id: &str,
    model: &str,
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
    cost: Option<f64>,
) -> String {
    let cost_repr = cost
        .map(|cost| format!("{:016x}", cost.to_bits()))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "v1\u{1f}{time_created}\u{1f}{}:{provider_id}\u{1f}{}:{model}\u{1f}\
         {input}\u{1f}{output}\u{1f}{reasoning}\u{1f}{cache_read}\u{1f}{cache_write}\u{1f}{cost_repr}",
        provider_id.len(),
        model.len(),
    )
}

fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Normalizes the OpenCode `providerID` into the provider axis: verbatim
/// (trimmed) — the source reports the provider directly and it is never
/// inferred from the model name. An empty or non-identifier-shaped value
/// fails closed to `None`.
fn normalize_provider(raw: Option<&str>) -> Option<String> {
    let provider = raw?.trim();
    if provider.is_empty() {
        return None;
    }
    if !provider
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return None;
    }
    Some(provider.to_string())
}

/// The model id, verbatim (trimmed). Slashes are legitimate (observed on
/// openrouter models like `deepseek/deepseek-v4-flash-0717`).
fn normalize_model(raw: Option<&str>) -> Option<String> {
    let model = raw?.trim();
    if model.is_empty() {
        return None;
    }
    Some(model.to_string())
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

/// Copies the database and its `-wal` sidecar into a fresh temporary
/// directory. The source files are only ever opened for reading by the
/// copy calls themselves. The `-shm` sidecar is deliberately not copied:
/// it is a transient shared-memory index the copy rebuilds from the
/// copied WAL.
fn snapshot_database(db_path: &Path, wal_path: &Path) -> Result<SnapshotDirGuard, String> {
    let dir = std::env::temp_dir().join(format!(
        "limitscope-opencode-usage-snapshot-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir)
        .map_err(|error| format!("could not prepare a snapshot directory: {error}"))?;
    let guard = SnapshotDirGuard { path: dir };
    fs::copy(db_path, guard.path.join("opencode.db"))
        .map_err(|error| format!("could not copy the database: {error}"))?;
    if wal_path.exists() {
        fs::copy(wal_path, guard.path.join("opencode.db-wal"))
            .map_err(|error| format!("could not copy the database WAL: {error}"))?;
    }
    Ok(guard)
}

/// Verifies the snapshot carries the `message` table with every column
/// this reader selects. Additive columns are fine; a missing column or
/// the table itself is unsupported drift.
fn verify_schema(connection: &rusqlite::Connection) -> Result<(), String> {
    let table_present: bool = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            rusqlite::params![USAGE_TABLE],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .map_err(|error| format!("could not inspect the schema: {error}"))?;
    if !table_present {
        return Err(format!("table {USAGE_TABLE} is missing"));
    }
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({USAGE_TABLE})"))
        .map_err(|error| format!("could not inspect the schema: {error}"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("could not inspect the schema: {error}"))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|error| format!("could not inspect the schema: {error}"))?;
    for required in REQUIRED_COLUMNS {
        if !columns.iter().any(|column| column == required) {
            return Err(format!("column {required} is missing"));
        }
    }
    Ok(())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn fingerprint_of(db: &std::fs::Metadata, wal: Option<&std::fs::Metadata>) -> SourceFingerprint {
    SourceFingerprint {
        db_size: db.len(),
        db_mtime_ms: mtime_ms(db),
        wal_size: wal.map(|meta| meta.len()),
        wal_mtime_ms: wal.map(mtime_ms),
    }
}

fn current_fingerprint(db_path: &Path, wal_path: &Path) -> Option<SourceFingerprint> {
    let db = fs::metadata(db_path).ok()?;
    let wal = fs::metadata(wal_path).ok();
    Some(fingerprint_of(&db, wal.as_ref()))
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// A display-safe read-failure summary that never carries a path or raw
/// error bytes to the UI.
fn read_failure_detail() -> String {
    "the OpenCode database snapshot could not be read".to_string()
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_intelligence::{
        run_collection_pass, UsageIntelligenceStore, UsageRange, USAGE_INTELLIGENCE_FILE_NAME,
        USAGE_SOURCE_CODEX, USAGE_SOURCE_ZCODE,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW_MS: i64 = 1_791_288_000_000; // 2026-10-06T12:00:00.000Z

    const MIN: i64 = 60_000;

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
                "limitscope-usage-opencode-test-{}-{}-{}",
                tag,
                std::process::id(),
                unique
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }
    }

    /// The verified live schema of the `message` table (2026-10-06).
    const FIXTURE_SCHEMA: &str = "CREATE TABLE message (
        id text primary key,
        session_id text not null,
        time_created integer not null,
        time_updated integer not null,
        data text not null
    )";

    /// A synthetic assistant accounting row. Only accounting metadata is
    /// ever generated; no prompt/response text exists in any fixture.
    struct AssistantFixture {
        time_created: i64,
        /// Overrides the JSON `time.created` while the table column keeps
        /// `time_created` — the drift case.
        created_override: Option<i64>,
        time_completed: Option<i64>,
        provider: String,
        model: String,
        input: i64,
        output: i64,
        reasoning: i64,
        cache_read: i64,
        cache_write: i64,
        total: Option<i64>,
        cost: Option<f64>,
        finish: Option<String>,
        error: bool,
        role: &'static str,
        omit_cache: bool,
    }

    impl AssistantFixture {
        /// A healthy completed row: disjoint dimensions summing to the
        /// source total.
        fn completed(time_created: i64) -> Self {
            Self {
                time_created,
                created_override: None,
                time_completed: Some(time_created + 4_000),
                provider: "openai".to_string(),
                model: "gpt-5.6-sol".to_string(),
                input: 1_000,
                output: 400,
                reasoning: 100,
                cache_read: 300,
                cache_write: 50,
                total: Some(1_850),
                cost: Some(0.25),
                finish: Some("stop".to_string()),
                error: false,
                role: "assistant",
                omit_cache: false,
            }
        }

        /// The verified sparse-reporting pattern (usage never reported).
        fn all_zero(time_created: i64) -> Self {
            Self {
                input: 0,
                output: 0,
                reasoning: 0,
                cache_read: 0,
                cache_write: 0,
                total: None,
                cost: Some(0.0),
                finish: Some("unknown".to_string()),
                ..Self::completed(time_created)
            }
        }

        fn json(&self) -> String {
            let mut tokens = serde_json::json!({
                "input": self.input,
                "output": self.output,
                "reasoning": self.reasoning,
            });
            if !self.omit_cache {
                tokens["cache"] = serde_json::json!({
                    "read": self.cache_read,
                    "write": self.cache_write,
                });
            }
            if let Some(total) = self.total {
                tokens["total"] = serde_json::json!(total);
            }
            let mut body = serde_json::json!({
                "role": self.role,
                "providerID": self.provider,
                "modelID": self.model,
                "tokens": tokens,
                "agent": "build",
                "mode": "build",
                "parentID": "msg-parent",
                "path": {"cwd": "/proj", "root": "/proj"},
                "time": {
                    "created": self.created_override.unwrap_or(self.time_created),
                    "completed": self.time_completed,
                },
            });
            if let Some(cost) = self.cost {
                body["cost"] = serde_json::json!(cost);
            }
            if let Some(finish) = &self.finish {
                body["finish"] = serde_json::json!(finish);
            }
            if self.error {
                // Error-shaped object; its content is never read back.
                body["error"] = serde_json::json!({
                    "name": "ProviderAuthError",
                    "data": {"message": "synthetic"},
                });
            }
            body.to_string()
        }
    }

    struct Fixture {
        dir: tempdir::TempDir,
        connection: rusqlite::Connection,
    }

    impl Fixture {
        fn path(&self) -> PathBuf {
            self.dir.path().join("opencode.db")
        }

        fn insert(&self, id: &str, session: &str, time_created: i64, time_updated: i64, data: &str) {
            self.connection
                .execute(
                    "INSERT INTO message (id, session_id, time_created, time_updated, data) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![id, session, time_created, time_updated, data],
                )
                .unwrap();
        }

        fn insert_assistant(
            &self,
            id: &str,
            session: &str,
            time_updated: i64,
            row: &AssistantFixture,
        ) {
            self.insert(id, session, row.time_created, time_updated, &row.json());
        }

        fn insert_user(&self, id: &str, session: &str, time_updated: i64) {
            let data = serde_json::json!({
                "role": "user",
                "agent": "build",
                "model": {"modelID": "gpt-5.6-sol", "providerID": "openai"},
                "summary": true,
                "time": {"created": time_updated, "completed": time_updated},
            })
            .to_string();
            self.insert(id, session, time_updated, time_updated, &data);
        }

        /// An assistant row whose `time_updated` column value is not a
        /// millisecond integer. SQLite's dynamic column typing accepts it
        /// and schema verification (names only) still passes, but
        /// `MAX(time_updated)` then returns TEXT — a deterministic
        /// stand-in for any baseline high-water query failure.
        fn insert_corrupt_time_updated(&self, id: &str) {
            self.connection
                .execute(
                    "INSERT INTO message (id, session_id, time_created, time_updated, data) \
                     VALUES (?1, 'ses-corrupt', 0, 'not-a-timestamp', ?2)",
                    rusqlite::params![
                        id,
                        AssistantFixture::completed(NOW_MS - 61 * MIN).json()
                    ],
                )
                .unwrap();
        }
    }

    fn fixture(tag: &str) -> Fixture {
        let dir = tempdir::temp_dir(tag);
        let path = dir.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(FIXTURE_SCHEMA).unwrap();
        Fixture { dir, connection }
    }

    /// The one historical row used to anchor baselines in most tests.
    fn seed_old(fix: &Fixture) {
        fix.insert_assistant(
            "msg-old",
            "ses-old",
            NOW_MS - 60 * MIN,
            &AssistantFixture::completed(NOW_MS - 61 * MIN),
        );
    }

    fn store_at(
        tag: &str,
        db: Option<PathBuf>,
    ) -> (UsageIntelligenceStore, tempdir::TempDir) {
        let dir = tempdir::temp_dir(tag);
        let store = UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        )
        .with_codex_home_resolver(Box::new(|| None))
        .with_opencode_db_resolver(Box::new(move || db.clone()));
        (store, dir)
    }

    fn enable(store: &UsageIntelligenceStore) {
        store.set_enabled(true);
    }

    // 1. disabled = zero probes: the resolver is never even called.
    #[test]
    fn disabled_collect_never_resolves_or_touches_the_source() {
        let dir = tempdir::temp_dir("disabled");
        let fixture_path = {
            let fix = fixture("disabled-db");
            seed_old(&fix);
            fix.path()
        };
        let probes = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = probes.clone();
        let store = UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        )
        .with_opencode_db_resolver(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Some(fixture_path.clone())
        }));
        let outcome = collect(&store);
        assert_eq!(outcome, OpencodeScanOutcome::Disabled);
        assert_eq!(
            probes.load(Ordering::SeqCst),
            0,
            "the path is never resolved while disabled"
        );
    }

    // 2. missing database: source-absent, no error surface, and the
    //    failure record carries no baseline.
    #[test]
    fn missing_database_reports_source_absent() {
        let empty = tempdir::temp_dir("absent");
        let (store, _dir) = store_at("absent-store", Some(empty.path().join("opencode.db")));
        enable(&store);
        let outcome = collect(&store);
        assert_eq!(outcome, OpencodeScanOutcome::Absent);
        let cursor = store
            .cursor(USAGE_SOURCE_OPENCODE)
            .expect("diagnostics record exists");
        assert!(!cursor.baselined);
        assert_eq!(cursor.state, SourceState::SourceAbsent);
    }

    // 3. first enable baselines at the current high-water mark (the
    //    newest assistant row's commit stamp, +1) and imports zero
    //    historical rows — including in-flight and error rows.
    #[test]
    fn first_enable_baselines_without_importing_history() {
        let fix = fixture("baseline");
        seed_old(&fix);
        fix.insert_assistant(
            "msg-error",
            "ses-old",
            NOW_MS - 50 * MIN,
            &AssistantFixture {
                error: true,
                finish: None,
                total: None,
                ..AssistantFixture::completed(NOW_MS - 51 * MIN)
            },
        );
        // In-flight: created after everything else, never completed.
        fix.insert_assistant(
            "msg-inflight",
            "ses-old",
            NOW_MS - 40 * MIN,
            &AssistantFixture {
                time_completed: None,
                finish: None,
                total: None,
                ..AssistantFixture::completed(NOW_MS - 40 * MIN)
            },
        );
        let (store, _dir) = store_at("baseline-store", Some(fix.path()));
        enable(&store);
        let outcome = collect(&store);
        let OpencodeScanOutcome::Baseline { watermark_ms } = outcome else {
            panic!("expected a baseline, got {outcome:?}");
        };
        assert_eq!(
            watermark_ms,
            NOW_MS - 40 * MIN + 1,
            "the watermark sits one millisecond above the newest assistant row"
        );
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.events_in_range, 0, "no historical row was imported");
        assert_eq!(
            dto.collection_started_at.as_deref(),
            Some("2026-10-06T12:00:00.000Z")
        );
        assert!(dto.sources.iter().any(|s| s.source == USAGE_SOURCE_OPENCODE));
    }

    // 4. an empty database still baselines (at zero) instead of erroring.
    #[test]
    fn empty_database_baselines_at_zero() {
        let fix = fixture("empty");
        let (store, _dir) = store_at("empty-store", Some(fix.path()));
        enable(&store);
        let outcome = collect(&store);
        assert_eq!(outcome, OpencodeScanOutcome::Baseline { watermark_ms: 0 });
    }

    // 4a. a failed baseline high-water query fails closed: ReadFailure,
    //      no baseline, no cursor movement, no import — the backlog is
    //      never admitted as history.
    #[test]
    fn baseline_query_failure_fails_closed_without_baselining() {
        let fix = fixture("baseline-failure");
        fix.insert_corrupt_time_updated("msg-corrupt");
        seed_old(&fix);
        let (store, _dir) = store_at("baseline-failure-store", Some(fix.path()));
        enable(&store);
        let outcome = collect(&store);
        assert!(matches!(outcome, OpencodeScanOutcome::ReadFailure(_)));
        let cursor = store
            .cursor(USAGE_SOURCE_OPENCODE)
            .expect("the failure diagnostic exists");
        assert!(
            !cursor.baselined,
            "a failed high-water query must never baseline"
        );
        assert_eq!(cursor.state, SourceState::ReadFailure);
        assert!(
            store.collection_started_at().is_none(),
            "no baseline, no disclosure anchor"
        );
        assert!(
            store.events().is_empty(),
            "no historical row may be imported"
        );
    }

    // 4b. after a later successful pass the source baselines at the
    //      then-current high-water mark and still imports zero historical
    //      rows; normal incremental accounting resumes.
    #[test]
    fn baseline_query_failure_recovers_on_a_later_pass() {
        let fix = fixture("baseline-recovery");
        fix.insert_corrupt_time_updated("msg-corrupt");
        seed_old(&fix);
        let (store, _dir) = store_at("baseline-recovery-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(
            collect(&store),
            OpencodeScanOutcome::ReadFailure(_)
        ));

        // The corruption is repaired; the remaining row predates the
        // recovery pass and stays backlog.
        fix.connection
            .execute("DELETE FROM message WHERE id = 'msg-corrupt'", [])
            .unwrap();
        let outcome = collect(&store);
        let OpencodeScanOutcome::Baseline { watermark_ms } = outcome else {
            panic!("expected a baseline on recovery, got {outcome:?}");
        };
        assert_eq!(
            watermark_ms,
            NOW_MS - 60 * MIN + 1,
            "the recovery baseline sits one millisecond above the high-water mark"
        );
        assert!(store.events().is_empty(), "still zero historical rows");
        assert_eq!(store.collection_started_at(), Some(NOW_MS));

        // Normal accounting resumes: a post-baseline row is imported.
        fix.insert_assistant(
            "msg-new",
            "ses-new",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected {
            accepted,
            watermark_ms,
            ..
        } = outcome
        else {
            panic!("expected a collection after recovery, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        assert_eq!(watermark_ms, NOW_MS - 30 * MIN);
        assert_eq!(store.events().len(), 1);
    }

    // 5. incremental rows after the baseline are normalized with the
    //    verified disjoint-partition semantics: dimensions map directly
    //    and the total is derived from them, never summed on top.
    #[test]
    fn incremental_rows_are_normalized_from_the_disjoint_partition() {
        let fix = fixture("incremental");
        seed_old(&fix);
        let (store, _dir) = store_at("incremental-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));

        fix.insert_assistant(
            "msg-new",
            "ses-new",
            NOW_MS - 30 * MIN,
            &AssistantFixture {
                total: None, // no source total: derived from the dimensions
                ..AssistantFixture::completed(NOW_MS - 34 * MIN)
            },
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected {
            accepted,
            usage_not_reported,
            watermark_ms,
            ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        assert_eq!(usage_not_reported, 0);
        assert_eq!(watermark_ms, NOW_MS - 30 * MIN);

        let dto = store.aggregate(UsageRange::Today, Some(NOW_MS - 8 * 60 * 60_000));
        assert_eq!(dto.events_in_range, 1);
        assert_eq!(dto.groups[0].provider, "openai");
        let model = &dto.groups[0].models[0];
        assert_eq!(model.model, "gpt-5.6-sol");
        assert_eq!(
            model.input_tokens, 1_000,
            "the disjoint source input is already non-cached"
        );
        assert_eq!(model.cache_read_tokens, 300);
        assert_eq!(model.cache_write_tokens, 50);
        assert_eq!(
            model.output_tokens, 400,
            "the disjoint source output is already ex-reasoning"
        );
        assert_eq!(model.reasoning_tokens, 100);
        assert_eq!(
            model.total_tokens, 1_850,
            "the total is the sum of the five dimensions"
        );
        // The raw source provider string and harness identity survive as
        // provenance on the stored event.
        let stored = &store.events()[0];
        assert_eq!(stored.source_provider_id, "openai");
        assert_eq!(stored.source, USAGE_SOURCE_OPENCODE);
        assert!(stored.id.starts_with("opencode:"));
        assert_eq!(stored.event_at, NOW_MS - 34 * MIN + 4_000);
    }

    // 6. a present source total is cross-checked against the disjoint
    //    sum and never double-counted.
    #[test]
    fn source_total_is_cross_checked_and_never_double_counted() {
        let fix = fixture("source-total");
        seed_old(&fix);
        let (store, _dir) = store_at("source-total-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.insert_assistant(
            "msg-total",
            "ses-new",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        assert_eq!(
            store.events()[0].total_tokens, 1_850,
            "1_000 + 400 + 100 + 300 + 50, once"
        );
    }

    // 7. same commit stamp, distinct rows: both are ingested (composite
    //    ordering, content-addressed identities).
    #[test]
    fn same_timestamp_distinct_rows_are_both_ingested() {
        let fix = fixture("same-ts");
        seed_old(&fix);
        let (store, _dir) = store_at("same-ts-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        let a = AssistantFixture::completed(NOW_MS - 34 * MIN);
        let b = AssistantFixture {
            provider: "xai".to_string(),
            model: "grok-4.6".to_string(),
            input: 2_000,
            output: 800,
            reasoning: 0,
            cache_read: 0,
            cache_write: 0,
            total: Some(2_800),
            ..AssistantFixture::completed(NOW_MS - 34 * MIN)
        };
        fix.insert_assistant("msg-a", "ses", NOW_MS - 30 * MIN, &a);
        fix.insert_assistant("msg-b", "ses", NOW_MS - 30 * MIN, &b);
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 2, "both same-stamp rows are stored");
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.totals.events, 2);
        assert_eq!(dto.groups.len(), 2, "openai and xai stay separate providers");
    }

    // 8. non-assistant and non-final rows never enter the store, and
    //    they are filtered in SQL — not counted as malformed.
    #[test]
    fn user_inflight_and_error_rows_are_excluded_in_sql() {
        let fix = fixture("selection");
        seed_old(&fix);
        let (store, _dir) = store_at("selection-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.insert_user("msg-user", "ses", NOW_MS - 30 * MIN);
        fix.insert_assistant(
            "msg-inflight",
            "ses",
            NOW_MS - 30 * MIN,
            &AssistantFixture {
                time_completed: None,
                finish: None,
                total: None,
                ..AssistantFixture::completed(NOW_MS - 34 * MIN)
            },
        );
        fix.insert_assistant(
            "msg-error",
            "ses",
            NOW_MS - 29 * MIN,
            &AssistantFixture {
                error: true,
                finish: None,
                total: None,
                ..AssistantFixture::completed(NOW_MS - 33 * MIN)
            },
        );
        // Completed but never finalized in place (no finish key at all):
        // ambiguous by the completion predicate, so excluded in SQL like
        // the in-flight row — never stored, never counted as malformed.
        fix.insert_assistant(
            "msg-finishless",
            "ses",
            NOW_MS - 28 * MIN,
            &AssistantFixture {
                finish: None,
                ..AssistantFixture::completed(NOW_MS - 33 * MIN)
            },
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0, "no non-final row is collected");
        assert_eq!(
            rejected, 0,
            "they are filtered in SQL, not counted as malformed"
        );
        assert_eq!(store.aggregate(UsageRange::ThirtyDays, None).events_in_range, 0);
    }

    // 9. the sparse-reporting pattern: completed, error-free, all-zero
    //    rows are never stored as measured zero; they are counted as
    //    usage-not-reported and disposition the row (the watermark moves
    //    past them, so rows left behind the lookback are not re-read).
    #[test]
    fn all_zero_completed_rows_are_usage_not_reported_and_disposition_the_cursor() {
        let fix = fixture("all-zero");
        seed_old(&fix);
        let (store, _dir) = store_at("all-zero-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));

        // Baseline watermark: NOW-60min + 1. An unreported row one
        // minute later…
        let early = NOW_MS - 59 * MIN;
        fix.insert_assistant("msg-zero-early", "ses", early, &AssistantFixture::all_zero(early));
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected {
            accepted,
            usage_not_reported,
            ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0);
        assert_eq!(usage_not_reported, 1);
        assert_eq!(store.events().len(), 0, "an unreported row is never stored");

        // …another one nineteen minutes later is inside the lookback
        // window together with the first.
        let late = NOW_MS - 40 * MIN;
        fix.insert_assistant("msg-zero-late", "ses", late, &AssistantFixture::all_zero(late));
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { usage_not_reported, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(usage_not_reported, 2);

        // A good row twenty-one minutes after the baseline pushes the
        // watermark past it; the early unreported row now sits behind
        // the watermark's lookback and is not re-read (its disposition
        // was terminal), while the late one still is.
        let good = NOW_MS - 20 * MIN;
        fix.insert_assistant(
            "msg-good",
            "ses",
            good,
            &AssistantFixture::completed(good - 4_000),
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected {
            accepted,
            usage_not_reported,
            ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        assert_eq!(
            usage_not_reported, 1,
            "the early row's usage-not-reported disposition is terminal"
        );
        assert_eq!(store.events().len(), 1);
    }

    // 10. invalid completed rows are rejected and counted — never
    //     guessed into the store.
    #[test]
    fn invalid_rows_reject_instead_of_guessing() {
        let fix = fixture("invalid");
        seed_old(&fix);
        let (store, _dir) = store_at("invalid-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));

        let mut blank_model = AssistantFixture::completed(NOW_MS - 34 * MIN);
        blank_model.model = "   ".to_string();
        let mut bad_provider = AssistantFixture::completed(NOW_MS - 34 * MIN);
        bad_provider.provider = "bad provider!".to_string();
        let mut negative = AssistantFixture::completed(NOW_MS - 34 * MIN);
        negative.input = -5;
        let mut missing_cache = AssistantFixture::completed(NOW_MS - 34 * MIN);
        missing_cache.omit_cache = true;
        let mut bad_total = AssistantFixture::completed(NOW_MS - 34 * MIN);
        bad_total.total = Some(999);
        let mut future_stamp = AssistantFixture::completed(NOW_MS - 34 * MIN);
        future_stamp.time_created = NOW_MS + 60 * MIN;
        future_stamp.time_completed = Some(NOW_MS + 64 * MIN);
        let mut drift_stamp = AssistantFixture::completed(NOW_MS - 34 * MIN);
        drift_stamp.created_override = Some(NOW_MS - 35 * MIN);
        let mut negative_cost = AssistantFixture::completed(NOW_MS - 34 * MIN);
        negative_cost.cost = Some(-1.0);
        // An empty-string finish passes the SQL predicate (the key is
        // present) and must still fail closed in Rust.
        let mut empty_finish = AssistantFixture::completed(NOW_MS - 34 * MIN);
        empty_finish.finish = Some("".to_string());

        for (id, row) in [
            ("msg-blank-model", blank_model),
            ("msg-bad-provider", bad_provider),
            ("msg-negative", negative),
            ("msg-missing-cache", missing_cache),
            ("msg-bad-total", bad_total),
            ("msg-future", future_stamp),
            ("msg-drift", drift_stamp),
            ("msg-neg-cost", negative_cost),
            ("msg-empty-finish", empty_finish),
        ] {
            fix.insert_assistant(id, "ses", NOW_MS - 30 * MIN, &row);
        }
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0);
        assert_eq!(rejected, 9, "every invalid row is counted, none stored");
    }

    // 11. fork dedup, same window: two copies of one executed request
    //     (distinct ids and sessions, identical accounting) collapse to
    //     one event.
    #[test]
    fn fork_copies_in_one_window_collapse_to_one_event() {
        let fix = fixture("fork-same-window");
        seed_old(&fix);
        let (store, _dir) = store_at("fork-same-window-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        let original = AssistantFixture::completed(NOW_MS - 34 * MIN);
        fix.insert_assistant("msg-original", "ses-a", NOW_MS - 30 * MIN, &original);
        // The fork copy: new message id and session, preserved
        // time_created/completed/accounting, fresh commit stamp.
        fix.insert_assistant("msg-fork-copy", "ses-b", NOW_MS - 25 * MIN, &original);
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1, "the fork copy counts once");
        let events = store.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].total_tokens, 1_850, "tokens are not multiplied");
        let dto = store.aggregate(UsageRange::Today, Some(NOW_MS - 8 * 60 * 60_000));
        assert_eq!(dto.totals.total_tokens, 1_850);
    }

    // 12. fork dedup, across scans: a fork of an already-ingested
    //     request is re-identity-matched by the store (duplicate), and
    //     totals never grow.
    #[test]
    fn fork_copy_of_already_ingested_request_is_a_store_duplicate() {
        let fix = fixture("fork-cross-scan");
        seed_old(&fix);
        let (store, _dir) = store_at("fork-cross-scan-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.insert_assistant(
            "msg-original",
            "ses-a",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        let first = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = first else {
            panic!("expected a collection, got {first:?}");
        };
        assert_eq!(accepted, 1);

        // The fork happens later: fresh commit stamp, preserved content.
        fix.insert_assistant(
            "msg-fork-copy",
            "ses-b",
            NOW_MS - 10 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        let second = collect(&store);
        let OpencodeScanOutcome::Collected {
            accepted, duplicates, ..
        } = second
        else {
            panic!("expected a collection, got {second:?}");
        };
        assert_eq!(accepted, 0, "the fork copy is not stored again");
        assert_eq!(
            duplicates, 1,
            "both rows map to one content address; the store counts the duplicate once"
        );
        assert_eq!(store.events().len(), 1);
        assert_eq!(
            store.aggregate(UsageRange::ThirtyDays, None).totals.total_tokens,
            1_850
        );
    }

    // 13. legitimate distinct requests with identical token totals are
    //     not collapsed (different creation stamps separate the
    //     fingerprints).
    #[test]
    fn distinct_requests_are_not_collapsed() {
        let fix = fixture("distinct");
        seed_old(&fix);
        let (store, _dir) = store_at("distinct-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.insert_assistant(
            "msg-one",
            "ses",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        // Same model, dims, and cost — but a different creation stamp.
        fix.insert_assistant(
            "msg-two",
            "ses",
            NOW_MS - 29 * MIN,
            &AssistantFixture::completed(NOW_MS - 33 * MIN),
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 2, "distinct requests stay distinct");
        assert_eq!(store.events().len(), 2);
    }

    // 14. restart safety: the cursor, fingerprint, and owned identities
    //     survive a store reopen; an unchanged source costs no copy and
    //     a changed source ingests only what is new.
    #[test]
    fn restart_safe_cursor_and_identities() {
        let fix = fixture("restart");
        seed_old(&fix);
        let path = fix.path();
        let (store, dir) = store_at("restart-store", Some(path.clone()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.insert_assistant(
            "msg-new",
            "ses",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        let stored_id = store.events()[0].id.clone();
        drop(store);

        let reopened = UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        )
        .with_codex_home_resolver(Box::new(|| None))
        .with_opencode_db_resolver(Box::new(move || Some(path.clone())));
        assert!(reopened.enabled(), "the opt-in flag survives restart");
        let cursor = reopened
            .cursor(USAGE_SOURCE_OPENCODE)
            .expect("cursor survives");
        assert!(cursor.baselined);
        // Unchanged fingerprint: no snapshot copy is taken.
        assert_eq!(collect(&reopened), OpencodeScanOutcome::Unchanged);
        // A changed source ingests only what is new; the pre-restart row
        // is re-read inside the lookback and matched by its identity —
        // the same content address computed before the restart.
        fix.insert_assistant(
            "msg-newer",
            "ses",
            NOW_MS - 5 * MIN,
            &AssistantFixture::completed(NOW_MS - 9 * MIN),
        );
        let outcome = collect(&reopened);
        let OpencodeScanOutcome::Collected {
            accepted, duplicates, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        assert_eq!(duplicates, 1, "the pre-restart event is not re-ingested");
        assert!(reopened.events().iter().any(|event| event.id == stored_id));
    }

    // 15. the scan ceiling fails closed: an over-ceiling window ingests
    //     nothing and does not move the cursor.
    #[test]
    fn over_ceiling_window_fails_closed() {
        let fix = fixture("ceiling");
        seed_old(&fix);
        let (store, _dir) = store_at("ceiling-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.connection.execute_batch("BEGIN").unwrap();
        for index in 0..=(SCAN_CEILING as i64) {
            fix.insert_assistant(
                &format!("msg-flood-{index}"),
                "ses",
                NOW_MS - 30 * MIN + index,
                &AssistantFixture::completed(NOW_MS - 31 * MIN + index),
            );
        }
        fix.connection.execute_batch("COMMIT").unwrap();
        let outcome = collect(&store);
        let OpencodeScanOutcome::CeilingExceeded { rows } = outcome else {
            panic!("expected a ceiling failure, got {outcome:?}");
        };
        assert_eq!(rows, SCAN_CEILING + 1);
        assert_eq!(
            store.aggregate(UsageRange::ThirtyDays, None).events_in_range,
            0,
            "nothing from the over-ceiling window was stored"
        );
        assert_eq!(
            store.cursor(USAGE_SOURCE_OPENCODE).map(|c| c.state),
            Some(SourceState::ScanCeiling)
        );
        // Recovery: shrinking the window below the ceiling heals the
        // source without losing the baseline.
        fix.connection
            .execute("DELETE FROM message WHERE id LIKE 'msg-flood-%'", [])
            .unwrap();
        fix.insert_assistant(
            "msg-after-ceiling",
            "ses",
            NOW_MS - 5 * MIN,
            &AssistantFixture::completed(NOW_MS - 9 * MIN),
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
    }

    // 16. unexpected schema (a required column gone) fails closed with
    //     no partial ingest and no baseline; additive drift is tolerated.
    #[test]
    fn missing_required_column_fails_closed_but_additive_drift_is_tolerated() {
        let dir = tempdir::temp_dir("schema");
        let path = dir.path().join("opencode.db");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE message (
                        id text primary key,
                        time_created integer not null,
                        time_updated integer not null
                    )",
                )
                .unwrap();
        }
        let (store, _store_dir) = store_at("schema-store", Some(path));
        enable(&store);
        let outcome = collect(&store);
        assert_eq!(outcome, OpencodeScanOutcome::SchemaUnsupported);
        assert_eq!(
            store.cursor(USAGE_SOURCE_OPENCODE).map(|c| c.state),
            Some(SourceState::SchemaUnsupported)
        );
        assert_eq!(
            store.collection_started_at(),
            None,
            "no baseline is anchored on drift"
        );

        // Additive drift (an extra column, like a future OpenCode
        // release): the verified reader keeps working.
        let fix = fixture("schema-additive");
        fix.connection
            .execute_batch("ALTER TABLE message ADD COLUMN future_column integer default 0")
            .unwrap();
        seed_old(&fix);
        let (store2, _dir2) = store_at("schema-additive-store", Some(fix.path()));
        enable(&store2);
        assert!(matches!(
            collect(&store2),
            OpencodeScanOutcome::Baseline { .. }
        ));
    }

    // 17. the source database is byte-identical after collection passes,
    //     including with a live WAL writer, and the collector leaves no
    //     files beside the source.
    #[test]
    fn source_database_bytes_are_unchanged_by_collection() {
        let fix = fixture("unchanged");
        fix.connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        seed_old(&fix);
        let wal_path = sibling(&fix.path(), "-wal");
        let db_before = fs::read(fix.path()).unwrap();
        let wal_before = fs::read(&wal_path).unwrap();
        let listing_before = sorted_dir_names(fix.dir.path());

        let (store, _dir) = store_at("unchanged-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.insert_assistant(
            "msg-new",
            "ses",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        let _ = collect(&store);

        let db_after = fs::read(fix.path()).unwrap();
        let wal_after = fs::read(&wal_path).unwrap();
        assert_eq!(
            db_after, db_before,
            "the main database file is byte-identical"
        );
        assert!(
            wal_after.len() >= wal_before.len(),
            "the WAL only ever grows through its own writer"
        );
        assert_eq!(
            sorted_dir_names(fix.dir.path()),
            listing_before,
            "the collector creates no file beside the source (no -shm copy, no journals)"
        );
    }

    // 18. a database without a WAL sidecar (DELETE journal mode)
    //     snapshots and scans fine on the main file alone.
    #[test]
    fn db_without_wal_snapshots_alone() {
        let fix = fixture("no-wal");
        seed_old(&fix);
        assert!(!sibling(&fix.path(), "-wal").exists());
        let (store, _dir) = store_at("no-wal-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
    }

    // 19. the unchanged fingerprint short-circuit: an idle source is
    //     not re-copied (observable via the Unchanged outcome).
    #[test]
    fn unchanged_source_skips_the_snapshot_copy() {
        let fix = fixture("unchanged-fp");
        seed_old(&fix);
        let (store, _dir) = store_at("unchanged-fp-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        let second = collect(&store);
        assert_eq!(
            second,
            OpencodeScanOutcome::Unchanged,
            "no source change means no copy"
        );
    }

    // 20. provider and model come directly from the source fields,
    //     verbatim, never inferred; multi-model sessions separate
    //     events.
    #[test]
    fn provider_and_model_come_from_source_fields_and_multi_model_sessions_split() {
        let fix = fixture("attribution");
        seed_old(&fix);
        let (store, _dir) = store_at("attribution-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        // One session, two models across two providers.
        fix.insert_assistant(
            "msg-m1",
            "ses-multi",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        fix.insert_assistant(
            "msg-m2",
            "ses-multi",
            NOW_MS - 29 * MIN,
            &AssistantFixture {
                provider: "google".to_string(),
                model: "antigravity-gemini-3.8-flash".to_string(),
                input: 700,
                output: 200,
                reasoning: 0,
                cache_read: 0,
                cache_write: 0,
                total: Some(900),
                ..AssistantFixture::completed(NOW_MS - 33 * MIN)
            },
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 2);
        let events = store.events();
        let sol = events.iter().find(|e| e.model == "gpt-5.6-sol").unwrap();
        assert_eq!(sol.provider, "openai", "providerID preserved");
        assert_eq!(sol.source_provider_id, "openai");
        let gem = events
            .iter()
            .find(|e| e.model == "antigravity-gemini-3.8-flash")
            .unwrap();
        assert_eq!(gem.provider, "google");
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(
            dto.groups.len(),
            2,
            "a multi-model session yields separate provider groups"
        );
        assert!(dto.groups.iter().any(|g| g.provider == "openai"));
        assert!(dto.groups.iter().any(|g| g.provider == "google"));
    }

    #[test]
    fn provider_normalization_covers_the_contract_rules() {
        assert_eq!(normalize_provider(Some("openai")).as_deref(), Some("openai"));
        assert_eq!(
            normalize_provider(Some("opencode-go")).as_deref(),
            Some("opencode-go")
        );
        assert_eq!(
            normalize_provider(Some("  xai  ")).as_deref(),
            Some("xai"),
            "surrounding whitespace is trimmed, not guessed around"
        );
        assert_eq!(normalize_provider(Some("")).as_deref(), None);
        assert_eq!(normalize_provider(None).as_deref(), None);
        assert_eq!(normalize_provider(Some("bad provider!")).as_deref(), None);
        assert_eq!(
            normalize_model(Some("deepseek/deepseek-v4-flash-0731")).as_deref(),
            Some("deepseek/deepseek-v4-flash-0731"),
            "slashes are legitimate model-id shapes"
        );
        assert_eq!(normalize_model(Some("  ")).as_deref(), None);
        assert_eq!(normalize_model(None).as_deref(), None);
    }

    // 21. the owned identity is a pure content address: the same
    //     accounting yields the same id, and every fingerprint field
    //     discriminates.
    #[test]
    fn event_identity_is_a_deterministic_content_address() {
        let fingerprint = event_fingerprint(
            1_788_551_518_116,
            "google",
            "antigravity-gemini-3.8-flash",
            1_000,
            400,
            100,
            300,
            50,
            Some(0.25),
        );
        let id = format!("{USAGE_SOURCE_OPENCODE}:{}", sha256_hex(&fingerprint));
        let again = format!(
            "{USAGE_SOURCE_OPENCODE}:{}",
            sha256_hex(&event_fingerprint(
                1_788_551_518_116,
                "google",
                "antigravity-gemini-3.8-flash",
                1_000,
                400,
                100,
                300,
                50,
                Some(0.25),
            ))
        );
        assert_eq!(id, again, "same content, same identity");
        assert!(id.starts_with("opencode:"));
        assert_eq!(id.len(), "opencode:".len() + 64);

        // Every fingerprint field moves the identity.
        let variants = [
            event_fingerprint(1_788_551_518_117, "google", "antigravity-gemini-3.8-flash", 1_000, 400, 100, 300, 50, Some(0.25)),
            event_fingerprint(1_788_551_518_116, "openai", "antigravity-gemini-3.8-flash", 1_000, 400, 100, 300, 50, Some(0.25)),
            event_fingerprint(1_788_551_518_116, "google", "gpt-5.6-sol", 1_000, 400, 100, 300, 50, Some(0.25)),
            event_fingerprint(1_788_551_518_116, "google", "antigravity-gemini-3.8-flash", 1_001, 400, 100, 300, 50, Some(0.25)),
            event_fingerprint(1_788_551_518_116, "google", "antigravity-gemini-3.8-flash", 1_000, 401, 100, 300, 50, Some(0.25)),
            event_fingerprint(1_788_551_518_116, "google", "antigravity-gemini-3.8-flash", 1_000, 400, 101, 300, 50, Some(0.25)),
            event_fingerprint(1_788_551_518_116, "google", "antigravity-gemini-3.8-flash", 1_000, 400, 100, 301, 50, Some(0.25)),
            event_fingerprint(1_788_551_518_116, "google", "antigravity-gemini-3.8-flash", 1_000, 400, 100, 300, 51, Some(0.25)),
            event_fingerprint(1_788_551_518_116, "google", "antigravity-gemini-3.8-flash", 1_000, 400, 100, 300, 50, Some(0.26)),
            event_fingerprint(1_788_551_518_116, "google", "antigravity-gemini-3.8-flash", 1_000, 400, 100, 300, 50, None),
        ];
        for variant in variants {
            assert_ne!(
                sha256_hex(&fingerprint),
                sha256_hex(&variant),
                "every accounting field discriminates the identity"
            );
        }
    }

    // 22. the derived accounting layers (step-finish parts, session
    //     aggregate) are never summed: inflated rows in sibling tables
    //     cannot move the totals.
    #[test]
    fn sibling_accounting_layers_are_never_summed() {
        let fix = fixture("layers");
        // The duplicate layers, seeded with deliberately inflated
        // accounting. The reader has no code path that touches them.
        fix.connection
            .execute_batch(
                "CREATE TABLE part (id text primary key, session_id text not null, message_id text, data text not null);
                 CREATE TABLE session (id text primary key, time_created integer not null, time_updated integer not null, data text not null);
                 INSERT INTO part VALUES ('part-step', 'ses', 'msg-old', '{\"type\":\"step-finish\",\"tokens\":{\"input\":999999,\"output\":888888}}');
                 INSERT INTO session VALUES ('ses', 1, 1, '{\"tokens\":{\"input\":777777,\"output\":666666}}');",
            )
            .unwrap();
        seed_old(&fix);
        let (store, _dir) = store_at("layers-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.insert_assistant(
            "msg-new",
            "ses",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(
            dto.totals.total_tokens, 1_850,
            "only message.data.tokens is accounted"
        );
    }

    // 23. source isolation in the shared collection pass: a failing
    //     OpenCode source (schema drift) leaves ZCode collecting, and a
    //     failing ZCode source leaves OpenCode collecting. Codex is
    //     pointed nowhere (absent) and stays isolated likewise.
    #[test]
    fn opencode_failure_is_isolated_from_the_other_sources() {
        // OpenCode broken (a message table with no usable columns),
        // ZCode healthy.
        let bad_dir = tempdir::temp_dir("iso-bad");
        let bad_path = bad_dir.path().join("opencode.db");
        {
            let connection = rusqlite::Connection::open(&bad_path).unwrap();
            connection
                .execute_batch("CREATE TABLE message (id text primary key)")
                .unwrap();
        }
        let good_fix = fixture("iso-good");
        seed_old(&good_fix);
        let zcode_dir = tempdir::temp_dir("iso-zcode");
        let zcode_db = zcode_dir.path().join("db.sqlite");
        {
            let connection = rusqlite::Connection::open(&zcode_db).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE model_usage (
                        id text primary key, logical_request_id text not null,
                        session_id text not null, provider_id text not null,
                        model_id text not null, status text not null,
                        started_at integer not null, completed_at integer,
                        input_tokens integer not null default 0,
                        output_tokens integer not null default 0,
                        reasoning_tokens integer not null default 0,
                        cache_creation_input_tokens integer not null default 0,
                        cache_read_input_tokens integer not null default 0,
                        provider_total_tokens integer,
                        computed_total_tokens integer not null default 0)",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO model_usage (id, logical_request_id, session_id, provider_id, \
                     model_id, status, started_at, completed_at, input_tokens, output_tokens, \
                     reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens, \
                     provider_total_tokens, computed_total_tokens) \
                     VALUES ('z-old', 'z-old', 's', 'account:zai-individual-coding-plan', \
                     'GLM-5.3', 'completed', ?1, ?2, 10000, 2000, 0, 0, 8000, 12000, 12000)",
                    rusqlite::params![NOW_MS - 120 * MIN, NOW_MS - 60 * MIN],
                )
                .unwrap();
        }
        let store_dir = tempdir::temp_dir("iso-store");
        let (store, _dir) = {
            let bad = bad_path.clone();
            let zcode = zcode_db.clone();
            let store = UsageIntelligenceStore::open_with(
                store_dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
                fixed_clock(),
                Box::new(move || Some(zcode.clone())),
            )
            .with_codex_home_resolver(Box::new(|| None))
            .with_opencode_db_resolver(Box::new(move || Some(bad.clone())));
            (store, store_dir)
        };
        enable(&store);
        run_collection_pass(&store);
        assert!(
            store.cursor(USAGE_SOURCE_ZCODE).unwrap().baselined,
            "ZCode baselines despite the OpenCode failure"
        );
        assert_eq!(
            store.cursor(USAGE_SOURCE_OPENCODE).unwrap().state,
            SourceState::SchemaUnsupported,
            "the OpenCode failure is surfaced on its own diagnostics"
        );

        // ZCode broken (absent), OpenCode healthy.
        let fix = fixture("iso-opencode-good");
        seed_old(&fix);
        let empty = tempdir::temp_dir("iso-zcode-absent");
        let store_dir2 = tempdir::temp_dir("iso-store2");
        let (store2, _dir2) = {
            let fix_path = fix.path();
            let empty_path = empty.path().join("db.sqlite");
            let store = UsageIntelligenceStore::open_with(
                store_dir2.path().join(USAGE_INTELLIGENCE_FILE_NAME),
                fixed_clock(),
                Box::new(move || Some(empty_path.clone())),
            )
            .with_codex_home_resolver(Box::new(|| None))
            .with_opencode_db_resolver(Box::new(move || Some(fix_path.clone())));
            (store, store_dir2)
        };
        enable(&store2);
        run_collection_pass(&store2);
        assert!(
            store2.cursor(USAGE_SOURCE_OPENCODE).unwrap().baselined,
            "OpenCode baselines despite the ZCode failure"
        );
        assert_eq!(
            store2.cursor(USAGE_SOURCE_ZCODE).unwrap().state,
            SourceState::SourceAbsent
        );
        assert_eq!(
            store2.cursor(USAGE_SOURCE_CODEX).unwrap().state,
            SourceState::SourceAbsent
        );
    }

    // 24. the shared clear: owned OpenCode events and cursor are reset,
    //     the source stays untouched, and the next pass re-baselines at
    //     the current high-water mark without importing history.
    #[test]
    fn clear_rebaselines_without_backfill() {
        let fix = fixture("clear");
        seed_old(&fix);
        let (store, _dir) = store_at("clear-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), OpencodeScanOutcome::Baseline { .. }));
        fix.insert_assistant(
            "msg-new",
            "ses",
            NOW_MS - 30 * MIN,
            &AssistantFixture::completed(NOW_MS - 34 * MIN),
        );
        let outcome = collect(&store);
        let OpencodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);

        let db_before = fs::read(fix.path()).unwrap();
        assert!(store.clear_owned());
        assert_eq!(store.cursor(USAGE_SOURCE_OPENCODE), None, "cursor reset");
        assert_eq!(store.collection_started_at(), None);
        assert!(store.enabled(), "clearing is not disabling");

        // The next pass re-baselines at the current high-water mark and
        // imports nothing — the pre-clear usage never reappears.
        let outcome = collect(&store);
        let OpencodeScanOutcome::Baseline { watermark_ms } = outcome else {
            panic!("expected a re-baseline, got {outcome:?}");
        };
        assert_eq!(watermark_ms, NOW_MS - 30 * MIN + 1);
        assert_eq!(store.events().len(), 0);
        assert_eq!(
            fs::read(fix.path()).unwrap(),
            db_before,
            "the foreign database is untouched by the clear"
        );
    }

    fn sorted_dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    // Live proof (bounded, read-only). Run explicitly via
    // `cargo test live_proof -- --ignored --nocapture`; never part of
    // the ordinary suite. Exercises the REAL collector against the REAL
    // OpenCode database on this machine with the opt-in enabled only
    // inside this test path, and proves:
    //
    // - the source resolves and the schema verifies;
    // - the first observation baselines at the live high-water mark and
    //   imports ZERO historical rows (no backlog, no fake zero usage);
    // - the main database file is byte-identical after the pass
    //   (streamed FNV-1a over the whole file — the collector's only
    //   touch is the read-side of its snapshot copy);
    // - no OpenCode CLI command runs, no model/API request is made, and
    //   no usage payload is printed.
    //
    // The WAL is reported (not asserted): a live OpenCode writer
    // legitimately appends to it concurrently, so its growth is
    // unattributable and carries no collector signal. The fixture-based
    // test above carries the exact byte-identity proof.
    #[test]
    #[ignore = "live proof: reads the real local OpenCode database (read-only)"]
    fn live_proof_baseline_skips_backlog() {
        let home = std::env::home_dir().expect("a home directory");
        let db_path = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        assert!(
            db_path.exists(),
            "the real OpenCode database is expected on this machine"
        );

        let dir = tempdir::temp_dir("live-proof");
        let resolver_path = db_path.clone();
        let store = UsageIntelligenceStore::open_with(
            dir.path().join(USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(|| None),
        )
        .with_codex_home_resolver(Box::new(|| None))
        .with_opencode_db_resolver(Box::new(move || Some(resolver_path.clone())));

        let wal_path = sibling(&db_path, "-wal");
        let db_hash_before = streamed_fnv1a(&db_path).expect("hash the database before");
        let wal_len_before = std::fs::metadata(&wal_path)
            .map(|meta| meta.len())
            .unwrap_or(0);

        // The explicit enable exists ONLY inside this test path.
        store.set_enabled(true);
        let outcome = collect(&store);
        let OpencodeScanOutcome::Baseline { watermark_ms } = outcome else {
            panic!("expected a baseline on the live source, got {outcome:?}");
        };

        let events = store.events();
        assert!(
            events.is_empty(),
            "no historical row may be imported on the live baseline"
        );
        let cursor = store
            .cursor(USAGE_SOURCE_OPENCODE)
            .expect("the live cursor exists");
        assert!(cursor.baselined);
        assert!(
            watermark_ms > 0,
            "the live source has assistant rows to anchor the watermark"
        );
        assert_eq!(cursor.watermark_ms, watermark_ms);
        assert!(
            store.collection_started_at().is_some(),
            "the disclosure anchor is set by the baseline"
        );

        let db_hash_after = streamed_fnv1a(&db_path).expect("hash the database after");
        assert_eq!(
            db_hash_before, db_hash_after,
            "the live database file must be byte-identical after the pass"
        );
        let wal_len_after = std::fs::metadata(&wal_path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        println!(
            "live proof: baseline watermark {} ms, imported {} events, db bytes unchanged (fnv1a {:#018x}), wal {} -> {} bytes (concurrent writer expected)",
            watermark_ms,
            events.len(),
            db_hash_after,
            wal_len_before,
            wal_len_after,
        );
    }

    /// Streamed 64-bit FNV-1a over a file: a whole-file equality witness
    /// without holding the file in memory.
    fn streamed_fnv1a(path: &Path) -> Option<u64> {
        use std::io::Read;
        let mut file = std::fs::File::open(path).ok()?;
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut buffer = [0u8; 1 << 20];
        loop {
            let read = file.read(&mut buffer).ok()?;
            if read == 0 {
                return Some(hash);
            }
            for byte in &buffer[..read] {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
}
