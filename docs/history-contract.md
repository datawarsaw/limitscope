# Quota History Contract (v0.6)

This document defines the architecture, retention tiers, compaction semantics, reset boundary guarantees, query surface, prediction isolation, and storage bounds for the Rust-owned quota history store in LimitScope v0.6.

---

## 1. Overview and Ownership

Quota history is owned entirely by the Rust shared runtime (src-tauri/src/history.rs).
The store records observations once per completed refresh cycle from normalized provider snapshots.

The frontend webview acts strictly as a reader and command dispatcher:
- Reads dense 24h history for predictions via get_history
- Reads trend ranges (24h, 7d, 30d) for UI visualizations via get_history_range
- Issues mutations via clear_history and one-time import_legacy_history

All persistence, deduplication, retention pruning, downsampling, reset tracking, and self-healing occur in Rust behind a non-blocking mutex.

---

## 2. Retention Model and Tiers

To support multi-day trend charts without unbounded storage growth, history is partitioned into two deterministic retention tiers:

### Tier 1: Recent Detailed Tier (0 to 24 hours)
- **Horizon:** Observations where now - observedAt <= 24 hours.
- **Resolution:** Full sampling resolution (every unique refresh cycle retained).
- **Cap:** At most 500 samples per logical window (QUOTA_HISTORY_MAX_PER_WINDOW).
- **Deduplication:** Observations of the same window with identical millisecond timestamps collapse with newest input winning.

### Tier 2: Compacted Tier (24 hours to 7 days)
- **Horizon:** Observations where 24 hours < now - observedAt <= 7 days.
- **Resolution:** Deterministic downsampling into 30-minute buckets (QUOTA_HISTORY_COMPACT_BUCKET_MS = 1_800_000 ms).
- **Representative Point:** The latest valid observation within each (bucket, reset cycle) group is retained.
- **Cap:** At most 350 compacted samples per logical window (QUOTA_HISTORY_MAX_COMPACTED_PER_WINDOW).

### Expiration (> 7 days)
- Observations older than 7 days (now - observedAt > 7 * 24h) expire and are pruned during compaction and on load.

---

## 3. Quota Reset Boundaries & Discontinuity Preservation

Quota usage resets are critical discontinuities in quota tracking. If usage resets from 95% down to 5%, downsampling must not collapse or average across the boundary.

### Boundary Detection
A reset cycle transition between chronological observations prev and curr is identified when:
1. curr.used_percent < prev.used_percent - 5.0 (usage dropped by > 5%, matching prediction engine reset detection); OR
2. curr.reset_at moved forward by more than 1 minute (or changed presence/value).

### Compaction Semantics Across Resets
Within any 30-minute bucket, bucketing groups by (bucket_index, cycle_id):
- The pre-reset peak sample (highest usage before reset) belongs to cycle N.
- The post-reset baseline sample belongs to cycle N+1.
- Both representative samples are preserved.
- No cross-reset smoothing or false averaging is ever performed.

---

## 4. Logical Identity & Partitioning

Every observation is keyed by the triple:
```
(providerId, account, windowLabel)
```

- **Multi-Account Isolation:** Different accounts for the same provider (e.g. key:A and key:B) are tracked independently. They never share deduplication, bucket compaction, or point bounds.
- **Unattributed Isolation:** Observations without proven account identity (account: None) remain strictly unattributed and isolated from attributed partitions.
- **Window Isolation:** Distinct quota windows for the same provider/account (e.g. "5-hour" vs "weekly") never merge.

---

## 5. Prediction Engine Isolation

The prediction engine in TypeScript (src/lib/prediction/engine.ts) requires dense recent observations to calculate accurate burn rates and projected exhaustion.

**Critical Rule:** Compacted multi-day history is never fed to the prediction engine.
- get_history returns strictly observations from the last 24 hours.
- Prediction calculations operate on the exact same dense 24h data as in v0.5.
- Prediction math and formulas remain completely untouched.

---

## 6. Query API

### IPC Surface

1. `get_history() -> Vec<QuotaObservation>`
   - Returns recent 24-hour detailed history for predictions.

2. `get_history_range(providerId?, account?, windowLabel?, range?, exactAccount?, query?) -> Vec<QuotaObservation>`
   - Returns trend-ready history matching requested filters.
   - **Supported ranges:**
     - "24h": recent 24 hours.
     - "7d": full 7-day retained history (recent 24h detailed + older 24h–7d compacted).
     - "30d": queries available history up to 30 days (in v0.6 bounded to the 7-day retention horizon).
   - **Multi-Provider Trends:** Omitting providerId, account, and windowLabel returns all active windows across all providers in a single call.

3. `clear_history()`
   - Clears memory and overwrites the disk file with an empty envelope. Survived restarts guarantee no resurrection.

4. `import_legacy_history(observations)`
   - Idempotent one-time import of legacy localStorage blobs.

### TypeScript Client
Exported from src/lib/historyClient.ts:
- `loadRuntimeHistory(): Promise<QuotaObservation[]>`
- `loadRuntimeHistoryRange(params?: HistoryRangeParams): Promise<QuotaObservation[]>`
- `clearRuntimeHistory(): Promise<void>`

---

## 7. Storage Bounds & Performance

### Hard Bounds
- Recent tier: 500 samples / window.
- Compacted tier: 350 samples / window.
- Max points per window: 850 samples.
- Single observation wire size: ~150–180 bytes JSON.

### Storage Estimate
For a heavy multi-provider workload (5 providers, 2 windows each = 10 windows):
- Max total points: 10 * 850 = 8,500 points.
- Worst-case file size: ~1.4 MB.
- Realistic 7-day continuous ingestion (5-minute refresh cadence, 20,160 raw incoming samples):
  - Persisted file size: ~627 KB.
  - Total retained points: 5,940 points (~594 points / window).
  - Compaction + write time: < 700 ms for 20,160 raw points.
  - Disk load + parse time: < 250 ms.

Compaction executes synchronously during writes and on disk load; there is no background compaction thread or timer.

---

## 8. Versioning and Migration

The storage envelope schema uses version 2:
```json
{
  "version": 2,
  "observations": [ ... ]
}
```

- **Migration from v1:** When a legacy version: 1 file is encountered on disk, all valid observations are parsed, retained, and the file is automatically rewritten as version: 2.
- **Self-Healing:** Corrupted files or unknown foreign versions (e.g. version: 99) degrade safely to empty without crashing application startup.
