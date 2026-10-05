# Execution Provenance Contract (LimitScope v0.7 Core)

Status: Production Core Specification
Branch: `feature/v0.7-execution-provenance-core`
Base: `integration/v0.6-core-rehearsal` (`b0978b9`)
Maturity: Production Core Contract

This document formalizes the production execution provenance contract for LimitScope v0.7. It promotes the validated findings from `research/v0.7-execution-provenance` into production-grade core code without merging exploratory prototype clutter.

---

## 1. Core Principles & Attribution Ladder

Execution provenance associates bounded AI coding executions with observable quota snapshots without invasive process hooking, background daemons, or credential exposure.

### The Confidence Ladder

| Confidence | Meaning | Criteria |
| :--- | :--- | :--- |
| **`EXACT`** | Exact, verified task cost | **Disabled / Impossible in local monitors.** Requires a verifiable backend run/execution identifier binding directly into the provider quota ledger. LimitScope never auto-assigns `EXACT` and never invents synthetic execution identifiers. |
| **`BOUNDED`** | Honest observation window | Fresh manually-bracketed before/after snapshots of the same provider and account where no reset boundary was crossed. **This is the current expected ceiling.** |
| **`INFERRED`** | Degraded observation | Stale baseline snapshot (`capturedAt` older than max age threshold relative to `startedAt`), stale final snapshot, or unbracketed evaluation without explicit run context. |
| **`UNAVAILABLE`** | Incomparable | Provider error, provider mismatch, account mismatch, empty quota windows, or missing snapshot data. Direct quota comparison is forbidden. |

---

## 2. Canonical Run Model

The production run model represents active and completed execution brackets as a strongly typed, deterministic data structure (`ExecutionProvenanceRun` in `src/lib/provenance.ts`):

```typescript
export type ExecutionProvenanceRun = {
  runId: string;
  startedAt: string;
  endedAt?: string;
  harness?: string;
  model?: string;
  reasoningEffort?: string;
  /** Explicit subscription tier (e.g. "team", "plus", "pro") when reported by provider. Never guessed from model. */
  subscription?: string;
  /** Stable masked identity token (e.g. key:3456). Never a secret or full identifier. */
  account?: string;
  beforeSnapshot: ExecutionQuotaSnapshot;
  afterSnapshot?: ExecutionQuotaSnapshot;
  windows: ProvenanceWindowDelta[];
  confidence: AttributionConfidence;
  resetCrossed: boolean;
  comparable: boolean;
  incomparabilityReason?: string;
  providerId: string;
};
```

### Data Hygiene
- **No prompt text** is stored or accepted.
- **No response text** is stored or accepted.
- **No credential material** (tokens, API keys, raw JWTs) is stored or accepted.

---

## 3. Manual Bracket Workflow

Provenance uses an explicit start/finish bracket rather than automatic process sniffing.

### Architectural Invariants
- **No background daemon.**
- **No automatic process hooking** into unrelated terminal or editor processes.
- **Clean library API & CLI helper.**

### Workflow Lifecycle
1. **Start (`startProvenanceRun` / `scripts/provenance.mjs start`):**
   - Captures fresh baseline snapshot (`beforeSnapshot`).
   - Stamps `startedAt` timestamp.
   - Generates or binds `runId`.
   - Records optional metadata: `harness`, `model`, `reasoningEffort`, explicit `subscription`, and masked `account`.
   - In CLI mode, writes active run context to a local file (default: `.limitscope-provenance-run.json`).

2. **External Execution:**
   - The user or orchestrator runs an AI agent (Codex, Claude, etc.) externally.

3. **Finish (`finishProvenanceRun` / `scripts/provenance.mjs finish`):**
   - Captures fresh completion snapshot (`afterSnapshot`).
   - Stamps `endedAt` timestamp.
   - Computes window deltas, evaluates reset crossing, validates account identity continuity, and assigns attribution confidence.
   - Produces deterministic summary JSON and provenance footer text.

---

## 4. Quota-Delta Semantics & Wording

### Canonical Metric
The internal canonical storage metric is always **`usedPercent` (0–100)**. Remaining quota percentage is an optional display perspective only, ensuring independence from UI toggle states.

### Observed Delta vs Task Cost
- **Prohibited:** Stating or implying "task cost", e.g. "task cost: 6%" or "task used 6%".
- **Required:** Reporting **observed quota delta during execution**, e.g. "+6 percentage points observed during execution" or "50% remaining -> 44% remaining".
- **Mandatory Disclaimer:** Every footer output and user-facing presentation must include the explicit disclaimer:
  > *"Observed quota delta during execution (not exact task cost)."*

Concurrent background processes, asynchronous backend aggregation, and rounding preclude claiming exact per-task consumption.

---

## 5. Reset Crossing Semantics

When a quota window resets, quota drops sharply (e.g. 90% used -> 5% used). Treating this as a numeric delta would report a nonsensical negative consumption (-85 percentage points).

### Contract Rules
- If `resetAt` changes materially between snapshots, or a known reset boundary timestamp passes during the run (`startedAt <= resetAt <= endedAt`):
  - `resetCrossed` is set to `true`.
  - `comparable` is set to `false`.
  - All window `deltaPoints` are locked to `0`.
  - Window deltas are marked `comparable: false` with reason: *"Quota reset occurred during execution. Direct quota delta is not comparable."*
  - The footer explicitly reports: *"Quota reset occurred during execution."*

---

## 6. Account Isolation & Attribution

LimitScope v0.6 hardened provider identity with masked account tokens (`AccountAttribution.identity`, e.g. `key:3456` or `xai:••••1234`).

### Contract Rules
- **Cross-Account Protection:** If the before and after snapshots carry differing account identities, attribution confidence becomes **`UNAVAILABLE`** and the comparison is declared incomparable (`comparable: false`).
- **Unattributed Providers:** Providers that cannot prove account identity (or legacy unhardened sources) report both snapshots without identity; they remain eligible for `BOUNDED` confidence if fresh and matched.
- **Never compare cross-account percentages.**

---

## 7. Plan Type / Subscription Contract

The Rust backend (`src-tauri/src/codex.rs`) queries ChatGPT usage and receives `plan_type`. Historically, the frontend bridge dropped this field.

### Safe Plan Attribution
- `src-tauri/src/runtime.rs` now preserves `plan_type` from `CodexUsage` through `ProviderUsageDto.plan_type` (serialized as `planType`).
- `src/types.ts` exposes `planType?: string` on `ProviderUsage`.
- **Allowed values:** Only explicit provider values (`team`, `plus`, `pro`, `enterprise`, etc.).
- **Prohibited:** Inferring or deriving subscription tiers from model names (e.g. model "GPT-6 Pro" must never infer subscription "pro").
- Values are sanitized, trimmed, lowercased, and checked for secret shapes.

---

## 8. Freshness & Staleness Rules

Freshness is evaluated independently for each boundary:
- **Before Snapshot:** Evaluated relative to **`startedAt`**. A long-running execution (e.g. 30 minutes) does **not** retroactively stale a baseline that was fresh when the execution began.
- **After Snapshot:** Evaluated relative to **`endedAt` / evaluation time (`nowMs`)**.
- **Threshold:** Default freshness window is 15 minutes (`PROVENANCE_DEFAULT_MAX_SNAPSHOT_AGE_MS`). Future timestamps exceeding 5 minutes skew are rejected as stale.

---

## 9. Deterministic Footer Formatter

The provenance footer is formatted deterministically by `formatProvenanceFooter`:
- **Stable Ordering:** Quota windows are sorted alphabetically by label (e.g. "5-hour" before "Weekly").
- **No Secrets:** Account identities are masked (`key:3456`); credentials never appear.
- **Omission of Unknowns:** If `model`, `reasoningEffort`, or `subscription` are omitted/unknown, their corresponding lines are omitted entirely (no "Model: unknown").
- **Explicit Confidence:** Always concludes with `Attribution: bounded`, `Attribution: inferred`, or `Attribution: unavailable`.
- **Deterministic Disclaimers:** Explicitly distinguishes comparable deltas from reset or account incomparability.

---

## 10. Persistence Contract

- **Default:** Run objects are ephemeral in-memory entities.
- **CLI Helper:** Persists active run state in local-first `.limitscope-provenance-run.json` during the execution interval. Completed runs can optionally be exported to caller-specified JSON files via `--out`.
- **No Central Telemetry:** LimitScope does not build, maintain, or transmit to a telemetry database.
- **Boundaries:** Persisted files must never contain prompt text, response text, or raw credentials.

---

## 11. Known Limitations & Edge Cases

1. **Concurrent Usage Noise:**
   Local monitor snapshots reflect account-wide usage. Concurrent work in the browser, mobile apps, or other IDE windows during a run alters the observed delta. This is why attribution is bounded, not exact.
2. **Upstream Latency:**
   Quota backends may update usage asynchronously with seconds to minutes of delay.
3. **Integer Quantization:**
   Percentages are reported as whole or rounded floats. Sub-percent executions may read as 0 pp delta.
4. **Resets Masking Usage:**
   If a reset occurs mid-run, usage before the reset cannot be reliably measured. The contract conservatively treats all such runs as incomparable.
