# Usage Intelligence — v0.8.9 Phase 1 (ZCode)

Status: implemented (Phase 1 ZCode, Phase 2 Codex, Phase 3 OpenCode).
This document describes the plane and the ZCode source in depth; the
OpenCode source is specified in the section below. The Codex rollout
reader (`usage_source_codex.rs`) follows the same contracts.

## What this plane is (and is not)

Usage Intelligence is a **second, parallel local-data plane** next to the
quota-history plane (`quota-history-v1.json` / `QuotaObservation`):

| | Quota history | Usage Intelligence |
|---|---|---|
| Question | how full is each quota window? | how many tokens did each model actually consume? |
| Unit | used percent per window | token counts per completed request |
| Store | `quota-history-v1.json` | `usage-intelligence-v1.json` |
| Feeds | Usage Analytics, predictions | the Today / 7d / 30d token view |

Nothing in this plane extends, overloads, or reinterprets quota history.
The two stores never read each other.

Honesty pins (surfaced in the UI and the API):

- these are **reported token counts collected locally** — never account
  quota, billing usage, exact spend, or complete historical usage;
- history before the first enable is **never imported** (no backfill), so
  a "30d" view on a fresh install discloses `Collected locally since …`
  and `incompleteHistory` rather than implying 30 measured days.

## Event model (`usage_intelligence::UsageEvent`)

Source-neutral; designed so Codex/OpenCode readers can produce it without
a migration for the basic fields:

- `id` — stable owned identity `"{source}:{source_event_id}"`; the dedup key
- `source` — harness identity (`zcode`; `codex`/`opencode` reserved, not a
  user-facing grouping dimension in the MVP)
- `provider` — normalized provider axis (e.g. `zai`), kept **separate from
  the model dimension**
- `sourceProviderId` — the raw source provider string, provenance only
- `model` — as the source reports it (e.g. `GLM-5.3`)
- `eventAt` / `observedAt` — source completion time / LimitScope ingest time
- non-overlapping token dimensions: `inputTokens` (non-cached), `cacheReadTokens`,
  `cacheWriteTokens`, `outputTokens` (ex-reasoning), `reasoningTokens`
- `totalTokens` — the source's own canonical total

Deliberately absent: cost, pricing, project, prompt/response text,
conversation titles, task names, tool-call content.

## ZCode source (`usage_source_zcode.rs`)

- Location: `~/.zcode/cli/db/db.sqlite`, table `model_usage` (schema
  verified against the installed CLI 2026-10-06; additive columns are
  tolerated, a missing required column fails closed as
  `schemaUnsupported`).
- Read strategy: **snapshot copy** — the database plus its `-wal` sidecar
  are copied to a fresh temp directory and only the copy is opened (the
  same pattern `cursor_grok_bot.rs` uses). The source files are never
  opened for writing, never locked, never checkpointed; `-shm` is never
  copied. A cheap (size, mtime) fingerprint skips the copy entirely when
  the source is unchanged since the last successful scan.
- Cursor: `completed_at` watermark, ordered by `(completed_at, id)`. An
  immutable `baselineWatermarkMs` anchors the backlog boundary — reads
  never reach below it. Each incremental pass reads in two phases:
  1. a **forward drain** of rows strictly after the persisted composite
     `(completed_at, id)` batch position (from the baseline watermark on
     the first scan), and
  2. once drained, a **late-commit sweep** of the 10-minute lookback
     band at-or-before that position (newest first), catching rows
     committed after an earlier scan but stamped slightly behind it;
     id-dedup absorbs the re-reads.
- Scan bound: one scan commits at most 20,000 forward rows — a
  deterministic prefix under the `(completed_at, id)` order. An
  over-bound window commits that prefix (events stored, position
  persisted), reports `scanCeiling` with a progress-bearing detail, and
  continues from the exact next row on subsequent scans — including
  through a tie group larger than one batch. Nothing is skipped and an
  oversized backlog can never stall the collector (the v0.8.21 audit's
  F1 fix). The watermark advances through every committed row —
  accepted and rejected alike — so permanently malformed rows cannot
  block progress; they stay revisitable only inside the lookback band,
  then age out like late commits.
- Completion predicate: `status = 'completed' AND completed_at IS NOT NULL`.
- Fail-closed rows (rejected and counted, never guessed): blank
  provider/model, implausible timestamps, negative or NULL dimensions,
  `cache read + cache write > input`, `reasoning > output`, or
  `computed_total_tokens != input + output` (the verified live invariant).

### Verified token semantics (2026-10-06, 28,873 completed rows)

`computed_total_tokens == input_tokens + output_tokens` in every row,
`provider_total_tokens` mirrors it, and `input_tokens` already contains
the cache dimensions. Normalization therefore stores non-overlapping
dimensions (input minus cache read/write; output minus reasoning) and
takes the total from the source — the five raw fields are never summed.
Zero is preserved as measured zero (the dimensions are NOT NULL in the
source schema); absent values cannot occur for the dimensions and a NULL
total rejects the row.

## OpenCode source (`usage_source_opencode.rs`)

- Location: `~/.local/share/opencode/opencode.db`, table `message` (schema
  verified against the live corpus 2026-10-06: `id` PK, `session_id`,
  `time_created`, `time_updated`, `data` JSON). Additive columns are
  tolerated; a missing required column fails closed as
  `schemaUnsupported`.
- Accounting layer: the per-assistant-row `message.data.tokens` JSON
  only. The derived layers OpenCode maintains from it — step-finish
  parts (`part` table), the session aggregate, the event replication
  log — are never read (fixture-proven with inflated sibling rows).
- Read strategy: the same snapshot-copy pattern as ZCode (database plus
  `-wal`, never `-shm`, never any write open). A (size, mtime)
  fingerprint skips the copy for an unchanged source.
- Completion predicate (verified live): `time.completed` present AND
  `error` absent AND `finish` present — applied in SQL, so user rows,
  in-flight rows, and error rows (error payloads included) are never
  fetched or parsed. `finish` alone is no signal (the dominant live
  value is literally `"unknown"`).
- Cursor: `time_updated` — the row's last-write stamp — ordered by
  `(time_updated, id)`, floored at the immutable baseline watermark.
  Completions, late usage writes, and fork copies all bump it, so every
  newly-committed accounting state is caught regardless of request age.
  Each incremental pass reads in two phases: a **forward drain** of
  rows strictly after the persisted composite `(time_updated, id)`
  batch position, and — once drained — a **late-commit sweep** of the
  10-minute lookback band at-or-before that position (newest first).
  The watermark advances through every committed row: accepted events,
  all-zero rows dispositioned as usage-not-reported, and rejected rows
  alike.
- Token semantics (verified 428/428 live): the five dimensions
  (`input`, `output`, `reasoning`, `cache.read`, `cache.write`) are a
  **disjoint partition** — the total equals their sum, so they map to
  the event dimensions directly (no overlap subtraction, unlike ZCode)
  and the total is derived from them. A source total that disagrees
  rejects the row.
- Sparse reporting: completed, error-free rows whose every dimension is
  zero are the verified "usage not reported" pattern (dominated by
  Google / Antigravity Claude thinking models: ~2.4k of ~2.9k live
  assistant rows). They are never stored as measured zero; they are
  counted per scan as `usageNotReported` in the source diagnostics and
  disposition the cursor.
- Fork deduplication (verified live: 10 cross-session duplicate pairs,
  ~2% of volume): forking a session duplicates executed request rows
  under new message ids while preserving `time_created` and the whole
  accounting payload, so the owned identity is the SHA-256 content
  address of `(time_created, providerID, modelID, input, output,
  reasoning, cache_read, cache_write, cost)`. Both copies map to one
  id; the store's id dedup collapses them across rescans and restarts.
  Residual risk (documented): two truly distinct requests sharing the
  full fingerprint — same millisecond, model, dimensions, cost —
  collapse into one event.
- Scan bound: one scan commits at most 20,000 forward rows; an
  over-bound backlog commits its deterministic prefix, reports
  `scanCeiling` with a progress-bearing detail, and continues from the
  persisted position on subsequent scans — nothing is skipped, nothing
  stalls (the same bounded-batch contract as the ZCode reader).

## Local-data controls (docs/local-data-controls-v0.7.md §14)

- **Opt-in first**: the persisted `enabled` flag lives in the owned store;
  while false the collector returns before resolving the source path — no
  stat, no existence probe, no open, no copy, no schema inspection
  (unit-proven with a counting resolver; runtime-cycle-proven the same way).
- **Backlog skip**: the first successful observation establishes the
  baseline at `MAX(completed_at) + 1` and imports nothing. The +1 keeps
  the inclusive incremental window from re-admitting the boundary row.
- **Disable → re-enable (fresh baseline)**: usage completed while
  Usage Intelligence was disabled is never silently imported. The
  disable → enable transition marks every source cursor
  `pendingRebaseline` (persisted, so it survives a quit before the next
  pass); the next collection pass establishes a fresh baseline per
  source at its then-current high-water mark — the same backlog skip as
  a first enable. Already-collected events, the historical token
  totals, and the `collected locally since` disclosure anchor are
  preserved untouched; nothing is deleted. A re-enable that fails (the
  source is missing, or its baseline high-water read fails) keeps the
  pending flag, so a later pass retries the fresh baseline — the store
  never resumes from the stale pre-disable watermark. The Codex reader
  honors the same flag: its fresh baseline re-tracks every rollout file
  at its current EOF.
- **Read-only foreign ownership**: see the snapshot strategy above; the
  live proof asserts whole-file byte identity of the main database.
- **Bounded retention**: 35 days by `eventAt` plus a 200k event hard cap;
  only LimitScope-owned events are ever deleted.
- **Owned clear** (`clear_usage_intelligence`): clears events, cursors,
  and the disclosure anchor, preserves the opt-in flag, and re-baselines
  on the next collection — pre-clear usage never reappears.
- **No silent cursor skip**: cursor movement happens only through
  successfully applied scans; failures leave it in place.

## Partial scans, cursor guarantees, and known completeness limits

- **Partial scans / backlog behavior**: one scan commits at most
  20,000 eligible rows (ZCode/OpenCode; the Codex reader has analogous
  per-cycle file/byte/event budgets). An oversized incremental backlog
  — e.g. usage that piled up while the machine was off or the source
  was idle for days — drains across scans: each scan commits its
  deterministic prefix, persists the composite `(cursor-column, id)`
  resume position, and reports `scanCeiling` with a detail carrying the
  new watermark; the state heals to `ok` on the scan that drains the
  window. The saved position means a batch ending mid-tie (thousands of
  rows sharing one timestamp) resumes at the exact next row — nothing
  is skipped, nothing re-blocks, and committed rows never refill later
  batches as duplicates.
- **Cursor and deduplication guarantees**: event identities are stable
  and content-bound (`zcode:{row id}`, `opencode:{content address}`,
  `codex:{source-native ids}`), so re-reading any overlap — restarts,
  repeated scans, lookback sweeps — can never grow totals; the store
  counts re-reads as duplicates instead. The watermark only ever
  advances, only through successfully applied scans, and never reaches
  below the immutable per-source baseline watermark. Source read errors
  and unsupported schemas fail closed (no partial ingest, no cursor
  movement).
- **Known completeness limitations** (honest boundaries, all
  documented in the reader modules):
  - A disable → re-enable gap is never backfilled: usage completed
    while collection was off is permanently absent from the plane (by
    policy — see the fresh-baseline rule above). The disable instant is
    not persisted, so the gap's extent is unknowable; per-source coverage
    reports it as incomplete/uncertain rather than complete.
  - A source that has never established a baseline (failure-only cursor,
    or no collection pass yet) has no known coverage. Its range coverage
    is uncertain; an empty result from it is missing history, never a
    confirmed zero.
  - Sources remain invisible to `sourceCoverage` until their first
    cursor exists. The daily projection remains anchored to the first
    global collection start; it is not re-anchored per source.
  - `sources` and `sourceCoverage` are in the same order within one
    response, but that order is not guaranteed across responses: the
    store iterates a `HashMap`. Consumers must join entries by source id.
  - For a source that baselined but has not completed a scan yet,
    `lastObservedAt` equals the baseline moment (pre-existing field
    semantics). Coverage wording treats it as "events after this instant
    may be missing", which stays true.
  - If the Tauri app-data directory cannot be resolved, the command falls
    back to a degenerate empty payload (`incompleteHistory: false`, empty
    `daily` / `sources` / `sourceCoverage`). The view renders no
    completeness claim there because the KPI block requires at least one
    provider row; this is a pre-existing degenerate path, not a coverage
    verdict.
  - Late commits ride a 10-minute lookback: a row committed more than
    10 minutes behind a watermark that has already moved past its
    stamp is not re-read. While a backlog is mid-drain the lookback
    sweep is skipped entirely; late rows then wait for the draining
    scan that follows, within the same 10-minute honesty boundary.
  - Permanently malformed rows (validation failures) are rejected and
    counted, then age out of the 10-minute band; they are never
    re-interpreted.
  - The OpenCode content address can collapse two genuinely distinct
    requests sharing the full accounting fingerprint (same
    millisecond, model, dimensions, cost) into one event.
  - The residual OpenCode fork-collision risk above; a SHA-256
    collision, negligible at realistic store sizes.

## Aggregation API

`get_usage_intelligence(query)` — `range: today | 7d | 30d` (+ the
webview's local `todayStartMs`, sanity-clamped server-side), grouped
`provider -> model`, summing input / cache read / cache write / output /
reasoning / total, with `collectionStartedAt`, `incompleteHistory`, the
`enabled` flag, per-source diagnostics (`disabled | collecting |
sourceAbsent | schemaUnsupported | readFailure | scanCeiling |
collectionPaused | ok`, plus the last observation time), and the additive
`sourceCoverage` array. `scanCeiling` on the row-count-bounded readers
means a bounded batch was committed and the remainder of an oversized
backlog continues on subsequent scans — progress, not a failure.
`collectionPaused` is the fail-closed enumeration pause (the Codex
rollout tree above its file ceiling or nested too deeply): the scan
stops before reading anything, so no prefix was committed and coverage
cannot be called complete. Phase 1 creates no notifications and no
predictions from this data, and a source failure can never degrade
quota-provider health.

### Per-source coverage (`sourceCoverage`)

`sourceCoverage` contains one verdict per existing source cursor — the
same source universe and order as `sources`. It is always present and may
be empty. Each entry has `source`, `coverage` (`complete | incomplete |
uncertain`), and optional `reason`, `coveredFrom` (ISO-8601), and
`lastObservedAt` (ISO-8601).

| Persisted condition | Coverage | Reason | Detail |
|---|---|---|---|
| monitoring disabled | `uncertain` | `monitoringDisabled` | Usage during the disabled period was never imported; the gap extent is unknown. |
| enabled, no baseline (failure-only cursor) | `uncertain` | `noBaseline` | Nothing has ever been collected; never imply zero usage. |
| enabled, baselined, pending re-baseline | `incomplete` | `pendingRebaseline` | Usage while collection was off is missing; no `coveredFrom`. |
| enabled, baselined, coverage start later than range start | `incomplete` | `lateBaseline` | Coverage is continuous from `coveredFrom = max(baselinedAt, baselineWatermark)`. |
| enabled, baselined at or before range start, state `ok` or `scanCeiling` | `complete` | — | `scanCeiling` is committed progress, not a gap. |
| enabled, baselined at or before range start, state `collectionPaused` | `uncertain` | `collectionPaused` | The scan stopped fail-closed before reading anything; `lastObservedAt` is included and later events may be missing. |
| enabled, baselined at or before range start, state `sourceAbsent`, `schemaUnsupported`, or `readFailure` | `uncertain` | the wire state string | `lastObservedAt` is included; later events may be missing. |

The coverage boundary is the later of the store-clock baseline instant
(`baselinedAt`) and the authoritative source-time watermark
(`baselineWatermark`). Taking the maximum is conservative under clock
skew in either direction: a source watermark ahead of the store clock
means the source itself had not reached the range start at baseline
time, even when `baselinedAt` says it had.

Equality is covered: a coverage start equal to `rangeStart` is
complete-eligible.
`incompleteHistory` is now the union of the original global-anchor test
(`collectionStartedAt > rangeStart`), the enabled-without-anchor case,
and any per-source verdict other than `complete`. A disabled plane with
no cursors and no anchor keeps `incompleteHistory = false`, preserving
the existing empty-state behavior.

The rules stay conservative: never-baselined sources are uncertain,
disabled periods are uncertain, paused collection is uncertain, and the
extent of a disable → re-enable gap is unknowable because the disable
instant is not persisted. No token, dedup, ingestion, retention,
persistence, cursor, or daily projection behavior changes.

### Daily activity projection (`daily`, added in the v0.9 Usage view)

The same `get_usage_intelligence` response additionally carries `daily`:
one cell per **local calendar day** (machine timezone, same
`chrono::Local` convention as the quota plane's heatmap) touched by the
selected range, bucketing the already-filtered in-range events — so the
daily event and token sums always reconcile with `eventsInRange` and
`totals`, and no second collection pass exists. Cell semantics:

- `observed: true` — the day holds recorded events; `totalTokens` is the
  day's reported-token sum (`0` is a measured, reported zero).
- `observed: false` — no recorded events that day; `totalTokens` is
  **absent on the wire** (missing is never serialized as a confirmed
  zero) and the frontend renders a hatched cell labeled "no recorded
  events — not confirmed zero usage".
- `beforeCollection: true` — the whole day precedes the local collection
  start; no observation can exist (no historical backfill), rendered and
  labeled distinctly from an in-window empty day.

Intensity is presentation policy on the frontend (`src/lib/usageDaily.ts`):
five levels scaled **relative** to the busiest observed day in the
selected range — explicitly not quota consumption, billing data, or total
account usage. Dev fixture URLs accept `?range=7d|30d` alongside
`?fixture=…` for visual QA.
