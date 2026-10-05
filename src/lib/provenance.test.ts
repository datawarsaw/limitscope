import { describe, expect, it } from "vitest";
import {
  buildProvenance,
  formatProvenanceFooter,
  normalizeSnapshot,
  sanitizeAccountIdentity,
  sanitizePlanType,
  snapshotStaleness,
  startProvenanceRun,
  finishProvenanceRun,
  type ExecutionProvenanceRun,
  type ExecutionQuotaSnapshot,
} from "./provenance";

const NOW_MS = Date.parse("2026-09-30T10:00:00Z");
const STARTED = "2026-09-30T09:00:00Z";
const ENDED = "2026-09-30T09:45:00Z";
const ENDED_MS = Date.parse(ENDED);
const RESET_FUTURE = "2026-09-30T14:00:00Z";
const RESET_NEXT_CYCLE = "2026-10-01T14:00:00Z";

function snapshot(overrides: Partial<ExecutionQuotaSnapshot> = {}): ExecutionQuotaSnapshot {
  return {
    capturedAt: STARTED,
    providerId: "openai-codex",
    status: "ok",
    windows: [{ label: "5-hour", usedPercent: 58, resetAt: RESET_FUTURE }],
    ...overrides,
  };
}

function run(
  before: ExecutionQuotaSnapshot,
  after: ExecutionQuotaSnapshot,
  extra: Record<string, unknown> = {},
): ExecutionProvenanceRun {
  return buildProvenance({
    before,
    after,
    harness: "Codex",
    runId: "run-test-1234",
    model: "GPT-6 Sol",
    startedAt: STARTED,
    endedAt: ENDED,
    nowMs: ENDED_MS,
    ...extra,
  });
}

describe("provenance: fresh bracket ceiling", () => {
  it("fresh bracket -> BOUNDED", () => {
    const before = snapshot();
    const after = snapshot({
      capturedAt: ENDED,
      windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }],
    });
    const prov = run(before, after);
    expect(prov.confidence).toBe("BOUNDED");
    expect(prov.attributionConfidence).toBe("BOUNDED");
    expect(prov.comparable).toBe(true);
  });

  it("no exact automatic attribution", () => {
    const before = snapshot();
    const after = snapshot({
      capturedAt: ENDED,
      windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }],
    });
    const prov = run(before, after);
    expect(prov.confidence).not.toBe("EXACT");
    expect(prov.attributionConfidence).not.toBe("EXACT");
  });
});
describe("provenance: staleness", () => {
  it("stale before -> INFERRED", () => {
    const staleBefore = snapshot({ capturedAt: "2026-09-29T08:00:00Z" });
    expect(snapshotStaleness(staleBefore, NOW_MS).stale).toBe(true);
    const prov = run(
      staleBefore,
      snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 60, resetAt: RESET_FUTURE }] }),
      { nowMs: NOW_MS },
    );
    expect(prov.confidence).toBe("INFERRED");
    expect(prov.attributionConfidence).toBe("INFERRED");
  });

  it("stale after -> INFERRED", () => {
    const staleAfter = snapshot({
      capturedAt: "2026-09-29T08:00:00Z",
      windows: [{ label: "5-hour", usedPercent: 60, resetAt: RESET_FUTURE }],
    });
    expect(snapshotStaleness(staleAfter, NOW_MS).stale).toBe(true);
    const prov = run(snapshot(), staleAfter, { nowMs: NOW_MS });
    expect(prov.confidence).toBe("INFERRED");
  });

  it("missing runId drops fresh snapshots to INFERRED", () => {
    const prov = buildProvenance({
      before: snapshot(),
      after: snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 60, resetAt: RESET_FUTURE }] }),
      harness: "Codex",
      runId: undefined,
      nowMs: ENDED_MS,
    });
    expect(prov.confidence).toBe("INFERRED");
  });
});

describe("provenance: failed after snapshot", () => {
  it("failed after with error status -> UNAVAILABLE", () => {
    const prov = run(snapshot(), snapshot({ capturedAt: ENDED, status: "error", windows: [] }));
    expect(prov.confidence).toBe("UNAVAILABLE");
    expect(prov.comparable).toBe(false);
    expect(formatProvenanceFooter(prov, { nowMs: NOW_MS })).toContain("Quota unavailable");
  });

  it("failed after with stale status -> INFERRED", () => {
    const prov = run(
      snapshot(),
      snapshot({
        capturedAt: ENDED,
        status: "stale",
        windows: [{ label: "5-hour", usedPercent: 62, resetAt: RESET_FUTURE }],
      }),
    );
    expect(prov.confidence).toBe("INFERRED");
    expect(prov.comparable).toBe(true);
  });
});

describe("provenance: provider mismatch", () => {
  it("provider mismatch -> UNAVAILABLE", () => {
    const prov = run(
      snapshot({ providerId: "openai-codex" }),
      snapshot({
        capturedAt: ENDED,
        providerId: "zai",
        windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }],
      }),
    );
    expect(prov.confidence).toBe("UNAVAILABLE");
    expect(prov.comparable).toBe(false);
    expect(prov.incomparabilityReason).toContain("Provider changed during execution");
  });
});

describe("provenance: account mismatch", () => {
  it("account mismatch between differing identities -> UNAVAILABLE", () => {
    const prov = run(
      snapshot({ accountIdentity: "key:1111" }),
      snapshot({
        capturedAt: ENDED,
        accountIdentity: "key:2222",
        windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }],
      }),
    );
    expect(prov.comparable).toBe(false);
    expect(prov.confidence).toBe("UNAVAILABLE");
    expect(formatProvenanceFooter(prov, { nowMs: NOW_MS })).toContain("Account changed during execution.");
  });

  it("account mismatch when one snapshot loses identity -> UNAVAILABLE", () => {
    const prov = run(
      snapshot({ accountIdentity: "key:1111" }),
      snapshot({
        capturedAt: ENDED,
        accountIdentity: undefined,
        windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }],
      }),
    );
    expect(prov.comparable).toBe(false);
    expect(prov.confidence).toBe("UNAVAILABLE");
    expect(prov.incomparabilityReason).toContain("Account changed during execution");
  });

  it("matching account identities retain BOUNDED confidence", () => {
    const prov = run(
      snapshot({ accountIdentity: "key:1111" }),
      snapshot({
        capturedAt: ENDED,
        accountIdentity: "key:1111",
        windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }],
      }),
    );
    expect(prov.comparable).toBe(true);
    expect(prov.confidence).toBe("BOUNDED");
  });

  it("unattributed snapshots for unattributed providers retain BOUNDED confidence", () => {
    const prov = run(
      snapshot({ accountIdentity: undefined }),
      snapshot({
        capturedAt: ENDED,
        accountIdentity: undefined,
        windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }],
      }),
    );
    expect(prov.comparable).toBe(true);
    expect(prov.confidence).toBe("BOUNDED");
  });
});

describe("provenance: empty windows", () => {
  it("empty before windows -> UNAVAILABLE", () => {
    const prov = run(snapshot({ windows: [] }), snapshot({ capturedAt: ENDED }));
    expect(prov.confidence).toBe("UNAVAILABLE");
    expect(prov.comparable).toBe(false);
  });

  it("empty after windows -> UNAVAILABLE", () => {
    const prov = run(snapshot(), snapshot({ capturedAt: ENDED, windows: [] }));
    expect(prov.confidence).toBe("UNAVAILABLE");
    expect(prov.comparable).toBe(false);
  });
});

describe("provenance: reset crossing", () => {
  it("reset crossing marks delta incomparable and never reports negative task cost", () => {
    const before = snapshot({ windows: [{ label: "5-hour", usedPercent: 90, resetAt: "2026-09-30T09:30:00Z" }] });
    const after = snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 5, resetAt: RESET_NEXT_CYCLE }] });
    const prov = run(before, after);
    expect(prov.resetCrossed).toBe(true);
    expect(prov.comparable).toBe(false);
    expect(prov.windows[0].comparable).toBe(false);
    expect(prov.windows[0].deltaPoints).toBe(0);

    const footer = formatProvenanceFooter(prov, { nowMs: NOW_MS });
    expect(footer).toContain("Quota reset occurred during execution.");
    expect(footer).not.toContain("-85");
  });

  it("reset boundary passing during execution is flagged", () => {
    // Reset boundary at 09:30 passed before run ended at 09:45
    const before = snapshot({ windows: [{ label: "5-hour", usedPercent: 80, resetAt: "2026-09-30T09:30:00Z" }] });
    const after = snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 10, resetAt: "2026-09-30T09:30:00Z" }] });
    const prov = run(before, after);
    expect(prov.resetCrossed).toBe(true);
    expect(prov.comparable).toBe(false);
  });
});

describe("provenance: multiple windows", () => {
  it("multiple windows are kept independent without synthetic totals", () => {
    const before = snapshot({
      windows: [
        { label: "5-hour", usedPercent: 28, resetAt: RESET_FUTURE },
        { label: "Weekly", usedPercent: 31, resetAt: RESET_NEXT_CYCLE },
      ],
    });
    const after = snapshot({
      capturedAt: ENDED,
      windows: [
        { label: "5-hour", usedPercent: 31, resetAt: RESET_FUTURE },
        { label: "Weekly", usedPercent: 32, resetAt: RESET_NEXT_CYCLE },
      ],
    });
    const prov = run(before, after);
    expect(prov.windows).toHaveLength(2);
    expect(prov.windows.map((w) => w.label)).toEqual(["5-hour", "Weekly"]);
    expect(prov.windows[0].deltaPoints).toBeCloseTo(3);
    expect(prov.windows[1].deltaPoints).toBeCloseTo(1);

    const footer = formatProvenanceFooter(prov, { nowMs: NOW_MS });
    expect(footer).toContain("5-hour");
    expect(footer).toContain("Weekly");
    expect(footer).not.toMatch(/total/i);
  });
});

describe("provenance: quota deltas", () => {
  it("used delta increase", () => {
    const before = snapshot({ windows: [{ label: "5-hour", usedPercent: 58, resetAt: RESET_FUTURE }] });
    const after = snapshot({
      capturedAt: ENDED,
      windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }],
    });
    const prov = run(before, after);
    expect(prov.comparable).toBe(true);
    expect(prov.windows[0].deltaPoints).toBeCloseTo(6);
    expect(prov.windows[0].beforeUsedPercent).toBe(58);
    expect(prov.windows[0].afterUsedPercent).toBe(64);
  });

  it("used delta decrease without reset", () => {
    const before = snapshot({ windows: [{ label: "5-hour", usedPercent: 60, resetAt: RESET_FUTURE }] });
    const after = snapshot({
      capturedAt: ENDED,
      windows: [{ label: "5-hour", usedPercent: 55, resetAt: RESET_FUTURE }],
    });
    const prov = run(before, after);
    expect(prov.resetCrossed).toBe(false);
    expect(prov.comparable).toBe(true);
    expect(prov.windows[0].deltaPoints).toBeCloseTo(-5);
  });
});

describe("provenance: optional metadata and plan type", () => {
  it("unknown model omitted", () => {
    const prov = buildProvenance({
      before: snapshot(),
      after: snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 60, resetAt: RESET_FUTURE }] }),
      harness: "Codex",
      runId: "run-1",
      nowMs: ENDED_MS,
    });
    expect(prov.model).toBeUndefined();
    expect(formatProvenanceFooter(prov, { nowMs: NOW_MS })).not.toContain("Model:");
  });

  it("unknown reasoning omitted", () => {
    const prov = buildProvenance({
      before: snapshot(),
      after: snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 60, resetAt: RESET_FUTURE }] }),
      harness: "Codex",
      runId: "run-1",
      model: "GPT-6",
      nowMs: ENDED_MS,
    });
    expect(prov.reasoningEffort).toBeUndefined();
    expect(formatProvenanceFooter(prov, { nowMs: NOW_MS })).not.toContain("Reasoning:");
  });

  it("plan_type explicit: surfaces explicit plan and never infers from model", () => {
    // Model says "GPT-6 Pro" but no planType supplied -> subscription must be undefined, NOT "pro"
    const provWithoutPlan = buildProvenance({
      before: snapshot(),
      after: snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 60, resetAt: RESET_FUTURE }] }),
      harness: "Codex",
      runId: "run-1",
      model: "GPT-6 Pro",
      nowMs: ENDED_MS,
    });
    expect(provWithoutPlan.subscription).toBeUndefined();
    expect(formatProvenanceFooter(provWithoutPlan, { nowMs: NOW_MS })).not.toContain("Subscription:");

    // Explicit planType from provider snapshot
    const provWithPlan = buildProvenance({
      before: snapshot({ planType: "team" }),
      after: snapshot({ capturedAt: ENDED, planType: "team", windows: [{ label: "5-hour", usedPercent: 60, resetAt: RESET_FUTURE }] }),
      harness: "Codex",
      runId: "run-1",
      nowMs: ENDED_MS,
    });
    expect(provWithPlan.subscription).toBe("team");
    expect(formatProvenanceFooter(provWithPlan, { nowMs: NOW_MS })).toContain("Subscription: team");
  });
});

describe("provenance: footer determinism and language", () => {
  it("footer deterministic with stable ordering", () => {
    const before = snapshot({
      windows: [
        { label: "Weekly", usedPercent: 31, resetAt: RESET_NEXT_CYCLE },
        { label: "5-hour", usedPercent: 28, resetAt: RESET_FUTURE },
      ],
    });
    const after = snapshot({
      capturedAt: ENDED,
      windows: [
        { label: "Weekly", usedPercent: 32, resetAt: RESET_NEXT_CYCLE },
        { label: "5-hour", usedPercent: 31, resetAt: RESET_FUTURE },
      ],
    });
    const footer1 = formatProvenanceFooter(run(before, after), { nowMs: NOW_MS });
    const footer2 = formatProvenanceFooter(run(before, after), { nowMs: NOW_MS });
    expect(footer1).toBe(footer2);
    expect(footer1.indexOf("5-hour")).toBeLessThan(footer1.indexOf("Weekly"));
  });

  it("never presents the delta as task cost and includes explicit disclaimer", () => {
    const prov = run(snapshot(), snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }] }));
    const footerText = formatProvenanceFooter(prov, { nowMs: NOW_MS });
    expect(footerText).not.toMatch(/task cost:/i);
    expect(footerText).not.toMatch(/cost of (this|the) task/i);
    expect(footerText).not.toMatch(/task consumed/i);
    expect(footerText).toContain("(not exact task cost)");
    expect(footerText).toContain("Observed quota delta during execution");
  });

  it("renders remaining perspective correctly", () => {
    const prov = run(snapshot(), snapshot({ capturedAt: ENDED, windows: [{ label: "5-hour", usedPercent: 64, resetAt: RESET_FUTURE }] }));
    const footerRem = formatProvenanceFooter(prov, { perspective: "remaining", nowMs: NOW_MS });
    expect(footerRem).toContain("42% remaining -> 36% remaining");
    const footerUsed = formatProvenanceFooter(prov, { perspective: "used", nowMs: NOW_MS });
    expect(footerUsed).toContain("58% used -> 64% used");
  });
});

describe("provenance: manual bracket workflow", () => {
  it("startProvenanceRun and finishProvenanceRun lifecycle", () => {
    const before = snapshot({ accountIdentity: "key:5678", planType: "plus" });
    const activeRun = startProvenanceRun({
      before,
      model: "GPT-6 Luna",
      reasoningEffort: "medium",
      subscription: "plus",
    });

    expect(activeRun.runId).toMatch(/^run-/);
    expect(activeRun.beforeSnapshot).toBe(before);
    expect(activeRun.model).toBe("GPT-6 Luna");
    expect(activeRun.subscription).toBe("plus");
    expect(activeRun.account).toBe("key:5678");
    expect(activeRun.confidence).toBe("BOUNDED");
    expect(activeRun.comparable).toBe(false);

    const after = snapshot({
      capturedAt: ENDED,
      accountIdentity: "key:5678",
      planType: "plus",
      windows: [{ label: "5-hour", usedPercent: 62, resetAt: RESET_FUTURE }],
    });

    const finished = finishProvenanceRun(activeRun, after, { endedAt: ENDED, nowMs: ENDED_MS });
    expect(finished.runId).toBe(activeRun.runId);
    expect(finished.comparable).toBe(true);
    expect(finished.windows[0].deltaPoints).toBeCloseTo(4);
    expect(finished.confidence).toBe("BOUNDED");
  });
});

describe("provenance: security boundaries", () => {
  it("sanitizes account identity and drops secret-shaped tokens", () => {
    expect(sanitizeAccountIdentity("key:3456")).toBe("key:3456");
    expect(sanitizeAccountIdentity("sk-proj-1234567890abcdef")).toBeUndefined();
    expect(sanitizeAccountIdentity("user@example.com")).toBeUndefined();
    expect(sanitizeAccountIdentity("eyJhbGciOiJIUzI1NiJ9.payload.sig")).toBeUndefined();
    expect(sanitizeAccountIdentity("Bearer token_value")).toBeUndefined();
  });

  it("sanitizes plan type and drops secret-shaped tokens", () => {
    expect(sanitizePlanType("team")).toBe("team");
    expect(sanitizePlanType("Pro")).toBe("pro");
    expect(sanitizePlanType("sk-secret")).toBeUndefined();
    expect(sanitizePlanType("user@example.com")).toBeUndefined();
  });

  it("normalizes ProviderUsage-shaped snapshots with planType and masked identity", () => {
    const normalized = normalizeSnapshot({
      id: "openai-codex",
      checkedAt: STARTED,
      status: "ok",
      limits: [{ label: "5-hour", usedPercent: 58, resetAt: RESET_FUTURE }],
      account: { identity: "key:3456" },
      planType: "team",
    });
    expect(normalized?.providerId).toBe("openai-codex");
    expect(normalized?.accountIdentity).toBe("key:3456");
    expect(normalized?.planType).toBe("team");
  });
});
