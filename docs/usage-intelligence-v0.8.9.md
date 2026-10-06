# Usage Intelligence — v0.8.9 Phase 1 (ZCode)

Status: implemented (Phase 1). Codex rollouts and OpenCode `message.data`
are adopted sources from discovery but are deliberately **not implemented
here**; they are follow-up work on the same event model.

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
