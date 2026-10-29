//! Rust-owned quota history — v0.5 "More Rust Behind the UI" step 2.
//!
//! History moves out of the webview (formerly localStorage via
//! `src/lib/quotaHistory.ts`) into the shared runtime: this module is the
//! single owner of observation recording, persistence, retention,
//! deduplication, validation/self-healing, account-aware partitioning, and
//! the clear mutation. The runtime appends once per completed cycle from the
//! same normalized snapshot it broadcasts, so main open + floating open is
//! one observation stream, not two. TypeScript only reads history
//! (`get_history`), clears it on user request (`clear_history`), and hands
//! over the legacy localStorage blob once (`import_legacy_history`); the
//! prediction engine stays in TS and consumes these observations unchanged.
//!
//! Semantic parity with the retired TS store is test-for-test (the TS rules
//! and their ported tests):
//!
//! - observation identity: `(providerId, account, windowLabel)` — the
//!   account slot partitions multi-account providers; two accounts of one
//!   provider never share dedup, bounds, or entries;
//! - retention: 24 hours, relative to the injected clock; an observation
//!   exactly 24 hours old is still retained (age > max drops);
//! - clock skew: a stamp more than 5 minutes in the future is rejected;
//! - per-window bound: the newest 500 samples per logical window;
//! - dedup: same-timestamp observations of one window collapse, the newest
//!   input winning — repeated stale provider snapshots add at most one
//!   entry per distinct snapshot time, and quota cycles never merge (a
//!   fresh cycle is never starved by the cycle it replaced);
//! - validation: blank identifiers, non-finite percentages, unparseable
//!   `observedAt`, and a present-but-invalid `account` reject the
//!   observation; Codex load and record additionally require a v2-form
//!   `chatgpt-v2:<64 lowercase hex>` identity on every stored row;
//!   out-of-range percentages clamp, and an unparseable `resetAt` degrades
//!   to absent;
//! - self-healing: a corrupt or foreign-version blob degrades to empty (or
//!   the salvageable per-entry subset) and the sanitized state is written
//!   back, so corruption never survives to the next session and never
//!   crashes startup;
//! - non-Codex account identities keep the provider-generated, display-safe
//!   token contract (`key:3456`, `xai:<id>`). Codex history keeps only the
//!   v2-form `chatgpt-v2:<64 lowercase hex>` identity on load and record;
//!   legacy and unattributed Codex rows are discarded. Import rejects every
//!   Codex row because this identity form did not exist in legacy storage.
//!
//! Persistence is one bounded JSON file (`quota-history-v1.json`) in the
//! Tauri app-data directory — no database, no framework — written
//! temp-then-rename so a crash cannot leave a half-written blob. The file
//! is local-only and carries only the five observation fields plus their
//! optional two; no token, key, or credential material can appear in the
//! schema (pinned by tests).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::{ProviderUsageDto, RuntimeHandle};

/// Detailed recent window: observations within 24 hours are retained in full detail.
pub const QUOTA_HISTORY_RECENT_WINDOW_MS: i64 = 24 * 60 * 60_000;

/// Total retention window: observations older than 7 days (relative to now) are pruned.
pub const QUOTA_HISTORY_RETENTION_MS: i64 = 7 * 24 * 60 * 60_000;

/// Hard cap per logical window (`providerId` + `account` + `windowLabel`) for
/// the recent detailed 24h tier, applied to the newest samples.
pub const QUOTA_HISTORY_MAX_PER_WINDOW: usize = 500;

/// Hard cap per logical window for the compacted tier (24h to 7d).
pub const QUOTA_HISTORY_MAX_COMPACTED_PER_WINDOW: usize = 350;

/// Compaction bucket size for the 24h–7d tier: at most one sample per 30 minutes
/// within each distinct quota reset cycle.
pub const QUOTA_HISTORY_COMPACT_BUCKET_MS: i64 = 30 * 60_000;

/// Observations stamped further than this in the future are rejected as
/// clock skew (mirrors the engine's `clockSkewToleranceMs` default).
pub const QUOTA_HISTORY_CLOCK_SKEW_TOLERANCE_MS: i64 = 5 * 60_000;

/// Versioned file name, alongside the other app-data artifacts.
pub const HISTORY_FILE_NAME: &str = "quota-history-v1.json";

const BLOB_VERSION: u32 = 2;

const CODEX_PROVIDER_IDS: [&str; 2] = ["openai-codex", "openai-codex:secondary"];
const CODEX_HISTORY_IDENTITY_PREFIX: &str = "chatgpt-v2:";
const CODEX_HISTORY_IDENTITY_HEX_LEN: usize = 64;

/// One percentage snapshot of a single quota window at a point in time —
/// the exact wire shape the TS history produced (`QuotaObservation` in
/// `src/lib/quotaHistory.ts`), so the prediction engine consumes Rust
/// history unmodified. A logical window is identified by the
/// `(providerId, account, windowLabel)` triple, never by the label alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaObservation {
    pub provider_id: String,
    pub window_label: String,
    /// Percent of the window already consumed, clamped to 0–100.
    pub used_percent: f64,
    /// When this snapshot was taken, canonicalized to ISO-8601 UTC.
    pub observed_at: String,
    /// When the window is scheduled to reset. Optional: some providers omit
    /// it, and an unparseable value degrades to absent rather than rejecting
    /// the observation (matching the engine's tolerance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
    /// Stable account identity the observation belongs to (the provider's
    /// `AccountAttribution.identity` token). Optional: providers that cannot
    /// prove an account stay unpartitioned. Different accounts of one
    /// provider never mix — dedup, bounds, and prediction input all key on
    /// this field. Codex rows require `chatgpt-v2:` plus 64 lowercase hex
    /// characters; other providers keep their display-safe identity tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// Outcome of one recording pass (the retired TS `RecordResult`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordResult {
    pub accepted: usize,
    pub rejected: usize,
    /// False when the updated history could not reach disk — the in-memory
    /// (and therefore prediction-visible) state is still updated, and the
    /// next successful recording pass re-persists the full merged state.
    pub persisted: bool,
}

/// Outcome of one legacy import (accepted/rejected entry counts). Import is
/// idempotent: re-importing the same blob adds nothing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub accepted: usize,
    pub rejected: usize,
}

#[derive(Default)]
struct StoreState {
    /// Canonical, sorted history — the single in-memory source of truth.
    observations: Vec<QuotaObservation>,
    /// Monotonic counter, bumped on every state change. Broadcast in the
    /// runtime snapshot so consumers re-pull only when history changed.
    revision: u64,
}

/// The quota history store: bounded state behind a small mutex, persisted to
/// one JSON file. Locking model: mutate state under the lock, release, then
/// write the file — filesystem I/O never holds the lock, and provider fetch
/// tasks never wait on it.
pub struct QuotaHistoryStore {
    state: Mutex<StoreState>,
    path: PathBuf,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl QuotaHistoryStore {
    /// Opens (and loads/heals) the store at the given app-data path.
    pub fn open(path: PathBuf) -> Self {
        Self::open_with_clock(path, Box::new(epoch_now_ms))
    }

    /// Test constructor with an injectable clock (epoch milliseconds).
    pub fn open_with_clock(path: PathBuf, now_ms: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        let store = Self {
            state: Mutex::new(StoreState::default()),
            path,
            now_ms,
        };
        store.load_from_disk();
        store
    }

    fn now(&self) -> i64 {
        (self.now_ms)()
    }

    // ---- load / self-heal ----

    /// Loads the persisted blob. A missing file is a healthy empty history;
    /// corruption or a foreign version degrades to the salvageable subset
    /// (or empty), and the sanitized state is written back so degradation
    /// does not survive to the next session. Never crashes startup.
    fn load_from_disk(&self) {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            // Absent (first run, or cleared) and unreadable both start from
            // empty; the next write re-seeds the file either way.
            Err(_) => return,
        };
        let parsed = parse_blob(&raw);
        let now_ms = self.now();
        let retained = apply_retention(parsed.observations, now_ms);
        let degraded = !parsed.healthy || retained.len() != parsed.total;
        {
            let mut state = self.state.lock().unwrap();
            state.observations = retained.clone();
        }
        if degraded {
            // Self-heal: persist the sanitized state so corruption,
            // duplicates, and aged-out entries are not re-read next start.
            self.persist(&retained);
        }
    }

    // ---- recording ----

    /// Records observations from one completed runtime cycle and persists
    /// the bounded result. Invalid entries and entries outside the
    /// retention/skew window are rejected, not stored; same-timestamp
    /// duplicates of one window replace the stored entry instead of growing
    /// it. The revision is bumped only when the merged history actually
    /// changed, so an idle re-snapshot costs no re-pull on the TS side.
    pub fn record(&self, candidates: Vec<QuotaObservation>) -> RecordResult {
        let now_ms = self.now();
        let cutoff = now_ms - QUOTA_HISTORY_RETENTION_MS;
        let mut valid = Vec::new();
        let mut rejected = 0;
        for candidate in candidates {
            match validate_canonical(candidate, cutoff, now_ms) {
                Some(observation) => valid.push(observation),
                None => rejected += 1,
            }
        }

        let (merged, changed) = {
            let mut state = self.state.lock().unwrap();
            let mut all = state.observations.clone();
            all.extend(valid.iter().cloned());
            let merged = apply_retention(all, now_ms);
            let changed = merged != state.observations;
            if changed {
                state.observations = merged.clone();
                state.revision += 1;
            }
            (merged, changed)
        };
        // Write happens outside the lock; failure keeps the in-memory state
        // (predictions keep working) and the next changed pass re-persists.
        let persisted = if changed { self.persist(&merged) } else { true };
        RecordResult {
            accepted: valid.len(),
            rejected,
            persisted,
        }
    }

    /// The full history for prediction (exact 24-hour retention window).
    /// Pruning on read keeps the prediction input identical even when cycles pause.
    pub fn history(&self) -> Vec<QuotaObservation> {
        self.history_range(None, None, None, "24h")
    }

    /// Queries history filtered by range ("24h" | "7d" | "30d") and optional
    /// provider, account, and window label filters.
    ///
    /// `account_filter`:
    /// - `None`: match any account
    /// - `Some(None)`: match specifically unattributed observations
    /// - `Some(Some(id))`: match specifically that account identity
    pub fn history_range(
        &self,
        provider_id: Option<&str>,
        account_filter: Option<Option<&str>>,
        window_label: Option<&str>,
        range_str: &str,
    ) -> Vec<QuotaObservation> {
        let now_ms = self.now();
        let (retained, changed) = {
            let mut state = self.state.lock().unwrap();
            let retained = apply_retention(state.observations.clone(), now_ms);
            let changed = retained != state.observations;
            if changed {
                state.observations = retained.clone();
                state.revision += 1;
            }
            (retained, changed)
        };
        if changed {
            self.persist(&retained);
        }

        let range_ms = match range_str {
            "7d" => 7 * 24 * 60 * 60_000,
            "30d" => 30 * 24 * 60 * 60_000,
            _ => QUOTA_HISTORY_RECENT_WINDOW_MS,
        };
        let cutoff = now_ms - range_ms;

        retained
            .into_iter()
            .filter(|o| {
                let Some(t) = parse_epoch_ms(&o.observed_at) else {
                    return false;
                };
                if t < cutoff {
                    return false;
                }
                if let Some(pid) = provider_id {
                    if o.provider_id != pid {
                        return false;
                    }
                }
                if let Some(wl) = window_label {
                    if o.window_label != wl {
                        return false;
                    }
                }
                if let Some(expected_account) = account_filter {
                    match expected_account {
                        None => {
                            if o.account.is_some() {
                                return false;
                            }
                        }
                        Some(expected_id) => {
                            if o.account.as_deref() != Some(expected_id) {
                                return false;
                            }
                        }
                    }
                }
                true
            })
            .collect()
    }

    /// Current history revision (broadcast in the runtime snapshot).
    pub fn revision(&self) -> u64 {
        self.state.lock().unwrap().revision
    }

    // ---- mutations ----

    /// Discards the entire history: in-memory state immediately, persisted
    /// file best-effort (an empty envelope replaces it, so a stale file
    /// cannot resurrect cleared history on the next start). Never throws.
    pub fn clear(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.observations.clear();
            state.revision += 1;
        }
        if !self.persist(&[]) {
            let _ = fs::remove_file(&self.path);
        }
    }

    /// One-time import of the legacy localStorage blob. Every entry is
    /// validated with the exact store rules; valid entries merge into the
    /// existing history (dedup makes a retry — or two windows importing
    /// concurrently — a no-op), invalid ones are counted and dropped, and
    /// non-Codex account identities are stored verbatim. All Codex rows are
    /// rejected: the legacy storage source predates the `chatgpt-v2:`
    /// identity form, so it cannot establish account ownership. Import never
    /// blocks startup on malformed data.
    pub fn import_legacy(&self, raw: &Value) -> ImportResult {
        let entries: Vec<&Value> = match raw {
            Value::Array(entries) => entries.iter().collect(),
            Value::Object(object) => object
                .get("observations")
                .and_then(Value::as_array)
                .map(|entries| entries.iter().collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut valid = Vec::new();
        let mut rejected = 0;
        for entry in entries {
            if entry
                .get("providerId")
                .and_then(Value::as_str)
                .is_some_and(is_codex_history_provider)
            {
                rejected += 1;
                continue;
            }
            match normalize_observation(entry) {
                Some(observation) => valid.push(observation),
                None => rejected += 1,
            }
        }

        let now_ms = self.now();
        let (merged, changed) = {
            let mut state = self.state.lock().unwrap();
            let mut all = state.observations.clone();
            all.extend(valid.iter().cloned());
            let merged = apply_retention(all, now_ms);
            let changed = merged != state.observations;
            if changed {
                state.observations = merged.clone();
                state.revision += 1;
            }
            (merged, changed)
        };
        if changed {
            self.persist(&merged);
        }
        ImportResult {
            accepted: valid.len(),
            rejected,
        }
    }

    // ---- persistence ----

    /// Serializes the bounded state and replaces the file atomically-ish:
    /// write to a temp file in the same directory, then rename over the
    /// target (Windows `MoveFileEx` semantics replace the existing file).
    fn persist(&self, observations: &[QuotaObservation]) -> bool {
        let envelope = HistoryEnvelope {
            version: BLOB_VERSION,
            observations: observations.to_vec(),
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

/// Shape of the persisted file: `{ "version": 1, "observations": [...] }` —
/// byte-compatible with the retired TS localStorage blob, so the imported
/// and native histories share one schema.
#[derive(Serialize, Deserialize)]
struct HistoryEnvelope {
    version: u32,
    observations: Vec<QuotaObservation>,
}

// ---------- validation (exact TS `normalizeObservation` parity) ----------

fn epoch_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Parses an RFC-3339 timestamp to epoch milliseconds, or `None`. The TS
/// store accepted anything `Date.parse` could read, but it canonicalized
/// every persisted stamp to `toISOString()` output on write — so every blob
/// this store (or its TS predecessor) ever produced is RFC-3339, and the
/// narrower parser is behaviorally identical for real data.
pub(crate) fn parse_epoch_ms(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc).timestamp_millis())
}

pub(crate) fn canonical_timestamp(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(|| DateTime::<Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

/// Validates a stored/incoming account identity token: a non-blank string
/// of at most 128 UTF-16 code units without the `\u0000` grouping
/// separator. The token is provider-generated (`AccountAttribution.identity`),
/// so this only guards storage hygiene — it never inspects or rewrites the
/// value. Returns the original, untrimmed value.
fn valid_account_identity(value: &str) -> bool {
    if value.trim().is_empty() || value.encode_utf16().count() > 128 {
        return false;
    }
    !value.contains('\u{0}')
}

pub(crate) fn is_codex_history_provider(provider_id: &str) -> bool {
    CODEX_PROVIDER_IDS.contains(&provider_id)
}

/// Codex history may only carry a provider-issued v2 account identity. The
/// fixed prefix and lowercase SHA-256-sized hex body make unattributed,
/// display-label, and retired identity formats distinguishable from a
/// v2-form account partition. This is a format check only; no cryptographic
/// verification is performed.
fn valid_codex_history_identity(account: Option<&str>) -> bool {
    let Some(account) = account else {
        return false;
    };
    let Some(hex) = account.strip_prefix(CODEX_HISTORY_IDENTITY_PREFIX) else {
        return false;
    };
    hex.len() == CODEX_HISTORY_IDENTITY_HEX_LEN
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_history_account(provider_id: &str, account: Option<&str>) -> bool {
    if is_codex_history_provider(provider_id) {
        valid_codex_history_identity(account)
    } else {
        account.is_none_or(valid_account_identity)
    }
}

/// Validates one raw observation (legacy import) and canonicalizes it, or
/// returns `None` when it must not be stored. Rejected: non-object shapes,
/// blank provider or window identifiers, non-finite percentages,
/// unparseable `observedAt`, and a present-but-invalid `account` (an
/// observation whose attribution cannot be trusted must not silently
/// degrade into the unattributed partition). Tolerated (engine parity):
/// out-of-range percentages are clamped, and an unparseable `resetAt`
/// degrades to absent.
fn normalize_observation(raw: &Value) -> Option<QuotaObservation> {
    let record = raw.as_object()?;
    let provider_id = record.get("providerId")?.as_str()?;
    if provider_id.trim().is_empty() {
        return None;
    }
    let window_label = record.get("windowLabel")?.as_str()?;
    if window_label.trim().is_empty() {
        return None;
    }
    let used_percent = record.get("usedPercent")?.as_f64()?;
    if !used_percent.is_finite() {
        return None;
    }
    let observed_at_ms = parse_epoch_ms(record.get("observedAt")?.as_str()?)?;
    let reset_at = record
        .get("resetAt")
        .and_then(Value::as_str)
        .and_then(parse_epoch_ms);
    // A present-but-invalid account rejects the whole observation (a JSON
    // `null` is present, not absent — matching the TS check).
    let account = match record.get("account") {
        None => None,
        Some(raw_account) => {
            let value = raw_account.as_str()?;
            if !valid_account_identity(value) {
                return None;
            }
            Some(value.to_string())
        }
    };
    if !valid_history_account(provider_id, account.as_deref()) {
        return None;
    }
    Some(QuotaObservation {
        provider_id: provider_id.to_string(),
        window_label: window_label.to_string(),
        used_percent: clamp_percent(used_percent),
        observed_at: canonical_timestamp(observed_at_ms),
        reset_at: reset_at.map(canonical_timestamp),
        account,
    })
}

/// Validates one typed candidate observation (runtime recording path) with
/// the same final rules the raw path applies after parsing: blank
/// identifiers, non-finite percentages, unparseable stamps, and invalid
/// accounts reject; retention and clock-skew windows reject; percentages
/// clamp and `resetAt` degrades.
fn validate_canonical(
    mut candidate: QuotaObservation,
    cutoff_ms: i64,
    now_ms: i64,
) -> Option<QuotaObservation> {
    if candidate.provider_id.trim().is_empty() || candidate.window_label.trim().is_empty() {
        return None;
    }
    if !candidate.used_percent.is_finite() {
        return None;
    }
    let observed_at_ms = parse_epoch_ms(&candidate.observed_at)?;
    if let Some(reset_at) = &candidate.reset_at {
        candidate.reset_at = parse_epoch_ms(reset_at).map(canonical_timestamp);
    }
    if !valid_history_account(&candidate.provider_id, candidate.account.as_deref()) {
        return None;
    }
    // Already outside retention, or stamped implausibly far into the future:
    // storing either would only be pruned again on the next pass.
    if observed_at_ms < cutoff_ms || observed_at_ms - now_ms > QUOTA_HISTORY_CLOCK_SKEW_TOLERANCE_MS
    {
        return None;
    }
    candidate.used_percent = clamp_percent(candidate.used_percent);
    candidate.observed_at = canonical_timestamp(observed_at_ms);
    Some(candidate)
}

// ---------- reset detection ----------

/// True when `curr` opens a new quota reset cycle relative to `prev`.
/// Detects two signals:
/// 1. `used_percent` dropped significantly (> 5% drop, matching engine reset detection);
/// 2. `reset_at` moved forward by > 1 minute (or changed presence/value).
pub(crate) fn is_reset_boundary(prev: &QuotaObservation, curr: &QuotaObservation) -> bool {
    if curr.used_percent < prev.used_percent - 5.0 {
        return true;
    }
    match (&prev.reset_at, &curr.reset_at) {
        (Some(prev_r), Some(curr_r)) => {
            if let (Some(prev_ms), Some(curr_ms)) = (parse_epoch_ms(prev_r), parse_epoch_ms(curr_r))
            {
                curr_ms.saturating_sub(prev_ms) > 60_000
            } else {
                prev_r != curr_r
            }
        }
        (None, Some(_)) | (Some(_), None) => true,
        (None, None) => false,
    }
}

// ---------- bounding rules (exact TS `applyRetention` parity) ----------

/// Applies the two-tier bounding rules to one observation list:
/// - drops entries outside 7 days retention (or implausibly far in future);
/// - collapses same-timestamp duplicates per window with newest input winning;
/// - preserves full detail for the recent 24h tier (up to 500 samples);
/// - compacts 24h–7d entries into 30m buckets per reset cycle, preserving
///   discontinuities across quota resets;
/// - sorts by provider, account, window label, and time.
fn apply_retention(observations: Vec<QuotaObservation>, now_ms: i64) -> Vec<QuotaObservation> {
    let cutoff = now_ms - QUOTA_HISTORY_RETENTION_MS;
    let mut windows: HashMap<(String, Option<String>, String), HashMap<i64, QuotaObservation>> =
        HashMap::new();
    for observation in observations {
        let Some(observed_at_ms) = parse_epoch_ms(&observation.observed_at) else {
            continue;
        };
        // Exactly 7 days old is still retained, matching the engine's
        // `maxSampleAgeMs` comparison (`age > max` drops, `age == max` keeps).
        if observed_at_ms < cutoff {
            continue;
        }
        if observed_at_ms - now_ms > QUOTA_HISTORY_CLOCK_SKEW_TOLERANCE_MS {
            continue;
        }
        let key = (
            observation.provider_id.clone(),
            observation.account.clone(),
            observation.window_label.clone(),
        );
        // Later input wins: the same instant recorded twice keeps the newest
        // value, like the TS `Map.set` overwrite.
        windows
            .entry(key)
            .or_default()
            .insert(observed_at_ms, observation);
    }

    let recent_cutoff = now_ms - QUOTA_HISTORY_RECENT_WINDOW_MS;
    let mut result: Vec<QuotaObservation> = Vec::new();
    for by_time in windows.into_values() {
        let mut entries: Vec<QuotaObservation> = by_time.into_values().collect();
        entries.sort_by_key(|o| parse_epoch_ms(&o.observed_at).unwrap_or(0));

        // Assign reset cycle IDs chronologically across the window's full timeline
        // to preserve discontinuities across reset boundaries.
        let mut with_cycle: Vec<(usize, QuotaObservation)> = Vec::with_capacity(entries.len());
        let mut current_cycle = 0usize;
        let mut prev_entry: Option<&QuotaObservation> = None;
        for entry in &entries {
            if let Some(prev) = prev_entry {
                if is_reset_boundary(prev, entry) {
                    current_cycle += 1;
                }
            }
            with_cycle.push((current_cycle, entry.clone()));
            prev_entry = Some(entry);
        }

        // Partition into Tier 1 (recent 24h detailed) and Tier 2 (24h–7d compacted)
        let mut tier1 = Vec::new();
        let mut tier2 = Vec::new();
        for (cycle_id, obs) in with_cycle {
            let t = parse_epoch_ms(&obs.observed_at).unwrap_or(0);
            if t >= recent_cutoff {
                tier1.push(obs);
            } else {
                tier2.push((cycle_id, obs));
            }
        }

        // Tier 1: keep newest detailed samples up to QUOTA_HISTORY_MAX_PER_WINDOW
        let tier1_start = tier1.len().saturating_sub(QUOTA_HISTORY_MAX_PER_WINDOW);
        let kept_tier1 = tier1.into_iter().skip(tier1_start);

        // Tier 2: compact into 30m buckets per reset cycle, keeping latest observation
        let mut bucket_groups: HashMap<(i64, usize), QuotaObservation> = HashMap::new();
        for (cycle_id, obs) in tier2 {
            let t = parse_epoch_ms(&obs.observed_at).unwrap_or(0);
            let bucket_index = t / QUOTA_HISTORY_COMPACT_BUCKET_MS;
            let key = (bucket_index, cycle_id);
            match bucket_groups.entry(key) {
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(obs);
                }
                std::collections::hash_map::Entry::Occupied(mut o) => {
                    let cur_t = parse_epoch_ms(&o.get().observed_at).unwrap_or(0);
                    if t >= cur_t {
                        o.insert(obs);
                    }
                }
            }
        }
        let mut compacted: Vec<QuotaObservation> = bucket_groups.into_values().collect();
        compacted.sort_by_key(|o| parse_epoch_ms(&o.observed_at).unwrap_or(0));
        let tier2_start = compacted
            .len()
            .saturating_sub(QUOTA_HISTORY_MAX_COMPACTED_PER_WINDOW);
        let kept_tier2 = compacted.into_iter().skip(tier2_start);

        result.extend(kept_tier2);
        result.extend(kept_tier1);
    }
    result.sort_by(|a, b| {
        a.provider_id
            .cmp(&b.provider_id)
            .then_with(|| {
                a.account
                    .clone()
                    .unwrap_or_default()
                    .cmp(&b.account.clone().unwrap_or_default())
            })
            .then_with(|| a.window_label.cmp(&b.window_label))
            .then_with(|| {
                parse_epoch_ms(&a.observed_at)
                    .unwrap_or(0)
                    .cmp(&parse_epoch_ms(&b.observed_at).unwrap_or(0))
            })
    });
    result
}

/// Parses the persisted blob. `healthy` reports whether the stored document
/// was fully intact; invalid entries are discarded individually while the
/// valid ones are salvaged. A blob written by a different schema version is
/// discarded whole rather than partially interpreted: history is a
/// disposable cache, and blending an unknown layout into the prediction
/// input risks silent corruption.
fn parse_blob(raw: &str) -> ParsedBlob {
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return ParsedBlob::unhealthy(0);
    };
    let Value::Object(blob) = parsed else {
        return ParsedBlob::unhealthy(0);
    };
    let version = blob.get("version").and_then(Value::as_u64);
    let is_legacy_v1 = version == Some(1);
    let is_current_v2 = version == Some(BLOB_VERSION as u64);
    if !is_legacy_v1 && !is_current_v2 {
        return ParsedBlob::unhealthy(0);
    }
    let Some(entries) = blob.get("observations").and_then(Value::as_array) else {
        return ParsedBlob::unhealthy(0);
    };
    let mut observations = Vec::new();
    let mut healthy = !is_legacy_v1;
    for entry in entries {
        match normalize_observation(entry) {
            Some(observation) => observations.push(observation),
            None => healthy = false,
        }
    }
    ParsedBlob {
        observations,
        total: entries.len(),
        healthy,
    }
}

struct ParsedBlob {
    observations: Vec<QuotaObservation>,
    total: usize,
    healthy: bool,
}

impl ParsedBlob {
    fn unhealthy(total: usize) -> Self {
        Self {
            observations: Vec::new(),
            total,
            healthy: false,
        }
    }
}

// ---------- recording eligibility (exact TS `historyObservationsFromUsages` parity) ----------

/// A parseable RFC-3339 stamp is a usable time; anything else is no stamp at
/// all. The original string is kept — the store canonicalizes on write, and
/// `checkedAt`/`sourceUpdatedAt` never carry unparseable values in practice.
fn valid_time(value: &str) -> Option<String> {
    parse_epoch_ms(value).map(|_| value.to_string())
}

/// Converts one completed cycle's normalized provider snapshot into
/// observation candidates — the exact eligibility rules the retired TS
/// recording path applied (`historyObservationsFromUsages`), expressed in
/// the normalized v0.6 health contract:
///
/// - only live entries are sampled: a genuine successful current fetch.
///   Stale, unknown, cooldown, error, and unavailable entries never enter
///   history (simulated providers cannot occur here — the production
///   registry carries no mock adapters and the runtime snapshot has no
///   simulated flag);
/// - a present freshness verdict requires a parseable `sourceUpdatedAt`;
///   the observation time is the source snapshot time when there is one,
///   otherwise the fetch time;
/// - other providers may store a non-blank identity or history without
///   attribution; Codex requires a v2-form `chatgpt-v2:<64
///   lowercase hex>` identity and contributes nothing without one;
/// - a non-blank window label and a finite percentage are required, while a
///   malformed sibling window is dropped without losing the rest.
pub fn observations_from_usages(usages: &[ProviderUsageDto]) -> Vec<QuotaObservation> {
    let mut observations = Vec::new();
    for usage in usages {
        if usage.id == crate::runtime::ProviderKind::OpenDesign.id() {
            continue;
        }
        if usage.health != crate::runtime::ProviderHealth::Live {
            continue;
        }
        if usage.data_freshness == Some("stale") {
            continue;
        }
        let checked_at = valid_time(&usage.checked_at);
        let source_updated_at = usage.source_updated_at.as_deref().and_then(valid_time);
        if usage.data_freshness.is_some() && source_updated_at.is_none() {
            continue;
        }
        let Some(observed_at) = source_updated_at.or(checked_at) else {
            continue;
        };
        let account = usage
            .account
            .as_ref()
            .and_then(|account| account.identity.as_deref())
            .filter(|identity| !identity.trim().is_empty())
            .map(str::to_string);
        if is_codex_history_provider(&usage.id) && !valid_codex_history_identity(account.as_deref())
        {
            continue;
        }

        for limit in &usage.limits {
            let window_label = limit.label.trim();
            if window_label.is_empty() || !limit.used_percent.is_finite() {
                continue;
            }
            let reset_at = limit.reset_at.as_deref().and_then(valid_time);
            observations.push(QuotaObservation {
                provider_id: usage.id.clone(),
                window_label: window_label.to_string(),
                used_percent: limit.used_percent,
                observed_at: observed_at.clone(),
                reset_at,
                account: account.clone(),
            });
        }
    }
    observations
}

pub(crate) type CodexIdentityBindings = HashMap<String, String>;

/// One configured Codex slot's observed identity state, captured in one
/// runtime snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexSlotFence {
    /// The generation the slot's identity was observed at. It advances on
    /// every observed change of that slot's identity, including
    /// `Some(a) -> None -> Some(a)`, so an identity string is never used
    /// as a substitute for provenance.
    pub(crate) generation: u64,
    /// The observed identity. `None` when the slot exists but no local
    /// login could be proven.
    pub(crate) identity: Option<String>,
}

/// One runtime snapshot's Codex ownership fence: one entry per configured
/// Codex slot.
///
/// The fence is per slot on purpose. The global runtime sequence also moves
/// for unrelated providers — a Grok cycle, a wallet publication, an
/// OpenDesign manual refresh, an account-configuration change — so using it
/// here would discard valid Codex history whenever any other provider
/// happened to be busy. Each slot instead carries the generation its
/// identity was observed at, which is monotonic for that slot alone. An
/// A -> B -> A move therefore still advances the generation twice and is
/// still proven, without borrowing another provider's activity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CodexSnapshotFence {
    slots: BTreeMap<String, CodexSlotFence>,
}

impl CodexSnapshotFence {
    /// Builds the fence from one snapshot's per-slot identity state.
    pub(crate) fn new(states: &[crate::runtime::CodexIdentityStateDto]) -> Self {
        let mut slots = BTreeMap::new();
        for state in states {
            // A malformed identity is treated as no identity at all, so the
            // slot fails closed on its own instead of failing the fence.
            let identity = state
                .identity
                .as_deref()
                .filter(|identity| valid_codex_history_identity(Some(identity)))
                .map(str::to_string);
            slots.insert(
                state.provider_id.clone(),
                CodexSlotFence {
                    generation: state.generation,
                    identity,
                },
            );
        }
        Self { slots }
    }

    /// The verifiable identities this capture proves, per slot. A slot
    /// whose identity is missing or malformed contributes nothing.
    fn bindings(&self) -> CodexIdentityBindings {
        self.slots
            .iter()
            .filter_map(|(id, slot)| slot.identity.clone().map(|identity| (id.clone(), identity)))
            .collect()
    }

    /// The slots this capture observed, with the generation and the
    /// identity the runtime reported for each. A consumer compares them
    /// with its own observation to tell a current projection from one
    /// that belongs to an older session.
    pub(crate) fn slots(&self) -> &BTreeMap<String, CodexSlotFence> {
        &self.slots
    }
}

/// The Codex identities two captures agree on, per slot.
///
/// A slot is included only when BOTH captures observed it at the same
/// generation with the same verifiable identity. A slot whose generation
/// moved between the captures - including an A -> B -> A move, which
/// advances the generation twice - is excluded, and its exclusion never
/// affects the other Codex slots or any unrelated provider.
pub(crate) fn codex_agreed_bindings(
    first: &CodexSnapshotFence,
    settled: &CodexSnapshotFence,
) -> CodexIdentityBindings {
    codex_agreed_bindings_across(&[first, settled])
}

/// The Codex identities every capture in the series agrees on, per slot.
///
/// A slot is proven only when EVERY capture observed it at the same
/// generation with the same verifiable identity. The series is compared
/// pairwise through the same rule the two-capture settlement uses, so a
/// slot that moved at any point in the series is excluded - and only that
/// slot is excluded.
pub(crate) fn codex_agreed_bindings_across(
    fences: &[&CodexSnapshotFence],
) -> CodexIdentityBindings {
    let mut agreed = CodexIdentityBindings::new();
    let Some((first, rest)) = fences.split_first() else {
        return agreed;
    };
    for (id, first_slot) in &first.slots {
        let Some(first_identity) = first_slot.identity.as_deref() else {
            continue;
        };
        let generation = first_slot.generation;
        let proven = rest.iter().all(|fence| {
            fence.slots.get(id).is_some_and(|slot| {
                slot.generation == generation && slot.identity.as_deref() == Some(first_identity)
            })
        });
        if proven {
            agreed.insert(id.clone(), first_identity.to_string());
        }
    }
    agreed
}

/// IPC-facing isolation: Codex rows survive only when they belong to the
/// supplied binding. A slot the bindings do not prove discards only its own
/// rows - including rows whose account looks valid - because no trustworthy
/// binding is available for that slot. Other Codex slots and every
/// unrelated provider retain their existing account semantics. The store
/// itself deliberately keeps its all-account query for Lane B; this filter
/// belongs at the read/export boundary.
fn isolate_codex_accounts(
    history: Vec<QuotaObservation>,
    current: &CodexIdentityBindings,
) -> Vec<QuotaObservation> {
    history
        .into_iter()
        .filter(|observation| {
            if !is_codex_history_provider(&observation.provider_id) {
                return true;
            }
            let account = observation.account.as_deref();
            valid_codex_history_identity(account)
                && current
                    .get(&observation.provider_id)
                    .is_some_and(|identity| account == Some(identity.as_str()))
        })
        .collect()
}

/// Filters already-prepared history against one binding. A caller that
/// brackets output preparation with two snapshots should filter its inputs
/// first and call [`settle_codex_accounts`] immediately before returning.
pub(crate) fn isolate_codex_accounts_for_bindings(
    history: Vec<QuotaObservation>,
    bindings: Option<&CodexIdentityBindings>,
) -> Vec<QuotaObservation> {
    match bindings {
        Some(bindings) => isolate_codex_accounts(history, bindings),
        None => history
            .into_iter()
            .filter(|observation| !is_codex_history_provider(&observation.provider_id))
            .collect(),
    }
}

/// Filters inputs against the bindings from one capture.
pub(crate) fn isolate_codex_accounts_for_fence(
    history: Vec<QuotaObservation>,
    fence: &CodexSnapshotFence,
) -> Vec<QuotaObservation> {
    isolate_codex_accounts(history, &fence.bindings())
}

/// Applies the conservative two-capture settlement, per Codex slot.
///
/// Only a slot whose ownership both captures agree on may keep its rows. A
/// slot whose generation or identity moved - including an A -> B -> A move
/// - discards only its own rows. Every unrelated provider is retained, and
/// one slot's disagreement never suppresses another slot.
pub(crate) fn settle_codex_accounts_for_fences(
    history: Vec<QuotaObservation>,
    first: &CodexSnapshotFence,
    settled: &CodexSnapshotFence,
) -> Vec<QuotaObservation> {
    isolate_codex_accounts(history, &codex_agreed_bindings(first, settled))
}

/// Single-snapshot compatibility wrapper for recording-side callers and
/// focused tests. IPC read/export paths use the two-snapshot settlement.
#[cfg(test)]
pub(crate) fn isolate_current_codex_accounts(
    history: Vec<QuotaObservation>,
    providers: &[ProviderUsageDto],
) -> Vec<QuotaObservation> {
    isolate_codex_accounts(history, &provider_codex_identities(providers))
}

/// Reads the v2-form Codex identities from one set of provider entries, per
/// slot. A slot whose identity is missing or malformed contributes nothing,
/// so that slot fails closed on its own and never suppresses the others.
#[cfg(test)]
fn provider_codex_identities(providers: &[ProviderUsageDto]) -> CodexIdentityBindings {
    let mut identities = CodexIdentityBindings::new();
    for usage in providers
        .iter()
        .filter(|usage| is_codex_history_provider(&usage.id))
    {
        let identity = usage
            .account
            .as_ref()
            .and_then(|account| account.identity.as_deref())
            .filter(|identity| valid_codex_history_identity(Some(identity)));
        if let Some(identity) = identity {
            identities.insert(usage.id.clone(), identity.to_string());
        }
    }
    identities
}

// ---------- tauri commands ----------

#[tauri::command]
pub fn get_history(handle: tauri::State<RuntimeHandle>) -> Vec<QuotaObservation> {
    match handle.history_store() {
        Some(store) => {
            let first = handle.snapshot();
            let first = CodexSnapshotFence::new(&first.codex_identity_states);
            let rows = isolate_codex_accounts_for_fence(store.history(), &first);
            let settled = handle.snapshot();
            let settled = CodexSnapshotFence::new(&settled.codex_identity_states);
            settle_codex_accounts_for_fences(rows, &first, &settled)
        }
        None => Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRangeQuery {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub window_label: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    /// When true, `account: None` specifically matches unattributed entries.
    /// When false (default), `account: None` matches any account.
    #[serde(default)]
    pub exact_account: bool,
}

#[tauri::command]
pub fn get_history_range(
    handle: tauri::State<RuntimeHandle>,
    provider_id: Option<String>,
    account: Option<String>,
    window_label: Option<String>,
    range: Option<String>,
    exact_account: Option<bool>,
    query: Option<HistoryRangeQuery>,
) -> Vec<QuotaObservation> {
    let q = query.unwrap_or_else(|| HistoryRangeQuery {
        provider_id,
        account,
        window_label,
        range,
        exact_account: exact_account.unwrap_or(false),
    });
    match handle.history_store() {
        Some(store) => {
            let account_filter = if q.exact_account {
                Some(q.account.as_deref())
            } else {
                q.account.as_deref().map(Some)
            };
            let first = handle.snapshot();
            let first = CodexSnapshotFence::new(&first.codex_identity_states);
            let rows = store.history_range(
                q.provider_id.as_deref(),
                account_filter,
                q.window_label.as_deref(),
                q.range.as_deref().unwrap_or("24h"),
            );
            let rows = isolate_codex_accounts_for_fence(rows, &first);
            let settled = handle.snapshot();
            let settled = CodexSnapshotFence::new(&settled.codex_identity_states);
            settle_codex_accounts_for_fences(rows, &first, &settled)
        }
        None => Vec::new(),
    }
}

#[tauri::command]
pub fn clear_history(handle: tauri::State<RuntimeHandle>) {
    // Lane B: clearing the history also drops held suspicious-drop
    // candidates — no gated window may outlive the history it was gated
    // against. The next cycle classifies from scratch.
    handle.clear_pending_confirmations();
    if let Some(store) = handle.history_store() {
        store.clear();
    }
}

#[tauri::command]
pub fn import_legacy_history(
    handle: tauri::State<RuntimeHandle>,
    observations: Value,
) -> ImportResult {
    match handle.history_store() {
        Some(store) => store.import_legacy(&observations),
        None => ImportResult {
            accepted: 0,
            rejected: 0,
        },
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{AccountAttributionDto, UsageLimitDto};
    use chrono::TimeZone;

    const NOW_MS: i64 = 1_790_596_800_000; // == Date.parse("2026-09-28T12:00:00.000Z")
    const CHECKED_AT: &str = "2026-09-28T12:00:00.000Z";

    fn fixed_clock() -> Box<dyn Fn() -> i64 + Send + Sync> {
        Box::new(|| NOW_MS)
    }

    fn temp_store(tag: &str) -> (QuotaHistoryStore, tempdir::TempDir) {
        let dir = tempdir::temp_dir(tag);
        let store =
            QuotaHistoryStore::open_with_clock(dir.path().join(HISTORY_FILE_NAME), fixed_clock());
        (store, dir)
    }

    /// Minimal temp-dir helper (no new dependencies): a unique directory
    /// under the cargo target temp root, removed on drop.
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
                "rate-limits-history-test-{}-{}-{}",
                tag,
                std::process::id(),
                unique
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }
    }

    fn observation(
        provider: &str,
        label: &str,
        percent: f64,
        observed_at: &str,
    ) -> QuotaObservation {
        QuotaObservation {
            provider_id: provider.to_string(),
            window_label: label.to_string(),
            used_percent: percent,
            observed_at: observed_at.to_string(),
            reset_at: None,
            account: None,
        }
    }

    fn codex_identity(character: char) -> String {
        assert!(character.is_ascii_hexdigit() && !character.is_ascii_uppercase());
        format!("chatgpt-v2:{}", character.to_string().repeat(64))
    }

    fn usage_with_identity(id: &str, identity: Option<&str>) -> ProviderUsageDto {
        let mut usage = ok_usage(id, "OpenAI / Codex", 10.0);
        usage.account = identity.map(|identity| AccountAttributionDto {
            label: "current Codex account".to_string(),
            note: None,
            identity: Some(identity.to_string()),
        });
        usage
    }

    fn at(minute_offset: i64) -> String {
        canonical_timestamp(NOW_MS - minute_offset * 60_000)
    }

    // 1. valid observation recording
    #[test]
    fn records_valid_observations_and_canonicalizes_them() {
        let (store, _dir) = temp_store("record-valid");
        let mut candidate =
            observation("opencode-go", "5-hour", 142.0, "2026-09-28T12:00:00+00:00");
        candidate.reset_at = Some("2026-09-28T13:00:00.000Z".to_string());
        candidate.account = Some("key:3456".to_string());
        let result = store.record(vec![candidate]);
        assert_eq!(
            result,
            RecordResult {
                accepted: 1,
                rejected: 0,
                persisted: true
            }
        );
        let history = store.history();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].used_percent, 100.0,
            "out-of-range percent clamps"
        );
        assert_eq!(
            history[0].observed_at, CHECKED_AT,
            "timestamps canonicalize to ISO-8601 UTC millis"
        );
        assert_eq!(
            history[0].reset_at.as_deref(),
            Some("2026-09-28T13:00:00.000Z")
        );
        assert_eq!(history[0].account.as_deref(), Some("key:3456"));
    }

    // 2. invalid windows ignored / write policy
    #[test]
    fn rejects_invalid_observations_while_accepting_the_valid_rest() {
        let (store, _dir) = temp_store("record-invalid");
        let blank_provider = observation("", "5-hour", 10.0, CHECKED_AT);
        let blank_window = observation("zai", "   ", 10.0, CHECKED_AT);
        let bad_account = observation("zai", "5-hour", 10.0, CHECKED_AT);
        let bad_account = QuotaObservation {
            account: Some("  ".to_string()),
            ..bad_account
        };
        let over_skew = observation("zai", "5-hour", 10.0, "2026-09-28T12:10:00.000Z");
        let outside_retention = observation("zai", "5-hour", 10.0, "2026-09-20T11:00:00.000Z");
        let good = observation("zai", "5-hour", 10.0, CHECKED_AT);
        let result = store.record(vec![
            blank_provider,
            blank_window,
            bad_account,
            over_skew,
            outside_retention,
            good.clone(),
        ]);
        assert_eq!(result.accepted, 1);
        assert_eq!(result.rejected, 5);
        assert_eq!(store.history(), vec![good]);
    }

    #[test]
    fn accepts_future_stamps_within_the_clock_skew_tolerance() {
        let (store, _dir) = temp_store("record-skew");
        let edge = observation("zai", "5-hour", 10.0, "2026-09-28T12:05:00.000Z");
        let result = store.record(vec![edge.clone()]);
        assert_eq!(result.accepted, 1, "exactly +5 minutes is tolerated");
        assert_eq!(store.history(), vec![edge]);
    }

    // 3. provider/account/window identity preserved
    #[test]
    fn preserves_provider_account_and_window_identity() {
        let (store, _dir) = temp_store("identity");
        let mut entry = observation("grok", "7-day", 42.5, CHECKED_AT);
        entry.reset_at = Some("2026-10-01T00:00:00.000Z".to_string());
        entry.account = Some("xai:acct12345".to_string());
        store.record(vec![entry]);
        let history = store.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].provider_id, "grok");
        assert_eq!(history[0].window_label, "7-day");
        assert_eq!(history[0].account.as_deref(), Some("xai:acct12345"));
        assert_eq!(
            history[0].reset_at.as_deref(),
            Some("2026-10-01T00:00:00.000Z")
        );
        assert_eq!(history[0].used_percent, 42.5);
    }

    // 4. account A and B remain isolated
    #[test]
    fn accounts_of_one_provider_never_share_dedup_or_bounds() {
        let (store, _dir) = temp_store("account-isolation");
        let same_instant = CHECKED_AT;
        let mut a = observation("opencode-go", "5-hour", 10.0, same_instant);
        a.account = Some("key:aaaa".to_string());
        let mut b = observation("opencode-go", "5-hour", 60.0, same_instant);
        b.account = Some("key:bbbb".to_string());
        // Same provider, label, and instant — only the account differs. A
        // shared dedup key would collapse these to one entry.
        store.record(vec![a.clone(), b.clone()]);
        let history = store.history();
        assert_eq!(
            history.len(),
            2,
            "distinct accounts are distinct logical windows"
        );
        assert_eq!(history[0].account.as_deref(), Some("key:aaaa"));
        assert_eq!(history[0].used_percent, 10.0);
        assert_eq!(history[1].account.as_deref(), Some("key:bbbb"));
        assert_eq!(history[1].used_percent, 60.0);

        // Bounds are per account too: flooding A cannot evict B's sample.
        for index in 0..(QUOTA_HISTORY_MAX_PER_WINDOW as i64) {
            let mut flood = observation(
                "opencode-go",
                "5-hour",
                1.0,
                &canonical_timestamp(NOW_MS - 600_000 + index),
            );
            flood.account = Some("key:aaaa".to_string());
            store.record(vec![flood]);
        }
        let history = store.history();
        let a_entries = history
            .iter()
            .filter(|entry| entry.account.as_deref() == Some("key:aaaa"))
            .count();
        let b_entries = history
            .iter()
            .filter(|entry| entry.account.as_deref() == Some("key:bbbb"))
            .count();
        assert_eq!(a_entries, QUOTA_HISTORY_MAX_PER_WINDOW);
        assert_eq!(
            b_entries, 1,
            "account B's history survives account A's flood"
        );
    }

    // 5. unattributed legacy history does not attach to an attributed account
    #[test]
    fn unattributed_entries_stay_unattributed_and_separate() {
        let (store, _dir) = temp_store("unattributed");
        store.import_legacy(
            &serde_json::to_value(vec![serde_json::json!({
                "providerId": "opencode-go",
                "windowLabel": "5-hour",
                "usedPercent": 20.0,
                "observedAt": CHECKED_AT
            })])
            .unwrap(),
        );
        let mut attributed = observation("opencode-go", "5-hour", 30.0, &at(5));
        attributed.account = Some("key:3456".to_string());
        store.record(vec![attributed]);

        let history = store.history();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[0].account, None,
            "imported unattributed entry is stored verbatim, never attached to an account"
        );
        assert_eq!(history[1].account.as_deref(), Some("key:3456"));
    }

    // 6. duplicate observation dedupes (newest input wins)
    #[test]
    fn same_timestamp_duplicates_collapse_with_newest_input_winning() {
        let (store, _dir) = temp_store("dedup");
        let first = observation("zai", "5-hour", 40.0, CHECKED_AT);
        let second = observation("zai", "5-hour", 41.0, CHECKED_AT);
        let result = store.record(vec![first, second]);
        assert_eq!(result.accepted, 2);
        let history = store.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].used_percent, 41.0);

        // A repeated identical snapshot (provider cache re-served) adds nothing.
        store.record(vec![observation("zai", "5-hour", 41.0, CHECKED_AT)]);
        assert_eq!(store.history().len(), 1);
    }

    // 7. retention window pruning
    #[test]
    fn prunes_older_than_retention_but_keeps_exactly_24_hours() {
        let (store, _dir) = temp_store("retention");
        let edge = observation("zai", "5-hour", 10.0, "2026-09-27T12:00:00.000Z");
        let older = observation("zai", "5-hour", 20.0, "2026-09-27T11:59:59.999Z");
        let fresh = observation("zai", "5-hour", 30.0, CHECKED_AT);
        store.record(vec![edge.clone(), older, fresh.clone()]);
        let history = store.history();
        assert_eq!(
            history.len(),
            2,
            "age == 24h keeps (engine parity), age > 24h drops"
        );
        assert_eq!(history[0], edge);
        assert_eq!(history[1], fresh);
    }

    // 8. max sample pruning
    #[test]
    fn keeps_only_the_newest_500_per_logical_window() {
        let (store, _dir) = temp_store("max-samples");
        let total = QUOTA_HISTORY_MAX_PER_WINDOW + 25;
        let candidates: Vec<QuotaObservation> = (0..total)
            .map(|index| {
                observation(
                    "zai",
                    "5-hour",
                    index as f64,
                    &canonical_timestamp(NOW_MS - (total - index) as i64),
                )
            })
            .collect();
        let result = store.record(candidates);
        assert_eq!(result.accepted, total as usize);
        let history = store.history();
        assert_eq!(history.len(), QUOTA_HISTORY_MAX_PER_WINDOW);
        // Newest samples survive; the oldest 25 are evicted.
        assert_eq!(history[0].observed_at, canonical_timestamp(NOW_MS - 500));
        assert_eq!(
            history.last().unwrap().observed_at,
            canonical_timestamp(NOW_MS - 1)
        );
    }

    #[test]
    fn bounds_apply_per_window_for_shared_labels() {
        let (store, _dir) = temp_store("shared-labels");
        let codex = codex_identity('f');
        for provider in ["openai-codex", "zai"] {
            let candidates: Vec<QuotaObservation> = (0..QUOTA_HISTORY_MAX_PER_WINDOW + 5)
                .map(|index| {
                    let mut entry = observation(
                        provider,
                        "5-hour",
                        index as f64,
                        &canonical_timestamp(
                            NOW_MS - (QUOTA_HISTORY_MAX_PER_WINDOW + 5 - index) as i64,
                        ),
                    );
                    if provider == "openai-codex" {
                        entry.account = Some(codex.clone());
                    }
                    entry
                })
                .collect();
            store.record(candidates);
        }
        let history = store.history();
        assert_eq!(history.len(), 2 * QUOTA_HISTORY_MAX_PER_WINDOW);
        assert_eq!(
            history
                .iter()
                .filter(|o| o.provider_id == "openai-codex")
                .count(),
            QUOTA_HISTORY_MAX_PER_WINDOW
        );
    }

    // 9. corrupt history file recovers safely
    #[test]
    fn corrupt_file_degrades_to_empty_and_the_app_continues() {
        let dir = tempdir::temp_dir("corrupt");
        let path = dir.path().join(HISTORY_FILE_NAME);
        std::fs::write(&path, "{not json at all").unwrap();
        let store = QuotaHistoryStore::open_with_clock(path.clone(), fixed_clock());
        assert!(store.history().is_empty(), "corrupt blob reads as empty");
        // The store still records and persists afterwards.
        let good = observation("zai", "5-hour", 10.0, CHECKED_AT);
        store.record(vec![good.clone()]);
        assert_eq!(store.history(), vec![good]);
        // Reopening sees the healed state, not the corruption.
        drop(store);
        let reopened = QuotaHistoryStore::open_with_clock(path, fixed_clock());
        assert_eq!(reopened.history().len(), 1);
    }

    // 10. mixed valid/invalid file self-heals
    #[test]
    fn mixed_valid_and_invalid_file_salvages_the_valid_subset() {
        let dir = tempdir::temp_dir("mixed");
        let path = dir.path().join(HISTORY_FILE_NAME);
        let blob = serde_json::json!({
            "version": 1,
            "observations": [
                { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": CHECKED_AT },
                { "providerId": "", "windowLabel": "5-hour", "usedPercent": 10.0, "observedAt": CHECKED_AT },
                { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": "high", "observedAt": CHECKED_AT },
                { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 40.0, "observedAt": "nope" },
                {
                    "providerId": "zai",
                    "windowLabel": "5-hour",
                    "usedPercent": 50.0,
                    "observedAt": CHECKED_AT,
                    // Built at runtime so the value really carries the \u0000
                    // grouping separator (a literal JSON escape would be a
                    // plain backslash sequence, not the control character).
                    "account": format!("key:{}", '\u{0000}')
                }
            ]
        });
        std::fs::write(&path, blob.to_string()).unwrap();
        let store = QuotaHistoryStore::open_with_clock(path.clone(), fixed_clock());
        let history = store.history();
        assert_eq!(
            history.len(),
            1,
            "valid entries are salvaged, invalid dropped"
        );
        assert_eq!(history[0].used_percent, 30.0);
        // The sanitized state was written back (self-heal): only the valid
        // entry survives on disk.
        drop(store);
        let reopened = QuotaHistoryStore::open_with_clock(path, fixed_clock());
        assert_eq!(reopened.history().len(), 1);
    }

    #[test]
    fn foreign_version_blob_is_discarded_whole() {
        let dir = tempdir::temp_dir("version");
        let path = dir.path().join(HISTORY_FILE_NAME);
        let blob = serde_json::json!({
            "version": 99,
            "observations": [
                { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": CHECKED_AT }
            ]
        });
        std::fs::write(&path, blob.to_string()).unwrap();
        let store = QuotaHistoryStore::open_with_clock(path, fixed_clock());
        assert!(
            store.history().is_empty(),
            "unknown layout must not blend into predictions"
        );
    }

    // 11. clear_history clears memory + persisted data
    #[test]
    fn clear_empties_memory_and_persistence() {
        let (store, dir) = temp_store("clear");
        store.record(vec![observation("zai", "5-hour", 10.0, CHECKED_AT)]);
        assert!(!store.history().is_empty());
        store.clear();
        assert!(store.history().is_empty());
        // The persisted file cannot resurrect the cleared history: an empty
        // envelope replaced it (or the file is gone).
        let path = dir.path().join(HISTORY_FILE_NAME);
        if let Ok(raw) = std::fs::read_to_string(&path) {
            let blob: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(blob["observations"].as_array().map(Vec::len), Some(0));
        }
        drop(store);
        let reopened = QuotaHistoryStore::open_with_clock(path, fixed_clock());
        assert!(
            reopened.history().is_empty(),
            "cleared state survives restart"
        );
    }

    // 12. import_legacy_history is idempotent
    #[test]
    fn legacy_import_is_idempotent() {
        let (store, _dir) = temp_store("import-idempotent");
        let blob = serde_json::json!([
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": CHECKED_AT },
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 31.0, "observedAt": "2026-09-28T11:55:00.000Z" }
        ]);
        let first = store.import_legacy(&blob);
        assert_eq!(first.accepted, 2);
        let after_first = store.history();
        let second = store.import_legacy(&blob);
        assert_eq!(second.accepted, 2, "entries re-validate on retry");
        assert_eq!(store.history(), after_first, "re-import adds no duplicates");
    }

    // 13. legacy import merges without duplicates
    #[test]
    fn legacy_import_merges_with_existing_history_without_duplicates() {
        let (store, _dir) = temp_store("import-merge");
        store.record(vec![observation("zai", "5-hour", 30.0, CHECKED_AT)]);
        let blob = serde_json::json!([
            // Same window+instant as the recorded entry: collapses, newest
            // input wins.
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 35.0, "observedAt": CHECKED_AT },
            // Genuinely new entry: merges.
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 20.0, "observedAt": "2026-09-28T11:50:00.000Z" }
        ]);
        store.import_legacy(&blob);
        let history = store.history();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[1].used_percent, 35.0,
            "imported value replaces the recorded same-instant one"
        );
    }

    // 14. legacy invalid entries are ignored
    #[test]
    fn legacy_import_ignores_invalid_entries_and_malformed_blobs() {
        let (store, _dir) = temp_store("import-invalid");
        let blob = serde_json::json!([
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": CHECKED_AT, "resetAt": "garbage" },
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": CHECKED_AT, "account": 42 },
            "not an object",
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": null, "observedAt": CHECKED_AT }
        ]);
        let result = store.import_legacy(&blob);
        assert_eq!(
            result.accepted, 1,
            "unparseable resetAt degrades, valid entry survives"
        );
        assert_eq!(result.rejected, 3);
        assert_eq!(store.history().len(), 1);

        // Malformed blobs (wrong shapes entirely) import nothing and do not
        // block anything.
        assert_eq!(
            store.import_legacy(&serde_json::json!("garbage")).accepted,
            0
        );
        assert_eq!(
            store
                .import_legacy(&serde_json::json!({ "version": 2 }))
                .accepted,
            0
        );
        assert_eq!(store.import_legacy(&serde_json::json!(null)).accepted, 0);
    }

    #[test]
    fn legacy_import_never_reshapes_account_identities() {
        let (store, _dir) = temp_store("import-account");
        let blob = serde_json::json!([
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": CHECKED_AT, "account": "key:3456" },
            { "providerId": "grok", "windowLabel": "7-day", "usedPercent": 12.0, "observedAt": CHECKED_AT, "account": "xai:acct" }
        ]);
        store.import_legacy(&blob);
        let history = store.history();
        let zai = history.iter().find(|o| o.provider_id == "zai").unwrap();
        let grok = history.iter().find(|o| o.provider_id == "grok").unwrap();
        assert_eq!(zai.account.as_deref(), Some("key:3456"));
        assert_eq!(grok.account.as_deref(), Some("xai:acct"));
    }

    #[test]
    fn codex_history_requires_a_v2_form_identity_on_record() {
        let (store, _dir) = temp_store("codex-record-identity");
        let primary = codex_identity('a');
        let secondary = codex_identity('b');
        let check_codex = |provider: &str, account: Option<&str>| {
            let mut entry = observation(provider, "5-hour", 10.0, CHECKED_AT);
            entry.account = account.map(str::to_string);
            entry
        };
        let zai_unattributed = observation("zai", "5-hour", 20.0, CHECKED_AT);
        let mut zai_attributed = observation("zai", "weekly", 30.0, CHECKED_AT);
        zai_attributed.account = Some("key:3456".to_string());
        let valid_primary = check_codex("openai-codex", Some(&primary));
        let valid_secondary = check_codex("openai-codex:secondary", Some(&secondary));

        let result = store.record(vec![
            valid_primary,
            valid_secondary,
            check_codex("openai-codex", Some("chatgpt:legacy")),
            check_codex("openai-codex", None),
            check_codex(
                "openai-codex",
                Some(&format!("chatgpt-v2:{}", "A".repeat(64))),
            ),
            check_codex("openai-codex:secondary", Some("chatgpt-v2:short")),
            zai_unattributed,
            zai_attributed,
        ]);

        assert_eq!(result.accepted, 4);
        assert_eq!(result.rejected, 4);
        let history = store.history();
        let codex: Vec<_> = history
            .iter()
            .filter(|entry| is_codex_history_provider(&entry.provider_id))
            .collect();
        assert_eq!(codex.len(), 2);
        assert!(codex
            .iter()
            .all(|entry| { valid_codex_history_identity(entry.account.as_deref()) }));
        let zai: Vec<_> = history
            .iter()
            .filter(|entry| entry.provider_id == "zai")
            .collect();
        assert_eq!(
            zai.len(),
            2,
            "other providers keep attributed and unattributed rows"
        );
        assert!(zai.iter().any(|entry| entry.account.is_none()));
        assert!(zai
            .iter()
            .any(|entry| entry.account.as_deref() == Some("key:3456")));
    }

    #[test]
    fn legacy_import_rejects_legacy_codex_rows_and_preserves_other_providers() {
        let (store, _dir) = temp_store("codex-import-identity");
        let primary = codex_identity('c');
        let blob = serde_json::json!([
            { "providerId": "openai-codex", "windowLabel": "5-hour", "usedPercent": 10.0, "observedAt": CHECKED_AT, "account": primary },
            { "providerId": "openai-codex", "windowLabel": "weekly", "usedPercent": 20.0, "observedAt": CHECKED_AT, "account": "chatgpt:legacy" },
            { "providerId": "openai-codex", "windowLabel": "weekly", "usedPercent": 30.0, "observedAt": CHECKED_AT },
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 40.0, "observedAt": CHECKED_AT },
            { "providerId": "zai", "windowLabel": "weekly", "usedPercent": 50.0, "observedAt": CHECKED_AT, "account": "key:3456" }
        ]);

        let result = store.import_legacy(&blob);
        assert_eq!(
            result,
            ImportResult {
                accepted: 2,
                rejected: 3
            }
        );
        let history = store.history();
        assert_eq!(
            history
                .iter()
                .filter(|entry| entry.provider_id == "openai-codex")
                .count(),
            0
        );
        assert_eq!(
            history
                .iter()
                .filter(|entry| entry.provider_id == "zai")
                .count(),
            2
        );
    }

    #[test]
    fn legacy_import_rejects_v2_form_codex_rows() {
        let (store, _dir) = temp_store("codex-import-v2-form");
        let codex_identity = codex_identity('f');
        let blob = serde_json::json!([
            { "providerId": "openai-codex", "windowLabel": "5-hour", "usedPercent": 10.0, "observedAt": CHECKED_AT, "account": codex_identity },
            { "providerId": "openai-codex:secondary", "windowLabel": "weekly", "usedPercent": 20.0, "observedAt": CHECKED_AT, "account": codex_identity },
            { "providerId": "opencode-go", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": CHECKED_AT, "account": "key:3456" }
        ]);

        let result = store.import_legacy(&blob);
        assert_eq!(
            result,
            ImportResult {
                accepted: 1,
                rejected: 2
            }
        );
        let history = store.history();
        assert!(!history
            .iter()
            .any(|entry| is_codex_history_provider(&entry.provider_id)));
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].provider_id, "opencode-go");
        assert_eq!(history[0].account.as_deref(), Some("key:3456"));
    }

    #[test]
    fn load_migrates_out_legacy_codex_rows_and_rewrites_the_sanitized_file() {
        let dir = tempdir::temp_dir("codex-load-migration");
        let path = dir.path().join(HISTORY_FILE_NAME);
        let primary = codex_identity('d');
        let secondary = codex_identity('e');
        let blob = serde_json::json!({
            "version": BLOB_VERSION,
            "observations": [
                { "providerId": "openai-codex", "windowLabel": "5-hour", "usedPercent": 10.0, "observedAt": CHECKED_AT, "account": primary },
                { "providerId": "openai-codex", "windowLabel": "weekly", "usedPercent": 20.0, "observedAt": CHECKED_AT, "account": "chatgpt:legacy" },
                { "providerId": "openai-codex", "windowLabel": "weekly", "usedPercent": 30.0, "observedAt": CHECKED_AT },
                { "providerId": "openai-codex:secondary", "windowLabel": "5-hour", "usedPercent": 40.0, "observedAt": CHECKED_AT, "account": secondary },
                { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 50.0, "observedAt": CHECKED_AT },
                { "providerId": "zai", "windowLabel": "weekly", "usedPercent": 60.0, "observedAt": CHECKED_AT, "account": "key:3456" }
            ]
        });
        std::fs::write(&path, blob.to_string()).unwrap();

        let store = QuotaHistoryStore::open_with_clock(path.clone(), fixed_clock());
        let history = store.history();
        assert_eq!(
            history
                .iter()
                .filter(|entry| entry.provider_id == "openai-codex")
                .count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .filter(|entry| entry.provider_id == "openai-codex:secondary")
                .count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .filter(|entry| entry.provider_id == "zai")
                .count(),
            2
        );
        assert!(history
            .iter()
            .filter(|entry| is_codex_history_provider(&entry.provider_id))
            .all(|entry| valid_codex_history_identity(entry.account.as_deref())));

        let healed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let observations = healed["observations"].as_array().unwrap();
        assert_eq!(
            observations.len(),
            4,
            "legacy Codex rows are removed from disk"
        );
        assert!(observations
            .iter()
            .filter(|entry| {
                entry["providerId"]
                    .as_str()
                    .is_some_and(is_codex_history_provider)
            })
            .all(|entry| entry["account"]
                .as_str()
                .is_some_and(|identity| { valid_codex_history_identity(Some(identity)) })));
    }

    #[test]
    fn command_read_isolation_keeps_only_the_current_v2_form_codex_accounts() {
        let primary = codex_identity('1');
        let other_primary = codex_identity('2');
        let secondary = codex_identity('3');
        let mut current_primary = observation("openai-codex", "5-hour", 10.0, CHECKED_AT);
        current_primary.account = Some(primary.clone());
        let mut old_primary = observation("openai-codex", "5-hour", 20.0, CHECKED_AT);
        old_primary.account = Some(other_primary);
        let mut legacy_primary = observation("openai-codex", "weekly", 30.0, CHECKED_AT);
        legacy_primary.account = None;
        let mut current_secondary =
            observation("openai-codex:secondary", "weekly", 40.0, CHECKED_AT);
        current_secondary.account = Some(secondary.clone());
        let mut unattributed_secondary =
            observation("openai-codex:secondary", "5-hour", 50.0, CHECKED_AT);
        unattributed_secondary.account = None;
        let zai_unattributed = observation("zai", "5-hour", 60.0, CHECKED_AT);
        let mut zai_attributed = observation("zai", "weekly", 70.0, CHECKED_AT);
        zai_attributed.account = Some("key:3456".to_string());
        let stored = vec![
            current_primary,
            old_primary,
            legacy_primary,
            current_secondary,
            unattributed_secondary,
            zai_unattributed,
            zai_attributed,
        ];

        let providers = vec![
            usage_with_identity("openai-codex", Some(&primary)),
            usage_with_identity("openai-codex:secondary", Some(&secondary)),
            usage_with_identity("zai", None),
        ];
        let isolated = isolate_current_codex_accounts(stored, &providers);
        assert_eq!(
            isolated
                .iter()
                .filter(|entry| entry.provider_id == "openai-codex")
                .count(),
            1
        );
        assert_eq!(
            isolated
                .iter()
                .filter(|entry| entry.provider_id == "openai-codex:secondary")
                .count(),
            1
        );
        assert_eq!(
            isolated
                .iter()
                .filter(|entry| entry.provider_id == "zai")
                .count(),
            2
        );
        assert!(isolated
            .iter()
            .filter(|entry| entry.provider_id == "openai-codex")
            .all(|entry| entry.account.as_deref() == Some(primary.as_str())));
        assert!(isolated
            .iter()
            .filter(|entry| entry.provider_id == "openai-codex:secondary")
            .all(|entry| entry.account.as_deref() == Some(secondary.as_str())));

        let invalid_snapshot = vec![
            usage_with_identity("openai-codex", None),
            usage_with_identity("openai-codex:secondary", Some("chatgpt:legacy")),
        ];
        let fail_closed = isolate_current_codex_accounts(isolated, &invalid_snapshot);
        assert!(!fail_closed
            .iter()
            .any(|entry| is_codex_history_provider(&entry.provider_id)));
        assert_eq!(
            fail_closed
                .iter()
                .filter(|entry| entry.provider_id == "zai")
                .count(),
            2
        );
    }

    /// One observed Codex slot, as the runtime publishes it.
    fn codex_state(
        provider_id: &str,
        generation: u64,
        identity: Option<&str>,
    ) -> crate::runtime::CodexIdentityStateDto {
        crate::runtime::CodexIdentityStateDto {
            provider_id: provider_id.to_string(),
            generation,
            identity: identity.map(str::to_string),
        }
    }

    #[test]
    fn two_capture_settlement_fails_closed_on_change_or_unverifiable_identity() {
        let first_identity = codex_identity('4');
        let second_identity = codex_identity('5');
        let mut codex = observation("openai-codex", "5-hour", 10.0, CHECKED_AT);
        codex.account = Some(first_identity.clone());
        let zai = observation("zai", "5-hour", 20.0, CHECKED_AT);

        let first = CodexSnapshotFence::new(&[codex_state("openai-codex", 7, Some(&first_identity))]);
        let same = CodexSnapshotFence::new(&[codex_state("openai-codex", 7, Some(&first_identity))]);
        // The identity moved but the generation did not: impossible in the
        // runtime, and the fence must still reject it.
        let changed =
            CodexSnapshotFence::new(&[codex_state("openai-codex", 7, Some(&second_identity))]);
        // The identity is back but the generation moved twice (A -> B -> A).
        let moved_back =
            CodexSnapshotFence::new(&[codex_state("openai-codex", 9, Some(&first_identity))]);
        let unverifiable = CodexSnapshotFence::new(&[codex_state("openai-codex", 7, None)]);

        let stable =
            settle_codex_accounts_for_fences(vec![codex.clone(), zai.clone()], &first, &same);
        assert_eq!(stable.len(), 2);

        for settled in [&changed, &moved_back, &unverifiable] {
            let filtered = settle_codex_accounts_for_fences(
                vec![codex.clone(), zai.clone()],
                &first,
                settled,
            );
            assert_eq!(filtered, vec![zai.clone()]);
        }
    }

    #[test]
    fn a_moved_slot_loses_only_its_own_rows() {
        let identity = codex_identity('6');
        let secondary_identity = codex_identity('7');
        let mut codex = observation("openai-codex", "5-hour", 10.0, CHECKED_AT);
        codex.account = Some(identity.clone());
        let mut secondary = observation("openai-codex:secondary", "5-hour", 11.0, CHECKED_AT);
        secondary.account = Some(secondary_identity.clone());
        let zai = observation("zai", "5-hour", 20.0, CHECKED_AT);

        let first = CodexSnapshotFence::new(&[
            codex_state("openai-codex", 7, Some(&identity)),
            codex_state("openai-codex:secondary", 3, Some(&secondary_identity)),
        ]);
        // The primary slot moved; the secondary slot did not.
        let moved_primary = CodexSnapshotFence::new(&[
            codex_state("openai-codex", 8, Some(&identity)),
            codex_state("openai-codex:secondary", 3, Some(&secondary_identity)),
        ]);

        let settled = settle_codex_accounts_for_fences(
            vec![codex.clone(), secondary.clone(), zai.clone()],
            &first,
            &moved_primary,
        );
        assert!(
            !settled.iter().any(|row| row.provider_id == "openai-codex"),
            "the moved slot loses its own rows"
        );
        assert!(
            settled
                .iter()
                .any(|row| row.provider_id == "openai-codex:secondary"),
            "the unmoved slot keeps its rows"
        );
        assert!(
            settled.iter().any(|row| row.provider_id == "zai"),
            "unrelated providers are never affected"
        );
    }

    #[test]
    fn an_unprovable_slot_fails_closed_without_touching_the_other_slot() {
        let identity = codex_identity('8');
        let secondary_identity = codex_identity('9');
        let mut codex = observation("openai-codex", "5-hour", 10.0, CHECKED_AT);
        codex.account = Some(identity.clone());
        let mut secondary = observation("openai-codex:secondary", "5-hour", 11.0, CHECKED_AT);
        secondary.account = Some(secondary_identity.clone());

        // The primary slot exists but no local login could be proven.
        let fence = CodexSnapshotFence::new(&[
            codex_state("openai-codex", 7, None),
            codex_state("openai-codex:secondary", 3, Some(&secondary_identity)),
        ]);
        let settled = settle_codex_accounts_for_fences(
            vec![codex.clone(), secondary.clone()],
            &fence,
            &fence,
        );
        assert!(
            !settled.iter().any(|row| row.provider_id == "openai-codex"),
            "the unprovable slot loses its own rows"
        );
        assert_eq!(
            settled
                .iter()
                .filter(|row| row.provider_id == "openai-codex:secondary")
                .count(),
            1,
            "the provable slot keeps its rows"
        );
    }


    // 15. one runtime cycle produces one history stream (runtime-owned)
    // 16. only usable snapshot entries are recorded (failed/stale/unknown
    //     never enter history; simulated providers cannot exist in the
    //     Rust runtime's production registry)
    #[tokio::test]
    async fn one_runtime_cycle_records_one_stream_from_usable_entries_only() {
        let (store, _dir) = temp_store("runtime-cycle");
        let store = std::sync::Arc::new(store);
        let core = crate::runtime::RuntimeCore::with_injections(
            vec![
                crate::runtime::ProviderSpec {
                    kind: crate::runtime::ProviderKind::Codex,
                    fetch: std::sync::Arc::new(|| {
                        Box::pin(async {
                            Ok(usage_with_identity(
                                "openai-codex",
                                Some(&codex_identity('a')),
                            ))
                                as Result<_, crate::runtime::ProviderFailure>
                        }) as crate::runtime::FetchFuture
                    }),
                },
                failed_spec("zai"),
                stale_spec("antigravity"),
            ],
            5,
            Box::new(|| 0),
            Box::new(fixed_runtime_now),
        )
        .with_history_store(Some(store.clone()));
        let core = std::sync::Arc::new(core);
        core.run_cycle().await;
        let history = store.history();
        assert_eq!(
            history.len(),
            1,
            "only the ok provider's window enters history"
        );
        assert_eq!(history[0].provider_id, "openai-codex");

        // A second identical cycle (and any number of consumer windows)
        // cannot grow the stream: recording is deduped by snapshot time.
        core.run_cycle().await;
        assert_eq!(store.history().len(), 1);
    }

    #[tokio::test]
    async fn grok_cached_snapshots_do_not_grow_history() {
        let (store, _dir) = temp_store("grok-cadence");
        let store = std::sync::Arc::new(store);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let spec = crate::runtime::ProviderSpec {
            kind: crate::runtime::ProviderKind::Grok,
            fetch: std::sync::Arc::new(move || {
                let nth = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                Box::pin(async move {
                    Ok(ok_usage("grok", "Grok (xAI)", nth as f64))
                        as Result<_, crate::runtime::ProviderFailure>
                }) as crate::runtime::FetchFuture
            }),
        };
        let core = std::sync::Arc::new(
            crate::runtime::RuntimeCore::with_injections(
                vec![spec],
                5,
                Box::new(|| 0),
                Box::new(fixed_runtime_now),
            )
            .with_history_store(Some(store.clone())),
        );
        core.run_cycle().await;
        core.run_cycle().await; // served from the 15-minute cadence cache
        let history = store.history();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].used_percent, 1.0,
            "cached result keeps its original observation time"
        );
    }

    // Eligibility port (former v03Integration "history observation boundary"):
    // the recorder keeps the five allowed fields, prefers sourceUpdatedAt,
    // skips indeterminate freshness, and drops malformed windows without
    // losing siblings.
    #[test]
    fn eligibility_prefers_source_time_and_skips_indeterminate_freshness() {
        let mut fresh = ok_usage("antigravity", "Google Antigravity", 50.0);
        fresh.data_freshness = Some("fresh");
        fresh.source_updated_at = Some("2026-09-28T11:30:00.000Z".to_string());
        let mut indeterminate = ok_usage("zai", "Z.ai", 60.0);
        indeterminate.data_freshness = Some("fresh");
        indeterminate.source_updated_at = None;
        let mut cached = usage_with_identity("openai-codex", Some(&codex_identity('b')));
        cached.limits[0].used_percent = 70.0;
        cached.data_freshness = Some("stale");
        let observations = observations_from_usages(&[fresh, indeterminate, cached]);
        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0].observed_at, "2026-09-28T11:30:00.000Z",
            "source snapshot time wins over the fetch time"
        );
        assert_eq!(observations[0].provider_id, "antigravity");
    }

    // v0.6 integration pin: eligibility is canonical-health based. Every
    // non-live health — stale (persisted last-good), error, cooldown,
    // unavailable — is rejected even with otherwise-perfect windows, while
    // the same windows record once the health is live. The legacy `status`
    // string is only a projection and must not drive eligibility.
    #[test]
    fn eligibility_accepts_only_live_canonical_health() {
        let non_live = [
            (crate::runtime::ProviderHealth::Stale, "stale"),
            (crate::runtime::ProviderHealth::Error, "error"),
            (crate::runtime::ProviderHealth::Cooldown, "error"),
            (crate::runtime::ProviderHealth::Unavailable, "error"),
            (crate::runtime::ProviderHealth::Unknown, "unknown"),
        ];
        for (health, legacy) in non_live {
            let mut usage = usage_with_identity("openai-codex", Some(&codex_identity('c')));
            usage.limits[0].used_percent = 42.0;
            usage.health = health;
            usage.status = legacy;
            usage.data_freshness = Some("fresh");
            usage.source_updated_at = Some("2026-09-28T11:30:00.000Z".to_string());
            assert!(
                observations_from_usages(&[usage]).is_empty(),
                "{health:?} must never produce a history observation"
            );
        }

        let mut live = usage_with_identity("openai-codex", Some(&codex_identity('d')));
        live.limits[0].used_percent = 42.0;
        live.data_freshness = Some("fresh");
        live.source_updated_at = Some("2026-09-28T11:30:00.000Z".to_string());
        assert_eq!(observations_from_usages(&[live]).len(), 1);
    }

    #[test]
    fn eligibility_drops_malformed_windows_without_losing_siblings() {
        let mut usage = ok_usage("zai", "Z.ai", 63.0);
        usage.limits = vec![
            UsageLimitDto {
                label: "  ".to_string(),
                used_percent: 10.0,
                reset_at: None,
                reset_text: None,
            },
            UsageLimitDto {
                label: "5-hour".to_string(),
                used_percent: f64::NAN,
                reset_text: None,
                reset_at: None,
            },
            // An unparseable resetAt degrades to absent — the window is
            // still recorded (engine tolerance parity).
            UsageLimitDto {
                label: "weekly".to_string(),
                used_percent: 20.0,
                reset_at: Some("garbage".to_string()),
                reset_text: None,
            },
            UsageLimitDto {
                label: "weekly".to_string(),
                used_percent: 25.0,
                reset_text: None,
                reset_at: Some("2026-10-01T00:00:00.000Z".to_string()),
            },
        ];
        let candidates = observations_from_usages(&[usage]);
        assert_eq!(
            candidates.len(),
            2,
            "blank label and NaN percent drop; both weekly windows survive eligibility"
        );
        assert_eq!(candidates[0].window_label, "weekly");
        assert_eq!(
            candidates[0].reset_at, None,
            "garbage resetAt degrades to absent"
        );
        assert_eq!(
            candidates[1].reset_at.as_deref(),
            Some("2026-10-01T00:00:00.000Z")
        );

        // Through the store, the two same-instant weekly candidates dedup
        // with the newest input winning — the recorded history keeps one.
        let (store, _dir) = temp_store("eligibility-dedup");
        store.record(candidates);
        let history = store.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].used_percent, 25.0);
        assert_eq!(
            history[0].reset_at.as_deref(),
            Some("2026-10-01T00:00:00.000Z")
        );
    }

    #[test]
    fn eligibility_carries_the_account_identity() {
        let mut usage = ok_usage("opencode-go", "OpenCode Go", 42.0);
        usage.account = Some(AccountAttributionDto {
            label: "key ••3456".to_string(),
            note: None,
            identity: Some("key:3456".to_string()),
        });
        let observations = observations_from_usages(&[usage]);
        assert_eq!(observations[0].account.as_deref(), Some("key:3456"));

        let mut blank = ok_usage("opencode-go", "OpenCode Go", 42.0);
        blank.account = Some(AccountAttributionDto {
            label: "key ••".to_string(),
            note: None,
            identity: Some("   ".to_string()),
        });
        assert_eq!(
            observations_from_usages(&[blank])[0].account,
            None,
            "a blank identity token stays unattributed"
        );
    }

    // 17. credentials/tokens are never part of the serialized schema
    #[test]
    fn serialized_schema_carries_only_the_safe_fields() {
        let mut entry = observation("opencode-go", "5-hour", 42.0, CHECKED_AT);
        entry.reset_at = Some("2026-09-28T13:00:00.000Z".to_string());
        entry.account = Some("key:3456".to_string());
        let wire = serde_json::to_value(&entry).unwrap();
        let mut keys: Vec<&str> = wire
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "account",
                "observedAt",
                "providerId",
                "resetAt",
                "usedPercent",
                "windowLabel"
            ],
            "exactly the six safe fields serialize; no token/credential/secret-shaped field exists"
        );
        let serialized = serde_json::to_string(&entry).unwrap().to_lowercase();
        for forbidden in [
            "token",
            "bearer",
            "apikey",
            "api_key",
            "secret",
            "password",
            "authorization",
            "jwt",
            "refresh",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "persisted schema must never contain secret-shaped field names ({forbidden})"
            );
        }
        // Optional fields are omitted when absent (wire parity with TS).
        let minimal = serde_json::to_value(&observation("zai", "5-hour", 1.0, CHECKED_AT)).unwrap();
        assert!(minimal.get("resetAt").is_none());
        assert!(minimal.get("account").is_none());
        assert!(
            serde_json::to_string(&minimal)
                .unwrap()
                .find("reset_at")
                .is_none(),
            "snake_case must never leak"
        );
    }

    // 18. restart/reload history round-trip preserves valid data
    #[test]
    fn history_round_trips_across_a_restart() {
        let dir = tempdir::temp_dir("round-trip");
        let path = dir.path().join(HISTORY_FILE_NAME);
        let mut attributed = observation("opencode-go", "5-hour", 42.0, CHECKED_AT);
        attributed.account = Some("key:3456".to_string());
        attributed.reset_at = Some("2026-09-28T13:00:00.000Z".to_string());
        let other = observation("zai", "weekly", 63.0, "2026-09-28T11:00:00.000Z");
        {
            let store = QuotaHistoryStore::open_with_clock(path.clone(), fixed_clock());
            store.record(vec![attributed.clone(), other.clone()]);
        }
        // "Restart": a fresh store instance over the same file.
        let reopened = QuotaHistoryStore::open_with_clock(path, fixed_clock());
        assert_eq!(reopened.history(), vec![attributed, other]);
    }

    #[test]
    fn persisted_blob_matches_the_legacy_ts_shape() {
        let (store, dir) = temp_store("blob-shape");
        store.record(vec![observation("zai", "5-hour", 10.0, CHECKED_AT)]);
        let raw = std::fs::read_to_string(dir.path().join(HISTORY_FILE_NAME)).unwrap();
        let blob: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(blob["version"], 2);
        assert!(blob["observations"].is_array());
        assert_eq!(blob["observations"][0]["providerId"], "zai");
        assert_eq!(blob["observations"][0]["windowLabel"], "5-hour");
        assert_eq!(blob["observations"][0]["usedPercent"], 10.0);
        assert_eq!(blob["observations"][0]["observedAt"], CHECKED_AT);
    }

    #[test]
    fn revision_bumps_only_when_history_changes() {
        let (store, _dir) = temp_store("revision");
        assert_eq!(store.revision(), 0);
        store.record(vec![observation("zai", "5-hour", 10.0, CHECKED_AT)]);
        let after_record = store.revision();
        assert_eq!(after_record, 1);
        // Re-recording the same snapshot changes nothing.
        store.record(vec![observation("zai", "5-hour", 10.0, CHECKED_AT)]);
        assert_eq!(store.revision(), after_record);
        store.clear();
        assert_eq!(store.revision(), after_record + 1);
    }

    // -------------------------------------------------------------
    // v0.6 trend retention, compaction, reset boundary, & range tests
    // -------------------------------------------------------------

    // 1. recent 24h samples remain detailed
    #[test]
    fn recent_24h_samples_remain_detailed() {
        let (store, _dir) = temp_store("recent-detailed");
        // Record 5 samples within recent 24 hours chronologically in the same 30m window
        let mut candidates = Vec::new();
        for i in 0..5 {
            let offset_mins = 20 - i * 2;
            candidates.push(observation(
                "zai",
                "5-hour",
                10.0 + i as f64,
                &at(offset_mins),
            ));
        }
        store.record(candidates);
        let history = store.history();
        assert_eq!(
            history.len(),
            5,
            "all recent 24h samples remain uncompacted and detailed"
        );
        for (i, obs) in history.iter().enumerate() {
            assert_eq!(obs.used_percent, 10.0 + i as f64);
        }
    }

    // 2. 2-day-old samples compact
    #[test]
    fn two_day_old_samples_compact_into_buckets() {
        let (store, _dir) = temp_store("two-day-compact");
        let two_days_ago_ms = NOW_MS - 48 * 60 * 60_000;
        // Two samples in the exact same 30-minute bucket 48 hours ago
        let s1 = observation("zai", "5-hour", 20.0, &canonical_timestamp(two_days_ago_ms));
        let s2 = observation(
            "zai",
            "5-hour",
            25.0,
            &canonical_timestamp(two_days_ago_ms + 10 * 60_000),
        );
        store.record(vec![s1, s2.clone()]);
        // Prediction 24h query ignores 2-day-old samples
        assert!(store.history().is_empty());
        // 7d query sees the compacted 1 sample (the later one in the 30m bucket)
        let seven_d = store.history_range(Some("zai"), None, Some("5-hour"), "7d");
        assert_eq!(
            seven_d.len(),
            1,
            "two samples in the same 30m bucket compact to one"
        );
        assert_eq!(
            seven_d[0].used_percent, 25.0,
            "latest sample in the bucket wins"
        );
        assert_eq!(seven_d[0].observed_at, s2.observed_at);
    }

    // 3. 7-day retention works
    #[test]
    fn seven_day_retention_works() {
        let (store, _dir) = temp_store("seven-day-retention");
        let exact_seven_days_ms = NOW_MS - 7 * 24 * 60 * 60_000;
        let s = observation(
            "zai",
            "5-hour",
            42.0,
            &canonical_timestamp(exact_seven_days_ms),
        );
        store.record(vec![s]);
        let trend = store.history_range(None, None, None, "7d");
        assert_eq!(trend.len(), 1, "observation at exactly 7 days is retained");
        assert_eq!(trend[0].used_percent, 42.0);
    }

    // 4. >7-day observations expire
    #[test]
    fn older_than_seven_days_observations_expire() {
        let (store, _dir) = temp_store("expire-seven-days");
        let over_seven_days_ms = NOW_MS - (7 * 24 * 60 * 60_000 + 1_000);
        let s = observation(
            "zai",
            "5-hour",
            42.0,
            &canonical_timestamp(over_seven_days_ms),
        );
        store.record(vec![s]);
        let trend = store.history_range(None, None, None, "7d");
        assert!(
            trend.is_empty(),
            "observations older than 7 days must expire"
        );
    }

    // 5. bucket selection deterministic
    #[test]
    fn bucket_selection_is_deterministic() {
        let t1_ms = NOW_MS - 40 * 60 * 60_000;
        let bucket1 = t1_ms / QUOTA_HISTORY_COMPACT_BUCKET_MS;
        let bucket1_later = (t1_ms + 10 * 60_000) / QUOTA_HISTORY_COMPACT_BUCKET_MS;
        assert_eq!(
            bucket1, bucket1_later,
            "same 30m window maps to same bucket index"
        );
        let bucket2 = (t1_ms + 35 * 60_000) / QUOTA_HISTORY_COMPACT_BUCKET_MS;
        assert_ne!(
            bucket1, bucket2,
            "different 30m window maps to different bucket index"
        );
    }

    // 6. reset boundary preserved
    #[test]
    fn reset_boundary_preserved_across_compaction() {
        let (store, _dir) = temp_store("reset-boundary");
        let two_days_ago_ms = NOW_MS - 48 * 60 * 60_000;
        // Observations within the SAME 30m bucket straddling a reset cycle:
        // Pre-reset: 95% at t0 with reset_at cycle 1
        let mut pre = observation(
            "zai",
            "5-hour",
            95.0,
            &canonical_timestamp(two_days_ago_ms + 5 * 60_000),
        );
        pre.reset_at = Some(canonical_timestamp(two_days_ago_ms + 10 * 60_000));

        // Post-reset: 5% at t0 + 15min with reset_at cycle 2 (moved forward by 5 hours)
        let mut post = observation(
            "zai",
            "5-hour",
            5.0,
            &canonical_timestamp(two_days_ago_ms + 15 * 60_000),
        );
        post.reset_at = Some(canonical_timestamp(
            two_days_ago_ms + (5 * 60 + 10) * 60_000,
        ));

        store.record(vec![pre, post]);
        let trend = store.history_range(Some("zai"), None, Some("5-hour"), "7d");
        assert_eq!(
            trend.len(),
            2,
            "both pre-reset peak and post-reset floor are preserved; no collapse"
        );
        assert_eq!(trend[0].used_percent, 95.0);
        assert_eq!(trend[1].used_percent, 5.0);

        // Also verify when reset_at is absent but percentage drops sharply (> 5%)
        let (store2, _dir2) = temp_store("reset-boundary-no-resetat");
        let pre_drop = observation(
            "codex",
            "5-hour",
            90.0,
            &canonical_timestamp(two_days_ago_ms + 5 * 60_000),
        );
        let post_drop = observation(
            "codex",
            "5-hour",
            10.0,
            &canonical_timestamp(two_days_ago_ms + 15 * 60_000),
        );
        store2.record(vec![pre_drop, post_drop]);
        let trend2 = store2.history_range(Some("codex"), None, Some("5-hour"), "7d");
        assert_eq!(
            trend2.len(),
            2,
            "percentage drop > 5% identifies reset boundary even without reset_at"
        );
        assert_eq!(trend2[0].used_percent, 90.0);
        assert_eq!(trend2[1].used_percent, 10.0);
    }

    // 10 & 11. prediction 24h query unchanged & compacted history not accidentally used by prediction
    #[test]
    fn prediction_query_stays_strictly_dense_24h_without_compacted_data() {
        let (store, _dir) = temp_store("prediction-isolation");
        let recent = observation("zai", "5-hour", 50.0, &at(30));
        let two_days_ago_ms = NOW_MS - 48 * 60 * 60_000;
        let compacted = observation("zai", "5-hour", 10.0, &canonical_timestamp(two_days_ago_ms));

        store.record(vec![recent.clone(), compacted.clone()]);

        // store.history() is the prediction entry point (via get_history)
        let pred_history = store.history();
        assert_eq!(
            pred_history,
            vec![recent],
            "prediction query sees only 24h data"
        );
        assert!(
            !pred_history.contains(&compacted),
            "compacted 2-day-old data must not leak into predictions"
        );

        // UI trend query sees both
        let trend = store.history_range(None, None, None, "7d");
        assert_eq!(trend.len(), 2);
    }

    // 12 & 13. 24h and 7d query behavior
    #[test]
    fn range_queries_24h_and_7d_return_expected_slices() {
        let (store, _dir) = temp_store("ranges");
        let h2 = observation("zai", "5-hour", 60.0, &at(120)); // 2 hours ago
        let d3 = observation(
            "zai",
            "5-hour",
            30.0,
            &canonical_timestamp(NOW_MS - 72 * 60 * 60_000),
        ); // 3 days ago

        store.record(vec![h2.clone(), d3.clone()]);

        let q24 = store.history_range(None, None, None, "24h");
        assert_eq!(q24, vec![h2.clone()]);

        let q7d = store.history_range(None, None, None, "7d");
        assert_eq!(q7d, vec![d3, h2]);
    }

    // 14. clear history clears all tiers (recent + compacted)
    #[test]
    fn clear_history_clears_both_recent_and_compacted_tiers() {
        let (store, dir) = temp_store("clear-tiers");
        let h2 = observation("zai", "5-hour", 60.0, &at(120));
        let d3 = observation(
            "zai",
            "5-hour",
            30.0,
            &canonical_timestamp(NOW_MS - 72 * 60 * 60_000),
        );

        store.record(vec![h2, d3]);
        assert_eq!(store.history_range(None, None, None, "7d").len(), 2);

        store.clear();
        assert!(store.history_range(None, None, None, "7d").is_empty());
        assert!(store.history().is_empty());

        let reopened =
            QuotaHistoryStore::open_with_clock(dir.path().join(HISTORY_FILE_NAME), fixed_clock());
        assert!(
            reopened.history_range(None, None, None, "7d").is_empty(),
            "cleared tiers stay empty after restart"
        );
    }

    // 15. restart persistence across tiers
    #[test]
    fn restart_persistence_preserves_both_tiers() {
        let (store, dir) = temp_store("restart-tiers");
        let h2 = observation("zai", "5-hour", 60.0, &at(120));
        let d3 = observation(
            "zai",
            "5-hour",
            30.0,
            &canonical_timestamp(NOW_MS - 72 * 60 * 60_000),
        );

        store.record(vec![h2.clone(), d3.clone()]);
        drop(store);

        let reopened =
            QuotaHistoryStore::open_with_clock(dir.path().join(HISTORY_FILE_NAME), fixed_clock());
        let trend = reopened.history_range(None, None, None, "7d");
        assert_eq!(trend, vec![d3, h2]);
    }

    // 17. legacy store migration
    #[test]
    fn legacy_store_migration_upgrades_v1_blob_to_v2() {
        let dir = tempdir::temp_dir("migration-v1-to-v2");
        let path = dir.path().join(HISTORY_FILE_NAME);
        let blob = serde_json::json!({
            "version": 1,
            "observations": [
                {
                    "providerId": "opencode-go",
                    "windowLabel": "5-hour",
                    "usedPercent": 42.0,
                    "observedAt": at(60),
                    "account": "key:3456"
                }
            ]
        });
        std::fs::write(&path, blob.to_string()).unwrap();

        let store = QuotaHistoryStore::open_with_clock(path.clone(), fixed_clock());
        let history = store.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].provider_id, "opencode-go");
        assert_eq!(history[0].account.as_deref(), Some("key:3456"));

        // Verify the file was rewritten as version 2
        let raw = std::fs::read_to_string(&path).unwrap();
        let upgraded: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            upgraded["version"], 2,
            "legacy v1 blob is automatically upgraded to v2"
        );
    }

    // 18. max points/window enforced
    #[test]
    fn max_points_per_window_enforced_for_both_tiers() {
        let (store, _dir) = temp_store("max-points");
        // Record 550 recent samples -> capped at 500
        let recent_total = QUOTA_HISTORY_MAX_PER_WINDOW + 50;
        let mut candidates = Vec::new();
        for i in 0..recent_total {
            candidates.push(observation(
                "zai",
                "5-hour",
                (i % 100) as f64,
                &canonical_timestamp(NOW_MS - 1000 * (recent_total - i) as i64),
            ));
        }
        store.record(candidates);
        assert_eq!(
            store.history().len(),
            QUOTA_HISTORY_MAX_PER_WINDOW,
            "recent tier capped at 500"
        );

        // Record 400 compacted samples in distinct buckets -> capped at 350
        let compacted_total = QUOTA_HISTORY_MAX_COMPACTED_PER_WINDOW + 50;
        let mut older_candidates = Vec::new();
        for i in 0..compacted_total {
            let t = NOW_MS - 48 * 60 * 60_000 - (i as i64) * 30 * 60_000;
            if t >= NOW_MS - QUOTA_HISTORY_RETENTION_MS {
                older_candidates.push(observation("zai", "5-hour", 50.0, &canonical_timestamp(t)));
            }
        }
        store.record(older_candidates);
        let all_7d = store.history_range(Some("zai"), None, Some("5-hour"), "7d");
        assert!(
            all_7d.len() <= QUOTA_HISTORY_MAX_PER_WINDOW + QUOTA_HISTORY_MAX_COMPACTED_PER_WINDOW
        );
    }

    // 22. storage growth bounded & performance benchmark
    #[test]
    fn storage_growth_bounded_and_performance_measured() {
        let (store, dir) = temp_store("storage-bounded");
        // Simulate 7 days of 5-minute refreshes for 5 providers with 2 windows each
        // (12 refreshes/hr * 24 * 7 = 2016 intervals * 10 windows = 20,160 raw observation candidates)
        let providers = ["openai-codex", "zai", "opencode-go", "antigravity", "grok"];
        let windows = ["5-hour", "weekly"];
        let codex = codex_identity('e');

        let mut all_samples = Vec::new();
        for interval in 0..2016 {
            let t_ms = NOW_MS - (2015 - interval) * 5 * 60_000;
            let t_str = canonical_timestamp(t_ms);
            for p in &providers {
                for w in &windows {
                    let percent = ((interval % 100) as f64).clamp(0.0, 100.0);
                    let mut entry = observation(p, w, percent, &t_str);
                    if p == &"openai-codex" {
                        entry.account = Some(codex.clone());
                    }
                    all_samples.push(entry);
                }
            }
        }

        let start_record = std::time::Instant::now();
        store.record(all_samples);
        let record_elapsed = start_record.elapsed();

        let path = dir.path().join(HISTORY_FILE_NAME);
        let file_meta = std::fs::metadata(&path).unwrap();
        let file_size_kb = file_meta.len() as f64 / 1024.0;

        let start_load = std::time::Instant::now();
        let reopened = QuotaHistoryStore::open_with_clock(path.clone(), fixed_clock());
        let load_elapsed = start_load.elapsed();

        let total_7d = reopened.history_range(None, None, None, "7d");
        let pred_24h = reopened.history();

        assert!(
            total_7d.len()
                <= 10 * (QUOTA_HISTORY_MAX_PER_WINDOW + QUOTA_HISTORY_MAX_COMPACTED_PER_WINDOW)
        );
        assert!(pred_24h.len() <= 10 * QUOTA_HISTORY_MAX_PER_WINDOW);

        println!("BENCHMARK: 7-day realistic dataset (10 windows, 20160 raw points ingested)");
        println!("  Compact + write time: {:?}", record_elapsed);
        println!("  File size on disk:    {:.2} KB", file_size_kb);
        println!("  Load + parse time:    {:?}", load_elapsed);
        println!("  Total 7d retained:    {} points", total_7d.len());
        println!("  Total 24h pred:       {} points", pred_24h.len());

        assert!(
            file_size_kb < 1500.0,
            "File size must remain well under 1.5MB (actual: {:.2}KB)",
            file_size_kb
        );
    }

    // ---------- shared fixtures ----------

    fn fixed_runtime_now() -> DateTime<Utc> {
        Utc.timestamp_millis_opt(NOW_MS).unwrap()
    }

    fn ok_usage(id: &str, name: &str, percent: f64) -> ProviderUsageDto {
        ProviderUsageDto {
            id: id.to_string(),
            name: name.to_string(),
            status: "ok",
            health: crate::runtime::ProviderHealth::Live,
            checked_at: CHECKED_AT.to_string(),
            limits: vec![UsageLimitDto {
                label: "5-hour".to_string(),
                used_percent: percent,
                reset_text: None,
                reset_at: None,
            }],
            account: None,
            plan_type: None,
            error: None,
            error_category: None,
            error_http_status: None,
            source_updated_at: None,
            data_freshness: None,
            reset_credits: None,
            zcode_reset_cards: None,
            zcode_plans: None,
            grok_bot: None,
            account_alias: None,
            monitoring_paused: None,
            fallback_failure: None,
        }
    }

    fn failed_spec(id: &'static str) -> crate::runtime::ProviderSpec {
        let kind = kind_of(id);
        crate::runtime::ProviderSpec {
            kind,
            fetch: std::sync::Arc::new(move || {
                let _ = id;
                Box::pin(async {
                    Err(crate::runtime::ProviderFailure {
                        code: "auth_expired".to_string(),
                        message: "session expired".to_string(),
                        http_status: None,
                        transient: Some(false),
                        retry_after_ms: None,
                        identity: None,
                        credential_binding: None,
                        transport_timeout: false,
                    })
                        as Result<ProviderUsageDto, crate::runtime::ProviderFailure>
                }) as crate::runtime::FetchFuture
            }),
        }
    }

    fn stale_spec(id: &'static str) -> crate::runtime::ProviderSpec {
        let kind = kind_of(id);
        crate::runtime::ProviderSpec {
            kind,
            fetch: std::sync::Arc::new(move || {
                let _ = id;
                Box::pin(async {
                    let mut usage = ok_usage(id, "Stale", 10.0);
                    usage.health = crate::runtime::ProviderHealth::Stale;
                    usage.status = usage.health.legacy_status();
                    Ok(usage) as Result<ProviderUsageDto, crate::runtime::ProviderFailure>
                }) as crate::runtime::FetchFuture
            }),
        }
    }

    fn kind_of(id: &str) -> crate::runtime::ProviderKind {
        match id {
            "openai-codex" => crate::runtime::ProviderKind::Codex,
            "zai" => crate::runtime::ProviderKind::Zai,
            "opencode-go" => crate::runtime::ProviderKind::OpenCodeGo,
            "antigravity" => crate::runtime::ProviderKind::Antigravity,
            "grok" => crate::runtime::ProviderKind::Grok,
            _ => unreachable!("unknown provider id in test fixture"),
        }
    }
}
