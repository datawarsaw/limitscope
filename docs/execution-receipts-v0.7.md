# Execution Receipts (LimitScope v0.7)

Status: Implementation Contract
Branch: `feature/v0.7-execution-receipts`
Base: `feature/v0.7-execution-provenance-core` (`629e678`)
Code: `src/lib/executionReceipt.ts`, `scripts/execution-receipt.mjs`
Depends on: `docs/execution-provenance-contract-v0.7.md`

This document defines the deterministic export layer for completed execution provenance runs. A
receipt is a pure text rendering of one `ExecutionProvenanceRun`: it is meant for HANDOFF metadata,
developer logs, research comparisons, and manual task accounting. It never claims exact task cost.

Deliberately out of scope: no UI surface, no Settings panel, no PDF, no HTML report generator, no
telemetry upload.

---

## 1. What a receipt is

A receipt restates what the provenance core already decided, in a stable byte-for-byte form:

- the execution bracket (`startedAt`, `endedAt`, duration),
- the optional attribution metadata that was actually supplied,
- the per-window observed quota movement or its explicit unavailability,
- the attribution confidence verdict.

A receipt adds no new measurement, no estimate, and no inference. If the core marked a comparison
incomparable, the receipt says so and exports no arithmetic delta.

---

## 2. JSON receipt schema (schemaVersion 1)

Two-space indent, LF newlines, exactly one trailing newline. Keys appear in the order below;
optional keys are omitted entirely when absent.

| Key | Type | Presence | Notes |
| :--- | :--- | :--- | :--- |
| `schemaVersion` | integer | always | `1` |
| `kind` | string | always | `limitscope-execution-receipt` |
| `runId` | string | when present | Safe token only (`^[A-Za-z0-9._:-]+$`). Omitted if unsafe. |
| `startedAt` | string | always | ISO-8601 UTC with milliseconds. Required for export. |
| `endedAt` | string | when present | ISO-8601 UTC with milliseconds. |
| `durationMs` | integer | when computable | `endedAt - startedAt`, omitted when negative or unknown. |
| `harness` | string | when present | e.g. `Codex` |
| `model` | string | when present | Copied from the run; never inferred. |
| `reasoningEffort` | string | when present | Copied from the run; never inferred. |
| `subscription` | string | when present | Explicit provider plan only; never inferred from the model name. |
| `account` | string | when present | Already-masked attribution only (e.g. `key:3456`). |
| `providerId` | string | when present | Safe token only. |
| `confidence` | string | always | `EXACT` \| `BOUNDED` \| `INFERRED` \| `UNAVAILABLE` |
| `resetCrossed` | boolean | always | From the run. |
| `comparable` | boolean | always | `false` whenever confidence is `UNAVAILABLE` or a reset was crossed. |
| `incomparabilityReason` | string | when not comparable | Deterministic reason string. |
| `windows` | array | always | Sorted ascending by `label`; may be empty. |
| `disclaimer` | string | always | `Observed quota delta during execution. Not exact task cost.` |

### Window record

| Key | Type | Notes |
| :--- | :--- | :--- |
| `label` | string | Quota window label, e.g. `5-hour`, `Weekly`. |
| `beforeUsedPercent` | number | Clamped to 0–100. Raw normalized value, not rounded. |
| `afterUsedPercent` | number | Clamped to 0–100. Raw normalized value, not rounded. |
| `deltaPoints` | number \| null | `null` unless the window is comparable. |
| `comparable` | boolean | `false` for any incomparable window. |
| `status` | string | `measured` \| `reset-crossed` \| `incomparable` |
| `reason` | string | Present only for incomparable windows. |

Raw before/after snapshots are not copied into the receipt: the receipt carries observed window
percentages and verdicts, not provider payloads.

### Example

```json
{
  "schemaVersion": 1,
  "kind": "limitscope-execution-receipt",
  "runId": "run-2026-09-30T0900Z-0001",
  "startedAt": "2026-09-30T09:00:00.000Z",
  "endedAt": "2026-09-30T09:18:00.000Z",
  "durationMs": 1080000,
  "harness": "Codex",
  "providerId": "openai-codex",
  "confidence": "BOUNDED",
  "resetCrossed": false,
  "comparable": true,
  "windows": [
    {
      "label": "5-hour",
      "beforeUsedPercent": 23,
      "afterUsedPercent": 28,
      "deltaPoints": 5,
      "comparable": true,
      "status": "measured"
    },
    {
      "label": "Weekly",
      "beforeUsedPercent": 38,
      "afterUsedPercent": 40,
      "deltaPoints": 2,
      "comparable": true,
      "status": "measured"
    }
  ],
  "disclaimer": "Observed quota delta during execution. Not exact task cost."
}
```

---

## 3. Markdown receipt contract

Blocks are separated by exactly one blank line. LF newlines, one trailing newline. The document is:

```text
LimitScope Execution Receipt

<header lines>

<one block per window, or "Quota observation unavailable">

<closing lines>
```

### Header lines (fixed order, optional lines omitted)

1. `Harness: <harness>` — when present
2. `Model: <model>` — when present
3. `Reasoning: <reasoningEffort>` — when present
4. `Plan: <subscription>` — when present
5. `Account: <masked account>` — when present
6. `Duration: <compact duration>` — when computable (`<1m`, `18m`, `1h 5m`, `2h`, `3d 4h`, `3d`)
7. `Confidence: <BOUNDED|INFERRED|UNAVAILABLE|EXACT>` — always last

### Window blocks (three lines each, sorted ascending by label)

Comparable window:

```text
5-hour
23% → 28%
Observed delta: +5 pp
```

Window affected by a reset crossing:

```text
Weekly
Reset occurred during execution
Delta unavailable
```

Otherwise incomparable window:

```text
5-hour
Comparison unavailable
Delta unavailable
```

### Closing lines

- comparable run: `Observed quota delta during execution.` then `Not exact task cost.`
- reset crossed: `Quota reset occurred during execution.` then `Not exact task cost.`
- otherwise: the run's `incomparabilityReason` (or `Quota observation unavailable.`) then `Not exact task cost.`

### Example

```text
LimitScope Execution Receipt

Harness: Codex
Duration: 18m
Confidence: BOUNDED

5-hour
23% → 28%
Observed delta: +5 pp

Weekly
38% → 40%
Observed delta: +2 pp

Observed quota delta during execution.
Not exact task cost.
```

---

## 4. Determinism pins

The same normalized run object must produce byte-identical output. Pinned decisions:

| Aspect | Pin |
| :--- | :--- |
| Window ordering | Ascending by `label` using code-unit comparison (locale-independent). `5-hour` sorts before `Weekly`. |
| Duplicate labels | Last occurrence wins; labels are the window key. |
| Percent formatting (Markdown) | `Math.round` to a whole percent, then `%`. |
| Delta formatting (Markdown) | `Math.round` to whole percentage points, `+N pp` / `-N pp` / `0 pp`; `Delta unavailable` when `null`. |
| Delta value (JSON) | Exact normalized number from the run, or `null`. Never rounded, never substituted arithmetic across a reset. |
| Duration formatting | Minutes floor; `<1m`, `Nm`, `Hh`, `Hh Mm`, `Dd`, `Dd Hh` (see `formatReceiptDuration`). |
| Timestamps | ISO-8601 UTC with milliseconds via `Date.toISOString()`. |
| Newlines | LF only, never CRLF. Exactly one trailing newline in both formats. |
| JSON shape | 2-space indent; fixed key order as documented above. |
| Omission | Absent optional fields are omitted, never rendered as `undefined`, `null`, or `unknown`. |
| Unicode | The arrow in Markdown is U+2192 (`→`); markup characters `&`, `<`, `>` are entity-escaped. |

Timestamps are normalized on export, so `2026-09-30T09:00:00Z` and
`2026-09-30T09:00:00.000Z` produce identical output.

---

## 5. File names

`receiptFileName(receipt, format)` returns `limitscope-run-<YYYY-MM-DDTHHMMZ>.md` or `.json`, derived
from `startedAt` in UTC. Examples: `limitscope-run-2026-09-30T0900Z.md`,
`limitscope-run-2026-09-30T0900Z.json`.

Names never contain account identifiers, model names, or run IDs, so they are safe to attach to
handoffs and logs. Because the stamp has minute resolution, two receipts started in the same UTC
minute resolve to the same name; callers that ingest both must add their own disambiguator.

---

## 6. Privacy

Rules enforced on export:

- Account attribution is only accepted if it is already masked and safe (`key:3456`, `xai:••••1234`).
  Full identifiers, emails, and credential shapes are dropped.
- Credential-shaped values are dropped from every exported field: JWT segments (`eyJ…`), `sk-` keys,
  `ghp_`/`gho_`/`github_pat_` tokens, PEM headers, `Bearer …` values, and anything containing
  `token`, `secret`, `password`, or `apikey`.
- Raw snapshots, prompts, responses, and store payloads are never exported.
- Optional metadata is omitted rather than partially redacted, so the receipt never implies a value
  it does not have.
- Control characters and whitespace runs in labels and metadata collapse to single spaces, so no
  value can inject a line or block into the Markdown receipt.

The library is a pure formatter: it performs no I/O beyond what the caller does with the returned
strings. The CLI reads one run JSON file and writes receipt files; it opens no network connection.

---

## 7. Reset crossing

When `resetCrossed` is true, every window exports `deltaPoints: null`, `comparable: false`,
`status: reset-crossed`, and Markdown renders `Reset occurred during execution` with
`Delta unavailable`. The misleading arithmetic delta (for example `-35 pp` from 38% down to 3%)
is never exported. This follows section 5 of the provenance contract.

---

## 8. Confidence values

Receipts always carry a confidence: `BOUNDED`, `INFERRED`, or `UNAVAILABLE` from production
generation, plus `EXACT` in the shared enum so a manually constructed fixture can be formatted.
Production generation rules are unchanged: LimitScope never auto-assigns `EXACT`.

---

## 9. Usage

```ts
import { exportExecutionReceipt } from "./lib/executionReceipt";

const json = exportExecutionReceipt(run, "json");
const markdown = exportExecutionReceipt(run, "markdown");
// { fileName: "limitscope-run-2026-09-30T0900Z.json", content: "..." } | undefined
```

Library surface: `createExecutionReceipt`, `formatReceiptJson`, `formatReceiptMarkdown`,
`formatReceiptDuration`, `receiptFileName`, `exportExecutionReceipt`.

CLI fixture exporter (mirrors the library; a parity test pins both to the same bytes):

```powershell
node scripts/provenance.mjs finish --snapshot after.json --out run.json
node scripts/execution-receipt.mjs --input run.json --format both --out-dir .\receipts
node scripts/execution-receipt.mjs --input run.json --format markdown --stdout
```

---

## 10. Future HANDOFF mapping (documented only)

This repository does not modify Agent Platform schemas, and nothing here depends on a schema change.
If a future HANDOFF wants to embed a receipt, the mapping is a straight read of the Markdown header
into the [Agent Platform Handoff Artifact Contract v1](C:/AI/agent-platform-state/HANDOFF_TEMPLATE.md)
`EXECUTION METADATA` block:

| Receipt field | Handoff field |
| :--- | :--- |
| `Harness:` | `HARNESS` |
| `Model:` | `MODEL` |
| `Reasoning:` | `REASONING / THINKING EFFORT` (otherwise `UNAVAILABLE`) |
| `Plan:` / `Account:` | supplementary context; no contract field today |
| `Confidence:` + window blocks | `EVIDENCE` (observation, explicitly not task cost) |
| `Duration:` | supporting detail for the same `EVIDENCE` entry |

`SESSION TYPE` is not derivable from a receipt and must come from the session itself. Receipts carry
no `SUBAGENTS` accounting, no token counts, and no timing beyond the execution bracket, so none of
those fields may be filled from a receipt. Because the Markdown is already flat text with fixed
ordering and LF newlines, an embed needs no transformation — only placement.

---

## 11. Test coverage

`src/lib/executionReceipt.test.ts` covers: basic JSON and Markdown, multi-window ordering, missing
model, missing reasoning, plan present (and never inferred from the model), reset crossing,
account unavailable, BOUNDED and INFERRED wording, UNAVAILABLE runs, special-character escaping,
byte-stable repeated and reordered output, deterministic file names, secret-shaped field removal,
and CLI/library byte parity.
