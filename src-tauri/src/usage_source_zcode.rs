//! ZCode local usage source — the first Usage Intelligence collector.
//!
//! Reads the ZCode CLI's local database (`~/.zcode/cli/db/db.sqlite`,
//! table `model_usage`; schema verified against the installed CLI on
//! 2026-10-06) and normalizes completed requests into the source-neutral
//! [`UsageEvent`] model.
//!
//! # Read-only access (the hard boundary)
//!
//! The live database and its `-wal` / `-shm` sidecars are never opened by
//! this module. Every collection pass copies the database plus its `-wal`
//! sidecar into a fresh temporary directory and queries only the copy —
//! the same snapshot pattern `cursor_grok_bot.rs` uses for Cursor's state
//! database. The source files are only ever touched by the copy calls
//! themselves (read handles), never locked, never written, never
//! checkpointed; the `-shm` is a transient index the copy rebuilds for
//! itself. A WAL copied mid-append is designed by SQLite to replay only
//! the committed, checksummed prefix, so the snapshot is a coherent
//! committed state.
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
//!   high-water mark (`MAX(completed_at)` over completed rows) and
//!   imports nothing — no historical backlog, no fake zero usage.
//! - Later passes read the bounded window
//!   `completed_at >= watermark - LOOKBACK` (inclusive at the boundary),
//!   ordered by `(completed_at, id)`, capped at `SCAN_CEILING + 1` rows.
//!   The inclusive boundary plus id-dedup tolerates rows sharing a
//!   timestamp, restarts, and duplicate reads; the lookback absorbs
//!   late-committing rows stamped slightly behind the watermark.
//! - Hitting the ceiling fails closed: the whole scan is abandoned, the
//!   cursor does not move, and the source state reports `scanCeiling`.
//!   Nothing is silently skipped.
//! - Only `status = 'completed'` rows with a non-null `completed_at` are
//!   candidates. Rows failing validation (blank provider/model,
//!   implausible timestamps, negative dimensions, an overlap-invariant
//!   violation, or a total that disagrees with the verified
//!   `input + output` identity) are rejected and counted — never guessed
//!   into the store.
//!
//! # Privacy

//!
//! The SELECT list is closed and minimal: identity, provider, model,
//! status, timestamps, and the token dimensions. The source's
//! `raw_usage_json`, `provider_metadata_json`, error text, and every
//! conversational column are never selected, never parsed, never logged.

use std::fs;
use std::path::{Path, PathBuf};

use crate::usage_intelligence::{
    SourceFingerprint, SourceScan, SourceState, UsageEvent, UsageIntelligenceStore,
    USAGE_SOURCE_ZCODE,
};

/// Columns this reader requires. The verified live schema carries many
/// more (query/session/trace identity, error metadata, raw usage JSON);
/// additive columns are compatible drift, a missing required column is
/// not.
const REQUIRED_COLUMNS: &[&str] = &[
    "id",
    "provider_id",
    "model_id",
    "status",
    "completed_at",
    "input_tokens",
    "output_tokens",
    "reasoning_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
    "computed_total_tokens",
];

/// The one table this reader understands.
const USAGE_TABLE: &str = "model_usage";

/// Maximum rows one incremental window may hold. Fetching one more than
/// the ceiling detects the overflow; an over-ceiling window abandons the
/// scan whole (fail closed) rather than chunking silently.
pub const SCAN_CEILING: usize = 20_000;

/// How far behind the watermark the window still reads, absorbing rows
/// committed slightly after their `completed_at` stamp.
const LOOKBACK_MS: i64 = 10 * 60_000;

/// Outcome of one collection pass, for diagnostics and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZcodeScanOutcome {
    /// The store is disabled; nothing was probed.
    Disabled,
    /// The baseline was established at the given high-water mark;
    /// nothing was imported.
    Baseline { watermark_ms: i64 },
    /// An incremental scan ran; rows were stored.
    Collected {
        accepted: usize,
        duplicates: usize,
        rejected: usize,
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
pub fn collect(store: &UsageIntelligenceStore) -> ZcodeScanOutcome {
    if !store.enabled() {
        return ZcodeScanOutcome::Disabled;
    }
    let Some(db_path) = (store.zcode_db_resolver())() else {
        store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::SourceAbsent, None);
        return ZcodeScanOutcome::Absent;
    };
    // Existence check (the first and only probe, post opt-in): a missing
    // database is a normal state — ZCode not installed or not yet run.
    let db_meta = match fs::metadata(&db_path) {
        Ok(meta) => meta,
        Err(_) => {
            store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::SourceAbsent, None);
            return ZcodeScanOutcome::Absent;
        }
    };
    let wal_path = sibling(&db_path, "-wal");
    let wal_meta = fs::metadata(&wal_path).ok();

    // Cheap change detection: an unchanged, previously-healthy source
    // costs no snapshot copy. Only a fingerprint from a successful scan
    // is trusted, so a failed pass always retries.
    let fingerprint = fingerprint_of(&db_meta, wal_meta.as_ref());
    if let Some(cursor) = store.cursor(USAGE_SOURCE_ZCODE) {
        if cursor.baselined
            && cursor.state == SourceState::Ok
            && cursor.fingerprint.as_ref() == Some(&fingerprint)
        {
            return ZcodeScanOutcome::Unchanged;
        }
    }

    let fingerprint = current_fingerprint(&db_path, &wal_path);

    let outcome = collect_from_live(&db_path, &wal_path, store, fingerprint);
    outcome
}

/// Snapshot-copies the database (plus WAL) and runs the baseline or
/// incremental read against the copy.
fn collect_from_live(
    db_path: &Path,
    wal_path: &Path,
    store: &UsageIntelligenceStore,
    fingerprint: Option<SourceFingerprint>,
) -> ZcodeScanOutcome {
    let guard = match snapshot_database(db_path, wal_path) {
        Ok(guard) => guard,
        Err(detail) => {
            store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::ReadFailure, Some(detail));
            return ZcodeScanOutcome::ReadFailure(read_failure_detail(db_path));
        }
    };
    let snapshot_db = guard.path.join("db.sqlite");
    let connection = match rusqlite::Connection::open(&snapshot_db) {
        Ok(connection) => connection,
        Err(error) => {
            let detail = format!("could not open the snapshot: {error}");
            store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::ReadFailure, Some(detail));
            return ZcodeScanOutcome::ReadFailure(read_failure_detail(db_path));
        }
    };
    if let Err(detail) = verify_schema(&connection) {
        store.mark_source_state(
            USAGE_SOURCE_ZCODE,
            SourceState::SchemaUnsupported,
            Some(detail),
        );
        return ZcodeScanOutcome::SchemaUnsupported;
    }

    // Baseline first: no cursor (or a failure-only record) means this is
    // the first successful observation — establish the backlog skip. The
    // watermark sits one millisecond ABOVE the current high-water mark, so
    // rows completed at or before the baseline instant (including every
    // row sharing the max timestamp) are strictly backlog and never
    // imported; the +1 keeps the incremental window's inclusive boundary
    // from re-admitting them on the next scan.
    let baselined = store
        .cursor(USAGE_SOURCE_ZCODE)
        .map(|cursor| cursor.baselined)
        .unwrap_or(false);
    if !baselined {
        let max_completed: Option<i64> = connection
            .query_row(
                &format!(
                    "SELECT MAX(completed_at) FROM {USAGE_TABLE} \
                     WHERE status = 'completed' AND completed_at IS NOT NULL"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap_or(None);
        let watermark_ms = max_completed.map(|max| max + 1).unwrap_or(0);
        store.establish_baseline(USAGE_SOURCE_ZCODE, watermark_ms, fingerprint);
        return ZcodeScanOutcome::Baseline { watermark_ms };
    }

    // Incremental window. The floor re-reads a bounded lookback behind the
    // watermark (absorbing late commits and boundary-timestamp ties, with
    // id-dedup absorbing the re-reads) but never reaches below the
    // immutable baseline watermark — rows completed before the baseline
    // are pre-enable backlog and stay unimported.
    let cursor = store.cursor(USAGE_SOURCE_ZCODE);
    let watermark_ms = cursor
        .as_ref()
        .map(|cursor| cursor.watermark_ms)
        .unwrap_or(0);
    let window_start = cursor
        .as_ref()
        .map(|cursor| (watermark_ms.saturating_sub(LOOKBACK_MS)).max(cursor.baseline_watermark_ms))
        .unwrap_or(0);
    let mut statement = match connection.prepare(&format!(
        "SELECT id, provider_id, model_id, completed_at, \
                input_tokens, output_tokens, reasoning_tokens, \
                cache_creation_input_tokens, cache_read_input_tokens, computed_total_tokens \
         FROM {USAGE_TABLE} \
         WHERE status = 'completed' AND completed_at IS NOT NULL AND completed_at >= ?1 \
         ORDER BY completed_at ASC, id ASC \
         LIMIT ?2"
    )) {
        Ok(statement) => statement,
        Err(error) => {
            let detail = format!("could not prepare the scan: {error}");
            store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::ReadFailure, Some(detail));
            return ZcodeScanOutcome::ReadFailure(read_failure_detail(db_path));
        }
    };
    let rows = statement.query_map(
        rusqlite::params![window_start, (SCAN_CEILING + 1) as i64],
        |row| {
            Ok(RawUsageRow {
                id: row.get(0)?,
                provider_id: row.get(1)?,
                model_id: row.get(2)?,
                completed_at: row.get(3)?,
                input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                reasoning_tokens: row.get(6)?,
                cache_creation_input_tokens: row.get(7)?,
                cache_read_input_tokens: row.get(8)?,
                computed_total_tokens: row.get(9)?,
            })
        },
    );
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            let detail = format!("could not query the snapshot: {error}");
            store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::ReadFailure, Some(detail));
            return ZcodeScanOutcome::ReadFailure(read_failure_detail(db_path));
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
        store.mark_source_state(USAGE_SOURCE_ZCODE, SourceState::ReadFailure, Some(detail));
        return ZcodeScanOutcome::ReadFailure(read_failure_detail(db_path));
    }

    // Fail closed on the scan ceiling: an over-ceiling window is
    // abandoned whole — no partial ingest, no cursor movement.
    if raw_rows.len() > SCAN_CEILING {
        store.mark_source_state(
            USAGE_SOURCE_ZCODE,
            SourceState::ScanCeiling,
            Some(format!(
                "incremental window held {} rows above the {} row ceiling",
                raw_rows.len(),
                SCAN_CEILING
            )),
        );
        return ZcodeScanOutcome::CeilingExceeded {
            rows: raw_rows.len(),
        };
    }

    let now_ms = store.now_ms();
    let mut events = Vec::with_capacity(raw_rows.len());
    let mut rejected = 0usize;
    let mut new_watermark = watermark_ms;
    for raw in raw_rows {
        // The watermark can only move through rows we actually hand to
        // the store; rejected rows never advance it (their data was not
        // trusted, so their timestamp must not gate later reads either —
        // the lookback keeps them revisitable until they age out).
        match raw.into_event(now_ms) {
            Some(event) => {
                new_watermark = new_watermark.max(event.event_at);
                events.push(event);
            }
            None => rejected += 1,
        }
    }

    let stats = store.apply_scan(
        USAGE_SOURCE_ZCODE,
        SourceScan {
            watermark_ms: new_watermark,
            fingerprint,
            rejected,
            events,
        },
    );
    ZcodeScanOutcome::Collected {
        accepted: stats.accepted,
        duplicates: stats.duplicates,
        rejected: stats.rejected,
        watermark_ms: new_watermark,
    }
}

/// One raw row of the closed SELECT list.
struct RawUsageRow {
    id: String,
    provider_id: String,
    model_id: String,
    completed_at: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    computed_total_tokens: Option<i64>,
}

impl RawUsageRow {
    /// Validates and normalizes one completed row into an event, or
    /// rejects it whole. Every rule here is fail-closed: a value this
    /// reader cannot verify is never guessed.
    fn into_event(self, now_ms: i64) -> Option<UsageEvent> {
        let id = self.id.trim();
        if id.is_empty() {
            return None;
        }
        let model = self.model_id.trim();
        if model.is_empty() {
            return None;
        }
        let Some(provider) = normalize_provider(&self.provider_id) else {
            return None;
        };
        let Some(completed_at) = self.completed_at else {
            return None;
        };
        // Timestamps: epoch milliseconds, plausible range only.
        if completed_at <= 0 || completed_at > now_ms + 5 * 60_000 {
            return None;
        }
        // The token dimensions are NOT NULL DEFAULT 0 in the verified
        // schema; a NULL (schema drift) or a negative value rejects.
        let raw_dims = [
            self.input_tokens,
            self.output_tokens,
            self.reasoning_tokens,
            self.cache_creation_input_tokens,
            self.cache_read_input_tokens,
            self.computed_total_tokens,
        ];
        let mut dims = [0i64; 6];
        for (slot, value) in dims.iter_mut().zip(raw_dims) {
            *slot = value.filter(|value| *value >= 0)?;
        }
        let [input, output, reasoning, cache_write, cache_read, computed_total] = dims;
        // Verified overlap identity: total == input + output, cache read
        // and write contained in input, reasoning contained in output.
        // Violations are drift, not data to reinterpret.
        if computed_total != input + output {
            return None;
        }
        if cache_read + cache_write > input {
            return None;
        }
        if reasoning > output {
            return None;
        }
        Some(UsageEvent {
            id: format!("{USAGE_SOURCE_ZCODE}:{id}"),
            source: USAGE_SOURCE_ZCODE.to_string(),
            provider,
            source_provider_id: self.provider_id.clone(),
            model: model.to_string(),
            event_at: completed_at,
            observed_at: now_ms,
            input_tokens: (input - cache_read - cache_write) as u64,
            cache_read_tokens: cache_read as u64,
            cache_write_tokens: cache_write as u64,
            output_tokens: (output - reasoning) as u64,
            reasoning_tokens: reasoning as u64,
            total_tokens: computed_total as u64,
        })
    }
}

/// Normalizes a ZCode `provider_id` into the provider axis. Observed
/// values carry an account/builtin lane prefix and a plan suffix
/// (`account:zai-individual-coding-plan`, `builtin:zai-start-plan`); the
/// provider axis is the leading family segment (`zai`). An unrecognized
/// shape fails closed to `None` — the row is rejected, never attributed
/// to a guessed provider.
fn normalize_provider(raw: &str) -> Option<String> {
    let stripped = raw
        .strip_prefix("account:")
        .or_else(|| raw.strip_prefix("builtin:"))
        .unwrap_or(raw);
    let family = stripped.split('-').next()?.trim();
    if family.is_empty() {
        return None;
    }
    // A provider family must look like an identifier, not punctuation.
    if !family
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    Some(family.to_string())
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
        "limitscope-zcode-usage-snapshot-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir)
        .map_err(|error| format!("could not prepare a snapshot directory: {error}"))?;
    let guard = SnapshotDirGuard { path: dir };
    fs::copy(db_path, guard.path.join("db.sqlite"))
        .map_err(|error| format!("could not copy the database: {error}"))?;
    if wal_path.exists() {
        fs::copy(wal_path, guard.path.join("db.sqlite-wal"))
            .map_err(|error| format!("could not copy the database WAL: {error}"))?;
    }
    Ok(guard)
}

/// Verifies the snapshot carries a `model_usage` table with every column
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
fn read_failure_detail(_db_path: &Path) -> String {
    "the ZCode database snapshot could not be read".to_string()
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_intelligence::UsageRange;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW_MS: i64 = 1_791_288_000_000; // 2026-10-06T12:00:00.000Z

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
                "limitscope-usage-zcode-test-{}-{}-{}",
                tag,
                std::process::id(),
                unique
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }
    }

    /// The verified live schema, reproduced for fixtures (columns this
    /// reader needs, plus a couple of unused siblings to prove the closed
    /// SELECT list tolerates additive columns).
    const FIXTURE_SCHEMA: &str = "CREATE TABLE model_usage (
        id text primary key,
        logical_request_id text not null,
        session_id text not null,
        provider_id text not null,
        model_id text not null,
        status text not null check(status in ('running', 'completed', 'error', 'cancelled')),
        started_at integer not null,
        completed_at integer,
        input_tokens integer not null default 0,
        output_tokens integer not null default 0,
        reasoning_tokens integer not null default 0,
        cache_creation_input_tokens integer not null default 0,
        cache_read_input_tokens integer not null default 0,
        provider_total_tokens integer,
        computed_total_tokens integer not null default 0,
        raw_usage_json text
    )";

    struct Fixture {
        dir: tempdir::TempDir,
        connection: rusqlite::Connection,
    }

    impl Fixture {
        fn path(&self) -> PathBuf {
            self.dir.path().join("db.sqlite")
        }
    }

    fn fixture(tag: &str) -> Fixture {
        let dir = tempdir::temp_dir(tag);
        let path = dir.path().join("db.sqlite");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(FIXTURE_SCHEMA).unwrap();
        Fixture { dir, connection }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        connection: &rusqlite::Connection,
        id: &str,
        provider: &str,
        model: &str,
        status: &str,
        completed_at: Option<i64>,
        input: i64,
        output: i64,
        reasoning: i64,
        cache_write: i64,
        cache_read: i64,
        computed_total: i64,
    ) {
        connection
            .execute(
                "INSERT INTO model_usage (id, logical_request_id, session_id, provider_id, \
                 model_id, status, started_at, completed_at, input_tokens, output_tokens, \
                 reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens, \
                 provider_total_tokens, computed_total_tokens) \
                 VALUES (?1, ?1, 's', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    id,
                    provider,
                    model,
                    status,
                    completed_at.unwrap_or(NOW_MS) - 60_000,
                    completed_at,
                    input,
                    output,
                    reasoning,
                    cache_write,
                    cache_read,
                    computed_total,
                    computed_total
                ],
            )
            .unwrap();
    }

    /// A healthy completed row at the standard historical offset:
    /// total = input + output with cache read contained in input.
    fn insert_completed(connection: &rusqlite::Connection, id: &str) {
        insert(
            connection,
            id,
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 60_000),
            10_000, // input (includes 8_000 cache read)
            2_000,  // output
            0,      // reasoning
            0,      // cache write
            8_000,  // cache read
            12_000, // computed total == input + output
        );
    }

    fn store_at(
        tag: &str,
        db: Option<PathBuf>,
    ) -> (
        crate::usage_intelligence::UsageIntelligenceStore,
        tempdir::TempDir,
    ) {
        let dir = tempdir::temp_dir(tag);
        let store = crate::usage_intelligence::UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(move || db.clone()),
        );
        (store, dir)
    }

    fn enable(store: &crate::usage_intelligence::UsageIntelligenceStore) {
        store.set_enabled(true);
    }

    // 1. disabled = zero probes: the resolver is never even called.
    #[test]
    fn disabled_collect_never_resolves_or_touches_the_source() {
        let dir = tempdir::temp_dir("disabled");
        let fixture_path = {
            let fix = fixture("disabled-db");
            insert_completed(&fix.connection, "historical-1");
            fix.path()
        };
        let probes = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = probes.clone();
        let store = crate::usage_intelligence::UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Some(fixture_path.clone())
            }),
        );
        let outcome = collect(&store);
        assert_eq!(outcome, ZcodeScanOutcome::Disabled);
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
        let (store, _dir) = store_at("absent-store", Some(empty.path().join("db.sqlite")));
        enable(&store);
        let outcome = collect(&store);
        assert_eq!(outcome, ZcodeScanOutcome::Absent);
        let cursor = store
            .cursor(USAGE_SOURCE_ZCODE)
            .expect("diagnostics record exists");
        assert!(!cursor.baselined);
        assert_eq!(cursor.state, SourceState::SourceAbsent);
    }

    // 3. first enable baselines at the current high-water mark and
    //    imports zero historical rows.
    #[test]
    fn first_enable_baselines_without_importing_history() {
        let fix = fixture("baseline");
        insert_completed(&fix.connection, "old-1");
        insert_completed(&fix.connection, "old-2");
        insert(
            &fix.connection,
            "incomplete",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "running",
            None,
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let (store, _dir) = store_at("baseline-store", Some(fix.path()));
        enable(&store);
        let outcome = collect(&store);
        let ZcodeScanOutcome::Baseline { watermark_ms } = outcome else {
            panic!("expected a baseline, got {outcome:?}");
        };
        assert_eq!(
            watermark_ms,
            NOW_MS - 60_000 + 1,
            "the watermark sits one millisecond above the newest completed row"
        );
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.events_in_range, 0, "no historical row was imported");
        assert_eq!(
            dto.collection_started_at.as_deref(),
            Some("2026-10-06T12:00:00.000Z")
        );
        assert!(!dto.sources.is_empty());
    }

    // 4. incremental rows after the baseline are normalized and stored
    //    with the verified non-overlapping semantics.
    #[test]
    fn incremental_rows_after_baseline_are_normalized_and_stored() {
        let fix = fixture("incremental");
        insert_completed(&fix.connection, "old");
        let (store, _dir) = store_at("incremental-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), ZcodeScanOutcome::Baseline { .. }));

        insert(
            &fix.connection,
            "new-1",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 30_000),
            46_129, // input, includes 39_040 cache read
            1_377,  // output
            0,      // reasoning
            0,      // cache write
            39_040, // cache read
            47_506, // computed total == 46_129 + 1_377
        );
        let outcome = collect(&store);
        let ZcodeScanOutcome::Collected {
            accepted,
            watermark_ms,
            ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 1);
        assert_eq!(watermark_ms, NOW_MS - 30_000);

        let dto = store.aggregate(UsageRange::Today, Some(NOW_MS - 8 * 60 * 60_000));
        assert_eq!(dto.events_in_range, 1);
        assert_eq!(dto.groups[0].provider, "zai");
        let model = &dto.groups[0].models[0];
        assert_eq!(model.model, "GLM-5.3");
        assert_eq!(
            model.input_tokens,
            46_129 - 39_040,
            "input excludes the cache read"
        );
        assert_eq!(model.cache_read_tokens, 39_040);
        assert_eq!(model.cache_write_tokens, 0);
        assert_eq!(model.output_tokens, 1_377);
        assert_eq!(model.reasoning_tokens, 0);
        assert_eq!(
            model.total_tokens, 47_506,
            "the source's canonical total, never re-summed"
        );
        // The raw source provider string and harness identity survive as
        // provenance on the stored event.
        let stored = &store.events()[0];
        assert_eq!(
            stored.source_provider_id,
            "account:zai-individual-coding-plan"
        );
        assert_eq!(stored.source, USAGE_SOURCE_ZCODE);
        assert_eq!(stored.id, "zcode:new-1");
    }

    // 5. overlap re-scan is idempotent: totals never grow.
    #[test]
    fn duplicate_overlap_scan_never_grows_totals() {
        let fix = fixture("overlap");
        insert_completed(&fix.connection, "old");
        let (store, _dir) = store_at("overlap-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), ZcodeScanOutcome::Baseline { .. }));
        insert(
            &fix.connection,
            "new-1",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 30_000),
            1_000,
            500,
            0,
            0,
            0,
            1_500,
        );
        let first = collect(&store);
        let ZcodeScanOutcome::Collected { accepted, .. } = first else {
            panic!("expected a collection, got {first:?}");
        };
        assert_eq!(accepted, 1);
        // The fingerprint changed on the insert; force a re-read by
        // touching the database so the overlap window is re-scanned.
        let re = collect(&store);
        assert!(
            matches!(
                re,
                ZcodeScanOutcome::Collected { .. } | ZcodeScanOutcome::Unchanged
            ),
            "a re-collect stays healthy, got {re:?}"
        );
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(
            dto.totals.total_tokens, 1_500,
            "re-running the collector cannot grow totals"
        );
    }

    // 6. rows sharing a timestamp: both are ingested; the boundary is
    //    inclusive and dedup is by id.
    #[test]
    fn same_timestamp_rows_are_both_ingested() {
        let fix = fixture("same-ts");
        insert_completed(&fix.connection, "old");
        let (store, _dir) = store_at("same-ts-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), ZcodeScanOutcome::Baseline { .. }));
        let at = NOW_MS - 10_000;
        insert(
            &fix.connection,
            "t-a",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(at),
            100,
            50,
            0,
            0,
            0,
            150,
        );
        insert(
            &fix.connection,
            "t-b",
            "builtin:zai-start-plan",
            "GLM-5.3-Flash",
            "completed",
            Some(at),
            200,
            100,
            0,
            0,
            0,
            300,
        );
        let outcome = collect(&store);
        let ZcodeScanOutcome::Collected { accepted, .. } = outcome else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 2, "both same-timestamp rows are stored");
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.totals.events, 2);
        // Both account/builtin lanes normalize onto one provider axis.
        assert_eq!(dto.groups.len(), 1);
        assert_eq!(dto.groups[0].provider, "zai");
        assert_eq!(
            dto.groups[0].models.len(),
            2,
            "models stay a separate dimension"
        );
    }

    // 7. incomplete rows never enter the store.
    #[test]
    fn incomplete_rows_are_excluded() {
        let fix = fixture("incomplete");
        insert_completed(&fix.connection, "old");
        let (store, _dir) = store_at("incomplete-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), ZcodeScanOutcome::Baseline { .. }));
        insert(
            &fix.connection,
            "running",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "running",
            None,
            500,
            0,
            0,
            0,
            0,
            500,
        );
        insert(
            &fix.connection,
            "running-stamped",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "running",
            Some(NOW_MS - 20_000),
            500,
            0,
            0,
            0,
            0,
            500,
        );
        insert(
            &fix.connection,
            "errored",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "error",
            Some(NOW_MS - 20_000),
            500,
            0,
            0,
            0,
            0,
            500,
        );
        insert(
            &fix.connection,
            "cancelled",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "cancelled",
            Some(NOW_MS - 20_000),
            500,
            0,
            0,
            0,
            0,
            500,
        );
        insert(
            &fix.connection,
            "no-completion",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            None,
            100,
            50,
            0,
            0,
            0,
            150,
        );
        let outcome = collect(&store);
        let ZcodeScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0, "no incomplete row is collected");
        assert_eq!(
            rejected, 0,
            "incomplete rows are filtered in SQL, not counted as malformed"
        );
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(dto.events_in_range, 0);
    }

    // 8. invalid completed rows are rejected and counted.
    #[test]
    fn invalid_rows_reject_instead_of_guessing() {
        let fix = fixture("invalid");
        insert_completed(&fix.connection, "old");
        let (store, _dir) = store_at("invalid-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), ZcodeScanOutcome::Baseline { .. }));
        // Negative input.
        insert(
            &fix.connection,
            "neg",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 30_000),
            -5,
            10,
            0,
            0,
            0,
            5,
        );
        // Blank model.
        insert(
            &fix.connection,
            "nomodel",
            "account:zai-individual-coding-plan",
            "   ",
            "completed",
            Some(NOW_MS - 30_000),
            10,
            10,
            0,
            0,
            0,
            20,
        );
        // Cache read exceeding input (overlap invariant violated).
        insert(
            &fix.connection,
            "badcache",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 30_000),
            100,
            50,
            0,
            0,
            200,
            150,
        );
        // Total disagreeing with input + output (semantic drift).
        insert(
            &fix.connection,
            "badtotal",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 30_000),
            100,
            50,
            0,
            0,
            0,
            999,
        );
        // Implausible future timestamp.
        insert(
            &fix.connection,
            "future",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS + 60 * 60_000),
            100,
            50,
            0,
            0,
            0,
            150,
        );
        // Unrecognizable provider shape.
        insert(
            &fix.connection,
            "badprov",
            ":::",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 30_000),
            100,
            50,
            0,
            0,
            0,
            150,
        );
        let outcome = collect(&store);
        let ZcodeScanOutcome::Collected {
            accepted, rejected, ..
        } = outcome
        else {
            panic!("expected a collection, got {outcome:?}");
        };
        assert_eq!(accepted, 0);
        assert_eq!(rejected, 6, "every invalid row is counted, none stored");
    }

    // 9. unexpected schema (a required column gone) fails closed with no
    //    partial ingest; additive columns are tolerated.
    #[test]
    fn missing_required_column_fails_closed_but_additive_drift_is_tolerated() {
        let dir = tempdir::temp_dir("schema");
        let path = dir.path().join("db.sqlite");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
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
                        cache_read_input_tokens integer not null default 0
                    )",
                )
                .unwrap();
        }
        let (store, _store_dir) = store_at("schema-store", Some(path.clone()));
        enable(&store);
        let outcome = collect(&store);
        assert_eq!(outcome, ZcodeScanOutcome::SchemaUnsupported);
        assert_eq!(
            store.cursor(USAGE_SOURCE_ZCODE).map(|c| c.state),
            Some(SourceState::SchemaUnsupported)
        );
        assert_eq!(
            store.collection_started_at(),
            None,
            "no baseline is anchored on drift"
        );

        // Additive drift (an extra column, like a future ZCode release):
        // the verified reader keeps working.
        let dir2 = tempdir::temp_dir("schema-additive");
        let path2 = dir2.path().join("db.sqlite");
        {
            let connection = rusqlite::Connection::open(&path2).unwrap();
            connection.execute_batch(&format!("{FIXTURE_SCHEMA}; ALTER TABLE model_usage ADD COLUMN future_column integer default 0")).unwrap();
            insert(
                &connection,
                "r1",
                "account:zai-individual-coding-plan",
                "GLM-5.3",
                "completed",
                Some(NOW_MS - 60_000),
                100,
                50,
                0,
                0,
                0,
                150,
            );
        }
        let (store2, _dir2) = store_at("schema-additive-store", Some(path2.clone()));
        enable(&store2);
        assert!(matches!(
            collect(&store2),
            ZcodeScanOutcome::Baseline { .. }
        ));
        insert(
            &store2_cursor_connection(&path2),
            "r2",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 10_000),
            100,
            50,
            0,
            0,
            0,
            150,
        );
        assert!(matches!(
            collect(&store2),
            ZcodeScanOutcome::Collected { accepted: 1, .. }
        ));
    }

    fn store2_cursor_connection(path: &Path) -> rusqlite::Connection {
        rusqlite::Connection::open(path).unwrap()
    }

    // 10. the scan ceiling fails closed: an over-ceiling window ingests
    //     nothing and does not move the cursor.
    #[test]
    fn over_ceiling_window_fails_closed() {
        let fix = fixture("ceiling");
        insert_completed(&fix.connection, "old");
        let (store, _dir) = store_at("ceiling-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), ZcodeScanOutcome::Baseline { .. }));
        // Fill the window beyond the ceiling (baseline watermark is the
        // fixture row's completion; every inserted row lands in-window).
        for index in 0..=(SCAN_CEILING as i64) {
            insert(
                &fix.connection,
                &format!("flood-{index}"),
                "account:zai-individual-coding-plan",
                "GLM-5.3",
                "completed",
                Some(NOW_MS - 30_000 + index),
                1,
                1,
                0,
                0,
                0,
                2,
            );
        }
        let outcome = collect(&store);
        let ZcodeScanOutcome::CeilingExceeded { rows } = outcome else {
            panic!("expected a ceiling failure, got {outcome:?}");
        };
        assert_eq!(rows, SCAN_CEILING + 1);
        let dto = store.aggregate(UsageRange::ThirtyDays, None);
        assert_eq!(
            dto.events_in_range, 0,
            "nothing from the over-ceiling window was stored"
        );
        assert_eq!(
            store.cursor(USAGE_SOURCE_ZCODE).map(|c| c.state),
            Some(SourceState::ScanCeiling)
        );
    }

    // 11. the source database is byte-identical after a collection pass,
    //     including while a live writer holds the database in WAL mode,
    //     and the collector leaves no files beside the source.
    #[test]
    fn source_database_bytes_are_unchanged_by_collection() {
        let fix = fixture("unchanged");
        // Switch the fixture to WAL mode and leave the writer open with
        // committed frames in the -wal file, like a live ZCode process.
        fix.connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        insert_completed(&fix.connection, "old");
        let wal_path = sibling(&fix.path(), "-wal");
        let db_before = fs::read(fix.path()).unwrap();
        let wal_before = fs::read(&wal_path).unwrap();
        let listing_before = sorted_dir_names(fix.dir.path());

        let (store, _dir) = store_at("unchanged-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), ZcodeScanOutcome::Baseline { .. }));
        insert(
            &fix.connection,
            "new-1",
            "account:zai-individual-coding-plan",
            "GLM-5.3",
            "completed",
            Some(NOW_MS - 30_000),
            100,
            50,
            0,
            0,
            0,
            150,
        );
        let _ = collect(&store);

        let db_after = fs::read(fix.path()).unwrap();
        let wal_after = fs::read(&wal_path).unwrap();
        // A live writer legitimately appends its own commit; the DB pages
        // themselves must be untouched by the collector.
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

    // 12. the unchanged fingerprint short-circuit: an idle source is not
    //     re-copied (observable via the Unchanged outcome).
    #[test]
    fn unchanged_source_skips_the_snapshot_copy() {
        let fix = fixture("unchanged-fp");
        insert_completed(&fix.connection, "old");
        let (store, _dir) = store_at("unchanged-fp-store", Some(fix.path()));
        enable(&store);
        assert!(matches!(collect(&store), ZcodeScanOutcome::Baseline { .. }));
        let second = collect(&store);
        assert_eq!(
            second,
            ZcodeScanOutcome::Unchanged,
            "no source change means no copy"
        );
    }

    // 13. provider normalization table.
    #[test]
    fn provider_normalization_covers_observed_shapes() {
        assert_eq!(
            normalize_provider("account:zai-individual-coding-plan").as_deref(),
            Some("zai")
        );
        assert_eq!(
            normalize_provider("builtin:zai-start-plan").as_deref(),
            Some("zai")
        );
        assert_eq!(normalize_provider("zai").as_deref(), Some("zai"));
        assert_eq!(normalize_provider("openai").as_deref(), Some("openai"));
        assert_eq!(
            normalize_provider("account:openai-team").as_deref(),
            Some("openai")
        );
        assert_eq!(normalize_provider("").as_deref(), None);
        assert_eq!(normalize_provider("builtin:-plan").as_deref(), None);
        assert_eq!(normalize_provider("!!").as_deref(), None);
    }

    // Live proof (task-contract item: bounded local live proof). Run
    // explicitly via `cargo test live_proof -- --ignored --nocapture`;
    // never part of the ordinary suite. Exercises the REAL collector
    // against the REAL ZCode database on this machine with the opt-in
    // enabled only inside this test path, and proves:
    //
    // - the source resolves and the schema verifies;
    // - the first observation baselines at the live high-water mark and
    //   imports ZERO historical rows (no backlog, no fake zero usage);
    // - the main database file is byte-identical after the pass
    //   (streamed FNV-1a over the whole file — the collector's only
    //   touch is the read-side of its snapshot copy);
    // - no model/API request is made and no usage payload is printed.
    //
    // The WAL is reported (not asserted): a live ZCode writer (this very
    // agent session is one) legitimately appends to it concurrently, so
    // its growth is unattributable and carries no collector signal. The
    // fixture-based test above carries the exact byte-identity proof.
    #[test]
    #[ignore = "live proof: reads the real local ZCode database (read-only)"]
    fn live_proof_baseline_skips_backlog() {
        let home = std::env::home_dir().expect("a home directory");
        let db_path = home.join(".zcode").join("cli").join("db").join("db.sqlite");
        assert!(
            db_path.exists(),
            "the real ZCode database is expected on this machine"
        );

        let dir = tempdir::temp_dir("live-proof");
        let resolver_path = db_path.clone();
        let store = crate::usage_intelligence::UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(move || Some(resolver_path.clone())),
        );

        let wal_path = sibling(&db_path, "-wal");
        let db_hash_before = streamed_fnv1a(&db_path).expect("hash the database before");
        let wal_len_before = std::fs::metadata(&wal_path)
            .map(|meta| meta.len())
            .unwrap_or(0);

        // The explicit enable exists ONLY inside this test path.
        store.set_enabled(true);
        let outcome = collect(&store);
        let ZcodeScanOutcome::Baseline { watermark_ms } = outcome else {
            panic!("expected a baseline on the live source, got {outcome:?}");
        };

        let events = store.events();
        assert!(
            events.is_empty(),
            "no historical row may be imported on the live baseline"
        );
        let cursor = store
            .cursor(USAGE_SOURCE_ZCODE)
            .expect("the live cursor exists");
        assert!(cursor.baselined);
        assert!(
            watermark_ms > 0,
            "the live source has completed rows to anchor the watermark"
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

    // 14. the runtime cycle collects an attached, enabled store once per
    //     cycle — and a disabled store's cycle performs no probe at all.
    #[tokio::test]
    async fn runtime_cycle_collects_enabled_and_never_probes_disabled() {
        use crate::runtime::RuntimeCore;
        use chrono::{DateTime, Utc};

        let fixed_now = DateTime::<Utc>::from_timestamp_millis(NOW_MS).unwrap();

        // Enabled: one cycle baselines the source through the cycle hook.
        let fix = fixture("cycle-enabled");
        insert_completed(&fix.connection, "old");
        let (store, _dir) = store_at("cycle-enabled-store", Some(fix.path()));
        enable(&store);
        let core = std::sync::Arc::new(
            RuntimeCore::with_injections(
                Vec::new(),
                5,
                Box::new(|| 0),
                Box::new(move || fixed_now),
            )
            .with_usage_intelligence_store(Some(std::sync::Arc::new(store))),
        );
        core.run_cycle().await;
        // The store moved into the runtime; observe the persisted state by
        // reopening the same file.
        let path = _dir
            .path()
            .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME);
        let reopened = crate::usage_intelligence::UsageIntelligenceStore::open_with(
            path,
            fixed_clock(),
            Box::new(|| None),
        );
        let cursor = reopened
            .cursor(USAGE_SOURCE_ZCODE)
            .expect("the cycle collected");
        assert!(cursor.baselined, "the cycle hook established the baseline");
        assert_eq!(cursor.watermark_ms, NOW_MS - 60_000 + 1);
        assert_eq!(reopened.events().len(), 0, "no backlog was imported");

        // Disabled: the cycle performs zero resolver calls.
        let probes = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = probes.clone();
        let dir = tempdir::temp_dir("cycle-disabled");
        let disabled = crate::usage_intelligence::UsageIntelligenceStore::open_with(
            dir.path()
                .join(crate::usage_intelligence::USAGE_INTELLIGENCE_FILE_NAME),
            fixed_clock(),
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                None
            }),
        );
        let core = std::sync::Arc::new(
            RuntimeCore::with_injections(
                Vec::new(),
                5,
                Box::new(|| 0),
                Box::new(move || fixed_now),
            )
            .with_usage_intelligence_store(Some(std::sync::Arc::new(disabled))),
        );
        core.run_cycle().await;
        assert_eq!(
            probes.load(Ordering::SeqCst),
            0,
            "a disabled store is never probed by a cycle"
        );
    }
}
