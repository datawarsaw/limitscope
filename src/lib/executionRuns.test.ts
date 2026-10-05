import { describe, expect, it } from "vitest";
import {
  beginExecutionRun,
  clearExecutionRunsState,
  completeExecutionRun,
  discardExecutionRun,
  EMPTY_EXECUTION_RUNS_STATE,
  EXECUTION_RUNS_MAX_RECENT,
  formatDeltaPoints,
  formatRunDuration,
  formatRunElapsed,
  loadExecutionRunsState,
  resolveStartSnapshot,
  saveExecutionRunsState,
  type ExecutionRunsState,
  type StartRunOutcome,
} from "./executionRuns";
import { finishProvenanceRun, startProvenanceRun } from "./provenance";
import type { ProviderUsage } from "../types";

const NOW_MS = Date.parse("2026-09-30T12:00:00Z");
const BASELINE_AT = "2026-09-30T11:58:00Z";
const RESET_FUTURE = "2026-09-30T18:00:00Z";

function codexUsage(overrides: Partial<ProviderUsage> = {}): ProviderUsage {
  return {
    id: "openai-codex",
    name: "OpenAI / Codex",
    status: "ok",
    health: "live",
    checkedAt: BASELINE_AT,
    limits: [
      { label: "5-hour", usedPercent: 23, resetAt: RESET_FUTURE },
      { label: "Weekly", usedPercent: 38, resetAt: "2026-10-05T00:00:00Z" },
    ],
    account: { label: "key:3456", identity: "key:3456" },
    planType: "team",
    ...overrides,
  };
}

function stateWithActiveRun(usage: ProviderUsage = codexUsage()): {
  state: ExecutionRunsState;
  outcome: StartRunOutcome & { status: "started" };
} {
  const outcome = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
    usage,
    harness: "Codex",
    nowMs: NOW_MS,
  });
  if (outcome.status !== "started") throw new Error("fixture failed to start");
  return { state: outcome.state, outcome };
}

describe("execution runs: start", () => {
  it("starts a run capturing runId, startedAt, snapshot, account, plan, harness (case 1)", () => {
    const { state, outcome } = stateWithActiveRun();
    expect(state.activeRun).not.toBeNull();
    const run = outcome.run;
    expect(run.runId).toMatch(/^run-/);
    expect(run.startedAt).toBe(new Date(NOW_MS).toISOString());
    expect(run.harness).toBe("Codex");
    expect(run.providerId).toBe("openai-codex");
    expect(run.beforeSnapshot.providerId).toBe("openai-codex");
    expect(run.beforeSnapshot.accountIdentity).toBe("key:3456");
    expect(run.beforeSnapshot.planType).toBe("team");
    expect(run.beforeSnapshot.windows.map((w) => w.label)).toEqual(["5-hour", "Weekly"]);
    expect(state.recentRuns).toHaveLength(0);
  });

  it("refuses a start when there is no usable current snapshot", () => {
    const outcome = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: undefined,
      nowMs: NOW_MS,
    });
    expect(outcome.status).toBe("no-usable-snapshot");
    if (outcome.status === "no-usable-snapshot") {
      expect(outcome.reason).toContain("No usable current provider snapshot");
    }
  });

  it("refuses a start from an errored or empty provider snapshot", () => {
    const errored = codexUsage({ status: "error", health: "error", limits: [] });
    expect(resolveStartSnapshot(errored, NOW_MS).ok).toBe(false);
    const old = codexUsage({ checkedAt: "2026-09-30T09:00:00Z" });
    expect(resolveStartSnapshot(old, NOW_MS).ok).toBe(false);
  });
});

describe("execution runs: single active run", () => {
  it("a second start conflicts instead of overwriting (case 2)", () => {
    const { state, outcome } = stateWithActiveRun();
    const second = beginExecutionRun(state, { usage: codexUsage(), nowMs: NOW_MS });
    expect(second.status).toBe("conflict");
    if (second.status === "conflict") {
      expect(second.activeRun.runId).toBe(outcome.run.runId);
    }
    expect(state.activeRun?.runId).toBe(outcome.run.runId);
  });

  it("cancelling a second start leaves the original run untouched (case 3)", () => {
    const { state, outcome } = stateWithActiveRun();
    const second = beginExecutionRun(state, { usage: codexUsage(), nowMs: NOW_MS });
    expect(second.status).toBe("conflict");
    // The conflict outcome carries the unchanged state; the operator's
    // Cancel is exactly "keep this".
    expect(second.state.activeRun?.runId).toBe(outcome.run.runId);
    expect(second.state.recentRuns).toHaveLength(0);
  });
});

describe("execution runs: discard", () => {
  it("discards the active run without recording anything (case 4)", () => {
    const { state, outcome } = stateWithActiveRun();
    const next = discardExecutionRun(state);
    expect(next.activeRun).toBeNull();
    expect(next.recentRuns).toHaveLength(0);
    expect(state.activeRun?.runId).toBe(outcome.run.runId);
  });

  it("discard of a recovered run clears it and persists cleared state (case 13)", () => {
    const { state } = stateWithActiveRun();
    const memory = memoryStorage();
    saveExecutionRunsState(state, memory);
    const recovered = loadExecutionRunsState(memory);
    expect(recovered.activeRun).not.toBeNull();
    const cleared = discardExecutionRun(recovered);
    expect(cleared.activeRun).toBeNull();
    saveExecutionRunsState(cleared, memory);
    expect(loadExecutionRunsState(memory).activeRun).toBeNull();
  });
});

describe("execution runs: finish", () => {
  it("finishes a normal run with fresh snapshots -> BOUNDED (cases 5, 6)", () => {
    const { state } = stateWithActiveRun();
    const after = codexUsage({
      checkedAt: "2026-09-30T12:19:00Z",
      limits: [
        { label: "5-hour", usedPercent: 28, resetAt: RESET_FUTURE },
        { label: "Weekly", usedPercent: 40, resetAt: "2026-10-05T00:00:00Z" },
      ],
    });
    const outcome = completeExecutionRun(state, [after], { nowMs: Date.parse("2026-09-30T12:19:30Z") });
    expect(outcome.status).toBe("finished");
    if (outcome.status !== "finished") return;
    expect(outcome.state.activeRun).toBeNull();
    expect(outcome.state.recentRuns[0]?.runId).toBe(outcome.run.runId);
    expect(outcome.run.confidence).toBe("BOUNDED");
    expect(outcome.run.comparable).toBe(true);
    expect(outcome.run.endedAt).toBe(new Date(Date.parse("2026-09-30T12:19:30Z")).toISOString());
    const fiveHour = outcome.run.windows.find((w) => w.label === "5-hour");
    expect(fiveHour?.deltaPoints).toBe(5);
    expect(fiveHour?.comparable).toBe(true);
  });

  it("finish without an active run is a no-op", () => {
    const outcome = completeExecutionRun(EMPTY_EXECUTION_RUNS_STATE, [codexUsage()], { nowMs: NOW_MS });
    expect(outcome.status).toBe("no-active-run");
  });

  it("after snapshot unavailable keeps the run active (case 11a)", () => {
    const { state, outcome } = stateWithActiveRun();
    const gone = completeExecutionRun(state, [], { nowMs: NOW_MS });
    expect(gone.status).toBe("after-unavailable");
    if (gone.status === "after-unavailable") {
      expect(gone.reason).toContain("After snapshot unavailable");
    }
    // The bracket is not lost: the operator can refresh and finish again.
    expect(gone.state.activeRun?.runId).toBe(outcome.run.runId);
  });

  it("failed after snapshot finishes with UNAVAILABLE confidence (case 11b)", () => {
    const { state } = stateWithActiveRun();
    const failed = codexUsage({
      checkedAt: "2026-09-30T12:19:00Z",
      status: "error",
      health: "error",
      limits: [],
    });
    const outcome = completeExecutionRun(state, [failed], { nowMs: NOW_MS });
    expect(outcome.status).toBe("finished");
    if (outcome.status !== "finished") return;
    expect(outcome.run.confidence).toBe("UNAVAILABLE");
    expect(outcome.run.comparable).toBe(false);
    expect(outcome.run.incomparabilityReason).toContain("Quota unavailable");
    expect(outcome.run.windows.every((w) => w.deltaPoints === 0 && !w.comparable)).toBe(true);
  });

  it("never substitutes a different provider at finish (case 8a)", () => {
    const { state } = stateWithActiveRun();
    const otherProvider = codexUsage({
      id: "zai",
      name: "Z.ai",
      checkedAt: "2026-09-30T12:19:00Z",
    });
    const outcome = completeExecutionRun(state, [otherProvider], { nowMs: NOW_MS });
    expect(outcome.status).toBe("after-unavailable");
  });

  it("a provider change between snapshots is refused by the delegated comparison (case 8b)", () => {
    const before = codexUsage();
    const started = startProvenanceRun({ before: normalizeOf(before), harness: "Codex" });
    const after = codexUsage({ id: "zai", name: "Z.ai", checkedAt: "2026-09-30T12:19:00Z" });
    const finished = finishProvenanceRun(started, normalizeOf(after), {
      endedAt: new Date(NOW_MS).toISOString(),
      nowMs: NOW_MS,
    });
    expect(finished.confidence).toBe("UNAVAILABLE");
    expect(finished.comparable).toBe(false);
    expect(finished.incomparabilityReason).toContain("Provider changed");
  });
});

describe("execution runs: bounded states", () => {
  it("account change during the run -> UNAVAILABLE, no percentages compared (case 7)", () => {
    const { state } = stateWithActiveRun();
    const switched = codexUsage({
      checkedAt: "2026-09-30T12:19:00Z",
      account: { label: "key:9999", identity: "key:9999" },
    });
    const outcome = completeExecutionRun(state, [switched], { nowMs: NOW_MS });
    expect(outcome.status).toBe("finished");
    if (outcome.status !== "finished") return;
    expect(outcome.run.confidence).toBe("UNAVAILABLE");
    expect(outcome.run.comparable).toBe(false);
    expect(outcome.run.incomparabilityReason).toContain("Account changed");
    expect(outcome.run.windows.every((w) => w.deltaPoints === 0)).toBe(true);
  });

  it("reset crossing during the run -> incomparable, never a negative delta (case 9)", () => {
    const { state } = stateWithActiveRun();
    const afterReset = codexUsage({
      checkedAt: "2026-09-30T12:19:00Z",
      limits: [
        // The reset boundary moved: usage dropped 23 -> 4 across a reset.
        { label: "5-hour", usedPercent: 4, resetAt: "2026-09-30T19:00:00Z" },
        { label: "Weekly", usedPercent: 40, resetAt: "2026-10-05T00:00:00Z" },
      ],
    });
    const outcome = completeExecutionRun(state, [afterReset], { nowMs: NOW_MS });
    expect(outcome.status).toBe("finished");
    if (outcome.status !== "finished") return;
    expect(outcome.run.resetCrossed).toBe(true);
    expect(outcome.run.comparable).toBe(false);
    expect(outcome.run.incomparabilityReason).toContain("reset");
    const fiveHour = outcome.run.windows.find((w) => w.label === "5-hour");
    expect(fiveHour?.deltaPoints).toBe(0);
    expect(fiveHour?.comparable).toBe(false);
  });

  it("a stale baseline is refused at start so bounded stays achievable (case 10)", () => {
    const stale = codexUsage({ checkedAt: "2026-09-30T09:00:00Z" });
    const resolution = resolveStartSnapshot(stale, NOW_MS);
    expect(resolution.ok).toBe(false);
    const outcome = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: stale,
      nowMs: NOW_MS,
    });
    expect(outcome.status).toBe("no-usable-snapshot");
  });

  it("a degraded stale baseline still yields INFERRED, never EXACT, in the delegated core (case 10b)", () => {
    const staleBefore = normalizeOf(
      codexUsage({ checkedAt: "2026-09-30T09:00:00Z" }),
    );
    const started = startProvenanceRun({
      before: staleBefore,
      harness: "Codex",
      startedAt: "2026-09-30T09:05:00Z",
    });
    const finished = finishProvenanceRun(
      started,
      normalizeOf(codexUsage({ checkedAt: "2026-09-30T09:30:00Z" })),
      { endedAt: new Date(NOW_MS).toISOString(), nowMs: Date.parse("2026-09-30T12:00:00Z") },
    );
    expect(finished.confidence).toBe("INFERRED");
  });
});

describe("execution runs: optional metadata", () => {
  it("model omitted stays omitted (case 14)", () => {
    const outcome = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: codexUsage(),
      harness: "Codex",
      nowMs: NOW_MS,
    });
    expect(outcome.status).toBe("started");
    if (outcome.status !== "started") return;
    expect(outcome.run.model).toBeUndefined();
  });

  it("reasoning effort omitted stays omitted; whitespace-only input is dropped (case 15)", () => {
    const outcome = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: codexUsage(),
      harness: "OpenCode",
      model: "   ",
      reasoningEffort: "  ",
      nowMs: NOW_MS,
    });
    expect(outcome.status).toBe("started");
    if (outcome.status !== "started") return;
    expect(outcome.run.model).toBeUndefined();
    expect(outcome.run.reasoningEffort).toBeUndefined();
    expect(outcome.run.harness).toBe("OpenCode");
  });

  it("explicitly entered model and reasoning effort are recorded verbatim", () => {
    const outcome = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: codexUsage(),
      harness: "Codex",
      model: "GPT-6 Sol",
      reasoningEffort: "high",
      nowMs: NOW_MS,
    });
    if (outcome.status !== "started") throw new Error("start failed");
    expect(outcome.run.model).toBe("GPT-6 Sol");
    expect(outcome.run.reasoningEffort).toBe("high");
  });
});

describe("execution runs: no automatic EXACT", () => {
  it("no lifecycle transition ever produces EXACT confidence (case 17)", () => {
    const { state } = stateWithActiveRun();
    const bestCase = completeExecutionRun(
      state,
      [codexUsage({ checkedAt: "2026-09-30T12:19:00Z" })],
      { nowMs: Date.parse("2026-09-30T12:19:30Z") },
    );
    expect(bestCase.status).toBe("finished");
    if (bestCase.status !== "finished") return;
    expect(bestCase.run.confidence).not.toBe("EXACT");
    expect(bestCase.run.confidence).toBe("BOUNDED");
  });
});

describe("execution runs: recovery persistence", () => {
  it("clears completed runs but preserves an active run without explicit confirmation", () => {
    const active = stateWithActiveRun();
    const finished = completeExecutionRun(
      active.state,
      [codexUsage({ checkedAt: new Date(NOW_MS + 60000).toISOString() })],
      { nowMs: NOW_MS + 90000 },
    );
    if (finished.status !== "finished") throw new Error("fixture finish failed");
    const mixed = {
      ...active.state,
      recentRuns: finished.state.recentRuns,
    };
    const memory = memoryStorage();
    saveExecutionRunsState(mixed, memory);

    const result = clearExecutionRunsState({}, memory);

    expect(result.activeRunPreserved).toBe(true);
    expect(result.cleared).toBe(false);
    expect(result.state.activeRun?.runId).toBe(active.outcome.run.runId);
    expect(result.state.recentRuns).toEqual([]);
  });

  it("removes an active run only behind explicit confirmation", () => {
    const active = stateWithActiveRun();
    const memory = memoryStorage();
    saveExecutionRunsState(active.state, memory);

    const result = clearExecutionRunsState(
      { deliberateActiveConfirmation: true },
      memory,
    );

    expect(result.activeRunPreserved).toBe(false);
    expect(result.cleared).toBe(true);
    expect(result.state).toEqual(EMPTY_EXECUTION_RUNS_STATE);
  });

  it("an active run is recovered after a restart (case 12)", () => {
    const { state, outcome } = stateWithActiveRun();
    const memory = memoryStorage();
    saveExecutionRunsState(state, memory);
    const recovered = loadExecutionRunsState(memory);
    expect(recovered.activeRun).not.toBeNull();
    expect(recovered.activeRun?.runId).toBe(outcome.run.runId);
    expect(recovered.activeRun?.startedAt).toBe(outcome.run.startedAt);
    expect(recovered.activeRun?.beforeSnapshot.accountIdentity).toBe("key:3456");
    expect(recovered.activeRun?.harness).toBe("Codex");
  });

  it("recent runs survive a restart, bounded to the cap", () => {
    let state: ExecutionRunsState = EMPTY_EXECUTION_RUNS_STATE;
    for (let i = 0; i < EXECUTION_RUNS_MAX_RECENT + 5; i += 1) {
      const started = beginExecutionRun(state, {
        usage: codexUsage({
          checkedAt: new Date(NOW_MS + i * 60000).toISOString(),
        }),
        harness: "Codex",
        nowMs: NOW_MS + i * 60000,
      });
      if (started.status !== "started") throw new Error("fixture start failed");
      const finished = completeExecutionRun(
        started.state,
        [codexUsage({ checkedAt: new Date(NOW_MS + i * 60000 + 60000).toISOString() })],
        { nowMs: NOW_MS + i * 60000 + 90000 },
      );
      if (finished.status !== "finished") throw new Error("fixture finish failed");
      state = finished.state;
    }
    expect(state.recentRuns).toHaveLength(EXECUTION_RUNS_MAX_RECENT);
    const memory = memoryStorage();
    saveExecutionRunsState(state, memory);
    const recovered = loadExecutionRunsState(memory);
    expect(recovered.recentRuns).toHaveLength(EXECUTION_RUNS_MAX_RECENT);
    expect(recovered.recentRuns[0]?.runId).toBe(state.recentRuns[0]?.runId);
  });

  it("corrupted or hostile persisted state is dropped, not shown", () => {
    const memory = memoryStorage();
    memory.setItem("limitscope.execution-runs.v1", "{not json");
    expect(loadExecutionRunsState(memory).activeRun).toBeNull();
    memory.setItem(
      "limitscope.execution-runs.v1",
      JSON.stringify({
        activeRun: {
          runId: "run-x",
          startedAt: "2026-09-30T12:00:00Z",
          beforeSnapshot: { providerId: "openai-codex", capturedAt: "garbage", status: "ok", windows: [] },
        },
        recentRuns: [
          {
            runId: "run-secret",
            startedAt: "2026-09-30T12:00:00Z",
            endedAt: "2026-09-30T12:01:00Z",
            confidence: "EXACT",
            beforeSnapshot: { providerId: "p", capturedAt: BASELINE_AT, status: "ok", windows: [] },
            afterSnapshot: { providerId: "p", capturedAt: BASELINE_AT, status: "ok", windows: [] },
            windows: [],
          },
        ],
      }),
    );
    const state = loadExecutionRunsState(memory);
    expect(state.activeRun).toBeNull();
    // EXACT is never produced by this app; a forged entry is dropped.
    expect(state.recentRuns).toHaveLength(0);
  });

  it("persisted state carries no secret-shaped material", () => {
    const { state } = stateWithActiveRun();
    const memory = memoryStorage();
    saveExecutionRunsState(state, memory);
    const raw = memory.getItem("limitscope.execution-runs.v1") ?? "";
    expect(raw).not.toMatch(/sk-/i);
    expect(raw).not.toMatch(/eyJ/);
    expect(raw).not.toMatch(/@/);
  });
});

describe("execution runs: presentation helpers", () => {
  it("elapsed and duration formatting", () => {
    expect(formatRunElapsed("2026-09-30T11:48:00Z", NOW_MS)).toBe("12m");
    expect(formatRunElapsed("2026-09-30T10:30:00Z", NOW_MS)).toBe("1h 30m");
    expect(formatRunElapsed("2026-09-30T11:59:30Z", NOW_MS)).toBe("<1m");
    expect(formatRunDuration("2026-09-30T11:32:00Z", "2026-09-30T11:51:00Z")).toBe("19 min");
    expect(formatRunDuration("2026-09-30T11:32:00Z", "2026-09-30T12:32:00Z")).toBe("1h");
  });

  it("delta wording never reads as task cost", () => {
    expect(formatDeltaPoints(5)).toBe("+5 pp observed");
    expect(formatDeltaPoints(-3)).toBe("-3 pp observed");
    expect(formatDeltaPoints(0)).toBe("0 pp observed");
    expect(formatDeltaPoints(0.4)).toBe("+<1 pp observed");
    expect(formatDeltaPoints(-0.4)).toBe("-<1 pp observed");
  });
});

function normalizeOf(usage: ProviderUsage) {
  // The store normalizes through the same core path; tests use it directly.
  const snapshot = resolveStartSnapshot(usage, Date.parse(usage.checkedAt));
  if (!snapshot.ok) throw new Error("fixture usage is not a usable snapshot");
  return snapshot.snapshot;
}

/** Minimal Storage stand-in so tests never touch real localStorage. */
function memoryStorage(): Pick<Storage, "getItem" | "setItem" | "removeItem"> {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? (map.get(key) as string) : null),
    setItem: (key, value) => void map.set(key, String(value)),
    removeItem: (key) => void map.delete(key),
  };
}
