/**
 * Manual execution run workflow (LimitScope v0.7 UX layer).
 *
 * A deliberate, operator-driven bracket around one external AI coding run:
 * the operator presses Start, works in Codex (or another harness they
 * declare), and presses Finish. LimitScope then presents the observed quota
 * delta during that execution using the accepted v0.7 provenance core
 * (src/lib/provenance.ts) — never a second comparison implementation.
 *
 * Product boundaries baked in here:
 * - Only one run may be active at a time; a second start is refused with a
 *   bounded resolution (finish / discard / cancel), never an overwrite.
 * - Nothing is automated: no process detection, no prompt injection, no
 *   watching terminals, no inferring a start from quota movement. The
 *   harness is caller-declared and never presented as verified.
 * - The run model is the production `ExecutionProvenanceRun`. This module
 *   only adds lifecycle transitions, bounded local persistence (active-run
 *   recovery + a small recent-run history), and presentation helpers.
 * - Storage holds only safe normalized provenance: no prompt text, no
 *   response text, no source code, no terminal commands, no credentials.
 */

import {
  normalizeSnapshot,
  snapshotStaleness,
  sanitizeAccountIdentity,
  sanitizePlanType,
  startProvenanceRun,
  finishProvenanceRun,
  isValidTimestamp,
  clampPercent,
  type AttributionConfidence,
  type ExecutionProvenanceRun,
  type ExecutionQuotaSnapshot,
  type ProvenanceWindowDelta,
} from "./provenance";
import type { ProviderUsage } from "../types";

export const EXECUTION_RUNS_STORAGE_KEY = "limitscope.execution-runs.v1";
/** Bounded recent-run history: newest first, oldest dropped. */
export const EXECUTION_RUNS_MAX_RECENT = 20;

/**
 * The deliberately small harness model: only harnesses the product has
 * semantics for, plus "Other". A label is caller-declared context — it must
 * never create integration behavior or claim automatic verification.
 */
export const EXECUTION_HARNESSES = ["Codex", "ZCode", "OpenCode", "Other"] as const;
export type ExecutionHarness = (typeof EXECUTION_HARNESSES)[number];
export const DEFAULT_EXECUTION_HARNESS: ExecutionHarness = "Codex";

/** The whole execution-run state the app keeps. */
export type ExecutionRunsState = {
  /** The single manually tracked bracket currently in flight, if any. */
  activeRun: ExecutionProvenanceRun | null;
  /** Completed runs, newest first, bounded to EXECUTION_RUNS_MAX_RECENT. */
  recentRuns: ExecutionProvenanceRun[];
};

export const EMPTY_EXECUTION_RUNS_STATE: ExecutionRunsState = {
  activeRun: null,
  recentRuns: [],
};

export type StartRunRequest = {
  /** The provider usage whose current snapshot becomes the baseline. */
  usage: ProviderUsage | undefined;
  /** Caller-declared harness label; never presented as verified. */
  harness?: string;
  /** Optional, only when explicitly entered by the operator. Never inferred. */
  model?: string;
  reasoningEffort?: string;
  nowMs?: number;
};

export type StartRunOutcome =
  | { status: "started"; state: ExecutionRunsState; run: ExecutionProvenanceRun }
  /** A run is already active; the caller must offer a bounded resolution. */
  | { status: "conflict"; state: ExecutionRunsState; activeRun: ExecutionProvenanceRun }
  | { status: "no-usable-snapshot"; state: ExecutionRunsState; reason: string };

export type FinishRunOptions = {
  nowMs?: number;
};

export type FinishRunOutcome =
  | { status: "finished"; state: ExecutionRunsState; run: ExecutionProvenanceRun }
  | { status: "no-active-run"; state: ExecutionRunsState }
  | { status: "after-unavailable"; state: ExecutionRunsState; reason: string };

/**
 * Resolves the operator-selected provider usage into a usable start
 * baseline: a fresh, successful snapshot that actually carries quota
 * windows. Freshness here is the provenance core's own rule.
 */
export function resolveStartSnapshot(
  usage: ProviderUsage | undefined,
  nowMs: number,
): { ok: true; snapshot: ExecutionQuotaSnapshot } | { ok: false; reason: string } {
  const snapshot = usage === undefined ? undefined : normalizeSnapshot(usage);
  if (snapshot === undefined) {
    return { ok: false, reason: "No usable current provider snapshot." };
  }
  const staleness = snapshotStaleness(snapshot, nowMs);
  if (staleness.stale) {
    return {
      ok: false,
      reason: `No usable current provider snapshot (${staleness.reason ?? "snapshot is not fresh"}).`,
    };
  }
  return { ok: true, snapshot };
}

/** Starts a run. Refuses a second concurrent run instead of overwriting it. */
export function beginExecutionRun(
  state: ExecutionRunsState,
  request: StartRunRequest,
): StartRunOutcome {
  if (state.activeRun !== null) {
    return { status: "conflict", state, activeRun: state.activeRun };
  }
  const baseline = resolveStartSnapshot(request.usage, request.nowMs ?? Date.now());
  if (!baseline.ok) {
    return { status: "no-usable-snapshot", state, reason: baseline.reason };
  }
  const run = startProvenanceRun({
    before: baseline.snapshot,
    harness: normalizeHarness(request.harness),
    model: sanitizeOptionalText(request.model),
    reasoningEffort: sanitizeOptionalText(request.reasoningEffort),
    startedAt: new Date(request.nowMs ?? Date.now()).toISOString(),
  });
  return { status: "started", state: { ...state, activeRun: run }, run };
}

/**
 * Finishes the active run against the provider's current snapshot and files
 * the result into the recent history. The comparison itself — account
 * mismatch, provider mismatch, staleness, empty windows, reset crossing —
 * is entirely the provenance core's verdict; nothing is recomputed here.
 */
export function completeExecutionRun(
  state: ExecutionRunsState,
  usages: ProviderUsage[],
  options: FinishRunOptions = {},
): FinishRunOutcome {
  const activeRun = state.activeRun;
  if (activeRun === null) {
    return { status: "no-active-run", state };
  }
  // Finish always brackets the run's own provider, regardless of which
  // provider the dashboard currently has selected.
  const usage = usages.find((candidate) => candidate.id === activeRun.providerId);
  const afterSnapshot = usage === undefined ? undefined : normalizeSnapshot(usage);
  if (afterSnapshot === undefined) {
    return {
      status: "after-unavailable",
      state,
      reason: "After snapshot unavailable.",
    };
  }
  const nowMs = options.nowMs ?? Date.now();
  const finished = finishProvenanceRun(activeRun, afterSnapshot, {
    endedAt: new Date(nowMs).toISOString(),
    nowMs,
  });
  return {
    status: "finished",
    state: {
      ...state,
      activeRun: null,
      recentRuns: [finished, ...state.recentRuns].slice(0, EXECUTION_RUNS_MAX_RECENT),
    },
    run: finished,
  };
}

/** Discards the active run without recording anything. */
export function discardExecutionRun(state: ExecutionRunsState): ExecutionRunsState {
  if (state.activeRun === null) return state;
  return { ...state, activeRun: null };
}

// ---------------------------------------------------------------------------
// Bounded local persistence (active-run recovery + recent-run history).
// localStorage only — no daemon, no telemetry, no new Tauri commands.
// ---------------------------------------------------------------------------

function sanitizeOptionalText(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  if (trimmed === "" || trimmed.length > 64) return undefined;
  return trimmed;
}

function normalizeHarness(value: unknown): string {
  const trimmed = typeof value === "string" ? value.trim() : "";
  if (trimmed === "") return DEFAULT_EXECUTION_HARNESS;
  return trimmed.slice(0, 32);
}

const CONFIDENCE_LADDER: readonly AttributionConfidence[] = [
  "EXACT",
  "BOUNDED",
  "INFERRED",
  "UNAVAILABLE",
];

function parseConfidence(value: unknown): AttributionConfidence | undefined {
  return typeof value === "string" && (CONFIDENCE_LADDER as readonly string[]).includes(value)
    ? (value as AttributionConfidence)
    : undefined;
}

function parseRunId(value: unknown): string | undefined {
  return sanitizeOptionalText(value);
}

function parseWindowDelta(raw: unknown): ProvenanceWindowDelta | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const record = raw as Record<string, unknown>;
  const label = typeof record.label === "string" ? record.label.trim() : "";
  const before = typeof record.beforeUsedPercent === "number" ? record.beforeUsedPercent : NaN;
  const after = typeof record.afterUsedPercent === "number" ? record.afterUsedPercent : NaN;
  const points = typeof record.deltaPoints === "number" ? record.deltaPoints : NaN;
  if (label === "" || !Number.isFinite(before) || !Number.isFinite(after) || !Number.isFinite(points)) {
    return undefined;
  }
  return {
    label,
    beforeUsedPercent: clampPercent(before),
    afterUsedPercent: clampPercent(after),
    deltaPoints: points,
    comparable: record.comparable === true,
    ...(typeof record.reason === "string" && record.reason.trim() !== ""
      ? { reason: record.reason }
      : {}),
  };
}

/**
 * Defensive parse of a run we ourselves persisted. Snapshots are re-run
 * through the core's normalizer (which enforces the secret-shape and
 * masking rules); anything that fails validation is dropped, never shown.
 * Confidence EXACT is rejected outright: the store never produces it, so an
 * EXACT value here did not come from this app.
 */
function parsePersistedRun(raw: unknown): ExecutionProvenanceRun | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const record = raw as Record<string, unknown>;
  const runId = parseRunId(record.runId);
  if (runId === undefined || !isValidTimestamp(record.startedAt)) return undefined;
  const beforeSnapshot = normalizeSnapshot(record.beforeSnapshot ?? record.before);
  if (beforeSnapshot === undefined) return undefined;
  const confidence = parseConfidence(record.confidence ?? record.attributionConfidence);
  if (confidence === undefined || confidence === "EXACT") return undefined;

  const endedAtRaw = record.endedAt;
  const hasEndedAt = isValidTimestamp(endedAtRaw);
  const afterRaw = record.afterSnapshot ?? record.after;
  const afterSnapshot = afterRaw === undefined || afterRaw === null ? undefined : normalizeSnapshot(afterRaw);
  if (hasEndedAt && afterSnapshot === undefined) return undefined;
  if (!hasEndedAt && record.endedAt !== undefined) return undefined;

  const windows = Array.isArray(record.windows)
    ? record.windows.map(parseWindowDelta).filter((w): w is ProvenanceWindowDelta => w !== undefined)
    : Array.isArray(record.deltas)
      ? record.deltas.map(parseWindowDelta).filter((w): w is ProvenanceWindowDelta => w !== undefined)
      : [];

  return {
    runId,
    startedAt: new Date(record.startedAt as string).toISOString(),
    ...(hasEndedAt ? { endedAt: new Date(endedAtRaw as string).toISOString() } : {}),
    ...(sanitizeOptionalText(record.harness) !== undefined
      ? { harness: sanitizeOptionalText(record.harness) }
      : {}),
    ...(sanitizeOptionalText(record.model) !== undefined
      ? { model: sanitizeOptionalText(record.model) }
      : {}),
    ...(sanitizeOptionalText(record.reasoningEffort) !== undefined
      ? { reasoningEffort: sanitizeOptionalText(record.reasoningEffort) }
      : {}),
    ...(sanitizePlanType(record.subscription) !== undefined
      ? { subscription: sanitizePlanType(record.subscription) }
      : {}),
    ...(sanitizeAccountIdentity(record.account) !== undefined
      ? { account: sanitizeAccountIdentity(record.account) }
      : {}),
    beforeSnapshot,
    ...(afterSnapshot !== undefined ? { afterSnapshot } : {}),
    windows,
    confidence,
    resetCrossed: record.resetCrossed === true,
    comparable: record.comparable === true,
    ...(typeof record.incomparabilityReason === "string" && record.incomparabilityReason.trim() !== ""
      ? { incomparabilityReason: record.incomparabilityReason }
      : {}),
    providerId: beforeSnapshot.providerId,
    before: beforeSnapshot,
    ...(afterSnapshot !== undefined ? { after: afterSnapshot } : {}),
    deltas: windows,
    attributionConfidence: confidence,
  };
}

function parsePersistedState(raw: string | null): ExecutionRunsState {
  if (raw === null || raw.trim() === "") return EMPTY_EXECUTION_RUNS_STATE;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return EMPTY_EXECUTION_RUNS_STATE;
  }
  if (typeof parsed !== "object" || parsed === null) return EMPTY_EXECUTION_RUNS_STATE;
  const record = parsed as Record<string, unknown>;
  const activeRun =
    record.activeRun === null || record.activeRun === undefined
      ? null
      : (parsePersistedRun(record.activeRun) ?? null);
  if (record.activeRun != null && activeRun === null) {
    // A corrupt active bracket is dropped, not shown as a ghost run.
    return { ...EMPTY_EXECUTION_RUNS_STATE, activeRun: null };
  }
  const recentRuns = Array.isArray(record.recentRuns)
    ? record.recentRuns
        .map(parsePersistedRun)
        .filter((run): run is ExecutionProvenanceRun => run !== undefined && run.endedAt !== undefined)
        .slice(0, EXECUTION_RUNS_MAX_RECENT)
    : [];
  return { activeRun, recentRuns };
}

export function loadExecutionRunsState(storage: Pick<Storage, "getItem"> = localStorage): ExecutionRunsState {
  try {
    return parsePersistedState(storage.getItem(EXECUTION_RUNS_STORAGE_KEY));
  } catch {
    return EMPTY_EXECUTION_RUNS_STATE;
  }
}

export function saveExecutionRunsState(
  state: ExecutionRunsState,
  storage: Pick<Storage, "setItem"> = localStorage,
): void {
  try {
    storage.setItem(EXECUTION_RUNS_STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Persistence is best-effort; the workflow keeps working without it.
  }
}

export type ClearExecutionRunsOptions = {
  /** Explicit user confirmation to discard an active run in flight. */
  deliberateActiveConfirmation?: boolean;
};

export type ClearExecutionRunsResult = {
  state: ExecutionRunsState;
  cleared: boolean;
  activeRunPreserved: boolean;
};

/**
 * Clears the owned execution-runs store. Completed runs always clear; an
 * active run is removed only behind deliberate confirmation. Receipts that
 * were already exported are user artifacts and are not touched.
 */
export function clearExecutionRunsState(
  options: ClearExecutionRunsOptions = {},
  storage: Pick<Storage, "getItem" | "setItem"> = localStorage,
): ClearExecutionRunsResult {
  const current = loadExecutionRunsState(storage);
  const activeRunPreserved =
    current.activeRun !== null && options.deliberateActiveConfirmation !== true;
  const state: ExecutionRunsState = activeRunPreserved
    ? { ...EMPTY_EXECUTION_RUNS_STATE, activeRun: current.activeRun }
    : EMPTY_EXECUTION_RUNS_STATE;
  saveExecutionRunsState(state, storage);
  return { state, cleared: !activeRunPreserved, activeRunPreserved };
}

// ---------------------------------------------------------------------------
// Presentation helpers — pure projections for the panel, no new schemas.
// ---------------------------------------------------------------------------

/** Compact live elapsed label ("12m", "1h 03m") for the active-run state. */
export function formatRunElapsed(startedAt: string, nowMs: number): string {
  const started = Date.parse(startedAt);
  if (!Number.isFinite(started)) return "—";
  const totalMinutes = Math.floor(Math.max(0, nowMs - started) / 60000);
  if (totalMinutes < 1) return "<1m";
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, "0")}m`;
  return `${minutes}m`;
}

/** Compact finished-run duration ("19 min", "1h 5m", "<1 min"). */
export function formatRunDuration(startedAt: string, endedAt: string): string {
  const started = Date.parse(startedAt);
  const ended = Date.parse(endedAt);
  if (!Number.isFinite(started) || !Number.isFinite(ended)) return "—";
  const totalMinutes = Math.floor(Math.max(0, ended - started) / 60000);
  if (totalMinutes < 1) return "<1 min";
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours > 0) return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  return `${minutes} min`;
}

/**
 * The observed window delta as display copy. Rounding is honest: a nonzero
 * sub-point delta never silently reads as exactly 0.
 */
export function formatDeltaPoints(points: number): string {
  const rounded = Math.round(points);
  if (rounded === 0 && points !== 0) {
    return `${points > 0 ? "+" : "-"}<1 pp observed`;
  }
  return `${rounded > 0 ? "+" : ""}${rounded} pp observed`;
}

/**
 * Confidence presentation. Labels carry the state in text (never color
 * alone); the description is the on-demand explanation behind the "Why?"
 * toggle. EXACT has no presentation: the workflow never produces it.
 */
export const CONFIDENCE_PRESENTATION: Record<
  Exclude<AttributionConfidence, "EXACT">,
  { label: string; description: string }
> = {
  BOUNDED: {
    label: "Bounded",
    description:
      "Both quota snapshots were fresh, taken manually around the run on the same provider and account, and no quota reset fell inside the run. Concurrent activity elsewhere on the account is still part of the observed delta.",
  },
  INFERRED: {
    label: "Inferred",
    description:
      "At least one quota snapshot was stale or incomplete, so the delta is a weaker observation of what happened during the run.",
  },
  UNAVAILABLE: {
    label: "Unavailable",
    description:
      "A direct quota comparison was not possible — for example a provider error, an account change, or no usable quota windows — so no delta is claimed.",
  },
};

export function confidencePresentation(
  confidence: AttributionConfidence,
): { label: string; description: string } {
  return confidence === "EXACT" ? CONFIDENCE_PRESENTATION.UNAVAILABLE : CONFIDENCE_PRESENTATION[confidence];
}
