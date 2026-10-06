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
- Cursor: `completed_at` watermark + 10-minute lookback, ordered by
  `(completed_at, id)`, inclusive at the floor; id-dedup absorbs re-reads.
  An immutable `baselineWatermarkMs` anchors the backlog boundary — the
  lookback never reaches below it.
- Completion predicate: `status = 'completed' AND completed_at IS NOT NULL`.
- Fail-closed rows (rejected and counted, never guessed): blank
  provider/model, implausible timestamps, negative or NULL dimensions,
  `cache read + cache write > input`, `reasoning > output`, or
  `computed_total_tokens != input + output` (the verified live invariant).
- Scan ceiling: 20,000 rows per window; an over-ceiling window is
  abandoned whole (`scanCeiling`), cursor unmoved — nothing silently
  skipped.

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
- Cursor: `time_updated` — the row's last-write stamp — with a
  10-minute lookback, ordered by `(time_updated, id)`, floored at the
  immutable baseline watermark. Completions, late usage writes, and
  fork copies all bump it, so the window catches every newly-committed
  accounting state regardless of request age. The watermark advances
  through accepted rows and through all-zero rows dispositioned as
  usage-not-reported; malformed rows never advance it.
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
- Scan ceiling: 20,000 rows per window, abandoned whole on overflow
  (`scanCeiling`), cursor unmoved.

## Local-data controls (docs/local-data-controls-v0.7.md §14)

- **Opt-in first**: the persisted `enabled` flag lives in the owned store;
  while false the collector returns before resolving the source path — no
  stat, no existence probe, no open, no copy, no schema inspection
  (unit-proven with a counting resolver; runtime-cycle-proven the same way).
- **Backlog skip**: the first successful observation establishes the
  baseline at `MAX(completed_at) + 1` and imports nothing. The +1 keeps
  the inclusive incremental window from re-admitting the boundary row.
- **Read-only foreign ownership**: see the snapshot strategy above; the
  live proof asserts whole-file byte identity of the main database.
- **Bounded retention**: 35 days by `eventAt` plus a 200k event hard cap;
  only LimitScope-owned events are ever deleted.
- **Owned clear** (`clear_usage_intelligence`): clears events, cursors,
  and the disclosure anchor, preserves the opt-in flag, and re-baselines
  on the next collection — pre-clear usage never reappears.
- **No silent cursor skip**: cursor movement happens only through
  successfully applied scans; failures leave it in place.

## Aggregation API

`get_usage_intelligence(query)` — `range: today | 7d | 30d` (+ the
webview's local `todayStartMs`, sanity-clamped server-side), grouped
`provider -> model`, summing input / cache read / cache write / output /
reasoning / total, with `collectionStartedAt`, `incompleteHistory`, the
`enabled` flag, and per-source diagnostics (`disabled | collecting |
sourceAbsent | schemaUnsupported | readFailure | scanCeiling | ok`, plus
the last observation time). Phase 1 creates no notifications and no
predictions from this data, and a source failure can never degrade
quota-provider health.
