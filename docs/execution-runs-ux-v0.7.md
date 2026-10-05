# Execution Runs UX (LimitScope v0.7)

Status: Implemented workflow contract
Branch: `feature/v0.7-execution-runs-ux`
Base: `feature/v0.7-execution-provenance-core` (`629e678`)
Maturity: v0.7 initial implementation

This document records the product contract for the first user-facing execution
runs workflow. It builds directly on the accepted provenance core
(`docs/execution-provenance-contract-v0.7.md`, `src/lib/provenance.ts`) and
adds **no** new attribution math: every delta, confidence verdict, and
incomparability reason is produced by the core; the UI is a presentation
projection only.

---

## 1. Product goal and stance

Start run → do work in Codex (or another declared harness) → Finish run →
see what changed in quota during that execution.

The workflow remains **manual and bounded**:

- LimitScope never claims "this task cost X". Every delta is labeled
  **"Observed quota delta during execution (not exact task cost)"**, and that
  disclaimer is always rendered alongside any delta (comparable or not).
- Nothing is automated. No process detection, no terminal watching, no prompt
  injection, no shell hooks, no editor-state scraping, and no inferring that a
  task started because quota moved. The operator presses Start; the operator
  presses Finish.
- `EXACT` exists only in the type system. No lifecycle transition can produce
  it (pinned by tests at the store and UI level); a forged persisted `EXACT`
  entry is dropped at load time.

## 2. App surface

Option A from the product brief: a compact **Run** action in the main toolbar
(`src/App.tsx`, header) plus a small panel that opens under the header
(`src/components/dashboard/ExecutionRunPanel.tsx`). No new navigation section;
the v0.6 dashboard is untouched. The floating bar deliberately carries **no**
execution-run controls — it stays quota-glance focused.

The toolbar button reflects run state in text:

- idle: `Run`
- active: `● 12m` (elapsed time; text-first, the pulsing dot is decorative)

## 3. Manual lifecycle

### Start

The panel's start form captures, at press time:

- `runId` (generated, `run-<timestamp>-<rand>`)
- `startedAt`
- the selected provider's **current normalized snapshot** (through the core's
  `normalizeSnapshot`, so masking and sanitization rules apply)
- masked account identity (`key:3456` style) and explicit `planType` when the
  provider reports them — never guessed
- the caller-declared harness and optional metadata

A start is refused with `Cannot start: No usable current provider snapshot`
when there is no selected provider, or the snapshot is errored, empty, or
older than the provenance freshness threshold (`resolveStartSnapshot`). A
stale baseline is therefore refused up front, which keeps `BOUNDED`
achievable for every run that does start.

The form offers an inline `Refresh` action (the app's normal quota refresh)
when the baseline is not usable.

### Harness selection

A deliberately small model: **Codex · ZCode · OpenCode · Other**
(`EXECUTION_HARNESSES` in `src/lib/executionRuns.ts`). Default is `Codex`
(product context), and the UI says plainly: *"The harness label is declared
by you; LimitScope does not detect or verify it."* A label creates no
integration behavior.

### Optional metadata

`model` and `reasoning effort` are free-text, visually secondary (collapsed
under "Optional details"), recorded **only when explicitly typed**, never
auto-inferred, and omitted entirely when blank.

### One active run

Only one manually tracked run may be active. A start while a run exists
returns a conflict; the panel offers the bounded resolution:

- **Finish current run**
- **Discard current run**
- **Cancel**

The active run is never silently overwritten. The guard also holds across
app windows: every transition re-reads the persisted state first
(`useExecutionRuns`), so a second window gets the conflict resolution instead
of clobbering a run it did not see.

### Finish

Finish captures `endedAt` plus the run provider's current snapshot and
invokes the accepted core comparison (`finishProvenanceRun`). Account
mismatch, provider mismatch, stale snapshots, empty windows, and reset
crossing are all the core's verdicts — the UI does not duplicate comparison
logic.

- After snapshot missing entirely (provider gone from the runtime list):
  `After snapshot unavailable. Refresh the provider and finish again, or
  discard the run.` The bracket stays active — nothing is lost.
- After snapshot present but errored/empty: the run **finishes** with
  `UNAVAILABLE` confidence and the core's reason — the confidence contract
  instead of a generic failure.

Finish always brackets the **run's own provider**, regardless of which
provider the dashboard currently has selected; it never substitutes another
provider (pinned by tests).

## 4. Result presentation

The "Execution summary" card shows:

- harness, optional model/effort, `started → ended`, duration, provider,
  masked account
- one row per quota window: `23% → 28%  +5 pp observed` (rounding is honest:
  a nonzero sub-point delta reads `+<1 pp observed`, never a false `0`)
- reset crossing: `Reset occurred during this run — delta not comparable`
  per window; never `90% → 5% = -85 pp`
- account change: the core's banner `Account changed during execution. Direct
  quota delta is not comparable.` with zeroed deltas; percentages are never
  compared
- explicit confidence chip: **Bounded / Inferred / Unavailable** (text, not
  color alone), with a short on-demand explanation behind `Why?`
- the mandatory disclaimer line

The last completed run's summary stays available in the panel (derived from
the recent-run history — no parallel state), and every completed run appears
in **Recent runs (≤ 20)**.

## 5. Active-run recovery

State is persisted to `localStorage` under
`limitscope.execution-runs.v1` on every change. After an app restart with an
active run, the panel shows the same `Running · Xm / Started HH:MM` card with
`Finish` and `Discard`. No daemon, no timers, no background process —
recovery is a load-time projection. Persisted payloads are parsed
defensively: snapshots re-run through the core normalizer, corrupt or forged
entries (including `EXACT`) are dropped, and the recent list is truncated to
the cap.

## 6. Recent-run history

Implemented (lightweight, same storage as recovery): the last **20**
completed runs, newest first. Stored content is safe normalized provenance
only — window labels, percentages, reset timestamps, masked identities,
explicit plan types, confidence. **No prompt text, no response text, no
source code, no terminal commands, no raw credentials** (pinned by a
persistence test that rejects secret-shaped material).

## 7. Privacy boundary

The workflow stores nothing the providers layer does not already display:
masked account tokens, explicit plan types, quota percentages. Optional
model/effort strings are operator-typed, trimmed, length-capped, and kept
local. Nothing leaves the device; there is no telemetry integration here, and
`prototype/v0.7-harness-telemetry` remains untouched (JSONL is never read).

## 8. Usage integration boundary

Completed runs live in the store (`ExecutionRunsState.recentRuns`,
`useExecutionRuns`) behind a clean API, so
`feature/v0.7-usage-analytics-backend` can consume them later without schema
changes. That integration is intentionally **not** built here. The run model
is the production `ExecutionProvenanceRun` — the UI adds no second run/delta
schema.

## 9. Tray

Evaluated and **declined** for v0.7: starting from the tray is ambiguous
(which provider should the run target — the dashboard selection is not
visible or confirmable from the tray?), and a second start from the tray
needs the resolution UI that lives in the main window. Per the brief, tray
integration is optional and only adopted if unambiguous; no Rust or tray code
changed in this task.

## 10. Accessibility

- All primary actions are native `button`/`select`/`input` elements — no
  hover-only affordances.
- Opening the panel moves focus to its heading; `Escape` or the close button
  closes it and focus returns to the toolbar Run button.
- After Start, focus moves to `Finish run`; after Finish, focus moves to the
  summary heading. A polite live region announces started/finished/discarded.
- Status never relies on color alone: the active state is the text
  `Running · Xm` (dot is decorative), and confidence/reset/account states are
  text labels and chips.
- Pinned by keyboard tests: Enter opens and starts, Tab traverses, Escape
  closes with sensible focus return.

## 11. Error handling summary

| Situation | Presentation |
| :--- | :--- |
| No usable baseline at start | `Cannot start: No usable current provider snapshot` + Refresh |
| Provider missing at finish | `After snapshot unavailable` — run stays active |
| Errored/empty snapshot at finish | Run finishes `UNAVAILABLE` with the core's reason |
| Stale baseline/after | `INFERRED` chip + explanation |
| Reset crossing | Per-window `Reset occurred during this run — delta not comparable` |
| Account change | Banner, zeroed deltas, `UNAVAILABLE` |
| Second start | Finish current / Discard current / Cancel |

## 12. Where the code lives

- `src/lib/executionRuns.ts` — pure lifecycle store, persistence, presentation helpers
- `src/hooks/useExecutionRuns.ts` — React binding (transitions re-read persisted state)
- `src/components/dashboard/ExecutionRunPanel.tsx` — the panel and summary card
- `src/App.tsx` — toolbar Run action and panel mount
- `src/styles.css` — execution-run styles (design tokens only)
- `src/lib/executionRuns.test.ts`, `src/App.executionRuns.test.tsx` — lifecycle and UI coverage
