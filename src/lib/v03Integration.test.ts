import { describe, expect, it } from "vitest";
import type { QuotaPrediction } from "./prediction/types";
import type { QuotaObservation } from "./quotaHistory";
import type { ProviderUsage } from "../types";
import { formatTime } from "./format";
import {
  historyForCurrentAccounts,
  predictionBasisLabel,
  providerSourceNote,
  providerStatusPresentation,
  refreshCycleSucceeded,
  visiblePrediction,
} from "./v03Integration";

const NOW = "2026-09-28T12:00:00.000Z";

function usage(overrides: Partial<ProviderUsage> = {}): ProviderUsage {
  return {
    id: "codex",
    name: "Codex",
    status: "ok",
    health: "live",
    checkedAt: NOW,
    limits: [{ label: "5-hour", usedPercent: 40, resetAt: "2026-09-28T14:00:00.000Z" }],
    ...overrides,
  };
}

function prediction(overrides: Partial<QuotaPrediction> = {}): QuotaPrediction {
  return {
    providerId: "codex",
    windowLabel: "5-hour",
    burnRatePerHour: 4,
    projectedPercentAtReset: 48,
    confidence: "medium",
    basis: {
      segmentId: "codex|5-hour|1",
      segmentCount: 1,
      segmentSampleCount: 4,
      fitSampleCount: 4,
      fitSpanMinutes: 65,
      fitMeanGapMinutes: 20,
      usedWholeSegment: false,
      latestUsedPercent: 40,
      isStale: false,
      resetExpired: false,
    },
    ...overrides,
  };
}

// The "history observation boundary" coverage (which refreshes are sampled,
// the five allowed fields, account identity carry) moved with history
// ownership into Rust: `observations_from_usages` in src-tauri/src/history.rs
// pins the same rules test-for-test.

/**
 * MIC-297 follow-up: predictions must never blend two accounts of one
 * provider. These tests pin the full scenario: record under account A,
 * swap the stored credential to B, and verify B starts independent while
 * A's history stays intact for a stable return.
 */
describe("history for current accounts", () => {
  const A = "key:aaaa";
  const B = "key:bbbb";

  const observation = (
    overrides: Partial<QuotaObservation> & { providerId: string; windowLabel: string },
  ): QuotaObservation => ({
    usedPercent: 40,
    observedAt: NOW,
    ...overrides,
  });

  const opencodeHistory = (): QuotaObservation[] => [
    observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 80, account: A }),
    observation({ providerId: "opencode-go", windowLabel: "Weekly", usedPercent: 20, account: A }),
  ];

  it("admits only the observations of the currently proven account", () => {
    const current = usage({
      id: "opencode-go",
      account: { label: "key ••bbbb", identity: B },
    });
    const filtered = historyForCurrentAccounts(
      [...opencodeHistory(), observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 5, account: B })],
      [current],
    );
    expect(filtered).toEqual([
      observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 5, account: B }),
    ]);
  });

  it("starts B independent, then reuses A's history when A returns", () => {
    const bUsage = usage({ id: "opencode-go", account: { label: "key ••bbbb", identity: B } });
    // After the swap, B consumes nothing of A's.
    expect(historyForCurrentAccounts(opencodeHistory(), [bUsage])).toEqual([]);

    // B accumulates its own observations; the store keeps A's partition.
    const stored = [
      ...opencodeHistory(),
      observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 5, account: B }),
    ];

    // Back to A: the stable identity re-admits A's history, never B's.
    const aUsage = usage({ id: "opencode-go", account: { label: "key ••aaaa", identity: A } });
    expect(historyForCurrentAccounts(stored, [aUsage])).toEqual(opencodeHistory());
  });

  it("never feeds legacy unattributed observations to an attributed account", () => {
    const legacy = [
      observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 55 }),
      observation({ providerId: "opencode-go", windowLabel: "Weekly", usedPercent: 15 }),
    ];
    const current = usage({
      id: "opencode-go",
      account: { label: "key ••aaaa", identity: A },
    });
    expect(historyForCurrentAccounts(legacy, [current])).toEqual([]);
  });

  it("keeps unattributed history flowing for providers without attribution", () => {
    const stored = [
      ...opencodeHistory(),
      observation({ providerId: "codex", windowLabel: "5h", usedPercent: 30 }),
      // A provider that is attributed right now must not see these, but
      // codex (no account concept) behaves exactly as before the change.
      observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 90 }),
    ];
    const current = [
      usage({ id: "codex" }),
      usage({ id: "zai", status: "error", health: "error", limits: [] }),
    ];
    expect(historyForCurrentAccounts(stored, current)).toEqual([
      observation({ providerId: "codex", windowLabel: "5h", usedPercent: 30 }),
      observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 90 }),
    ]);
  });

  it("treats a blank identity token as unattributed", () => {
    const stored = [
      observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 10 }),
      observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 20, account: A }),
    ];
    // A blank token proves nothing, so the provider is treated as
    // unattributed: unattributed history flows, partitioned history does not.
    const current = usage({ id: "opencode-go", account: { label: "key ••????", identity: "  " } });
    expect(historyForCurrentAccounts(stored, [current])).toEqual([
      observation({ providerId: "opencode-go", windowLabel: "5-hour", usedPercent: 10 }),
    ]);
  });
});

describe("prediction presentation gate", () => {
  it("shows medium and high predictions", () => {
    expect(visiblePrediction(usage(), 40, prediction())).toBeDefined();
    expect(visiblePrediction(usage(), 40, prediction({ confidence: "high" }))).toBeDefined();
  });

  it("hides insufficient and low confidence without a fallback badge", () => {
    expect(visiblePrediction(usage(), 40, prediction({ confidence: "insufficient" }))).toBeUndefined();
    expect(visiblePrediction(usage(), 40, prediction({ confidence: "low" }))).toBeUndefined();
  });

  it("hides stale-source and stale-basis predictions", () => {
    expect(visiblePrediction(usage({ status: "stale", health: "stale" }), 40, prediction())).toBeUndefined();
    expect(visiblePrediction(usage(), 40, prediction({
      basis: { ...prediction().basis, isStale: true },
    }))).toBeUndefined();
  });

  it("keeps an undatable freshness verdict out of predictions", () => {
    // A fresh verdict with no usable stamp is indeterminate, so it supports
    // no projection; a verdict that carries a real stamp is unaffected.
    expect(visiblePrediction(usage({
      dataFreshness: "fresh",
      sourceUpdatedAt: undefined,
    }), 40, prediction())).toBeUndefined();
    expect(visiblePrediction(usage({
      dataFreshness: "fresh",
      sourceUpdatedAt: "not-a-date",
    }), 40, prediction())).toBeUndefined();
    expect(visiblePrediction(usage({
      dataFreshness: "fresh",
      sourceUpdatedAt: "2026-09-28T11:55:00.000Z",
    }), 40, prediction())).toBeDefined();
  });

  it("allows fresh retained history after a failed refresh", () => {
    expect(visiblePrediction(usage({ status: "error", health: "error", limits: usage().limits }), 40, prediction())).toBeDefined();
  });

  it("treats cooldown and unavailable entries like the failure states they are", () => {
    // The gate never *promotes* a failure state; the retained-window
    // behavior of an error (kept deliberately unchanged) extends to the
    // other failure states. Health itself never hides a prediction that an
    // error would not already hide.
    expect(visiblePrediction(usage({ status: "error", health: "cooldown", limits: usage().limits }), 40, prediction())).toBeDefined();
    expect(visiblePrediction(usage({ status: "error", health: "unavailable", limits: usage().limits }), 40, prediction())).toBeDefined();
  });

  it("replaces a reached-limit projection with the observed limit state", () => {
    expect(visiblePrediction(usage(), 100, prediction())).toBeUndefined();
  });

  it("formats a compact current-cycle basis", () => {
    expect(predictionBasisLabel(prediction())).toBe(
      "Based on 4 samples over 1h 5m; current reset cycle only",
    );
  });
});

describe("provider status presentation", () => {
  it("keeps live, cached, stale, failed, and mock states distinct", () => {
    expect(providerStatusPresentation(usage()).label).toBe("Live");
    expect(providerStatusPresentation(usage({
      dataFreshness: "fresh",
      sourceUpdatedAt: "2026-09-28T11:55:00.000Z",
    })).label).toBe("Cached");
    expect(providerStatusPresentation(usage({ status: "stale", health: "stale" })).label).toBe("Stale");
    expect(providerStatusPresentation(usage({ status: "error", health: "error" })).label).toBe("Refresh failed");
    expect(providerStatusPresentation(usage({ simulated: true })).label).toBe("Mock");
  });

  it("names cooldown and unavailable health distinctly, in the error class", () => {
    // The runtime owns the cooldown verdict; the frontend only names it and
    // keeps the failure visuals.
    const cooldown = providerStatusPresentation(usage({
      status: "error",
      health: "cooldown",
    }));
    expect(cooldown.label).toBe("Cooldown");
    expect(cooldown.className).toBe("error");

    const unavailable = providerStatusPresentation(usage({
      status: "error",
      health: "unavailable",
      limits: [],
    }));
    expect(unavailable.label).toBe("Unavailable");
    expect(unavailable.className).toBe("error");
    // Neither state can read as live.
    expect(providerStatusPresentation(usage({ health: "cooldown" })).className).not.toBe("ok");
    expect(providerStatusPresentation(usage({ health: "unavailable" })).className).not.toBe("ok");
  });

  it("notes cooldown state with retained data age, or the waiting window", () => {
    const retained = providerSourceNote(usage({
      status: "error",
      health: "cooldown",
      checkedAt: "2026-09-28T11:30:00.000Z",
      error: "Refresh failed: rate limited (unexpected_response)",
    }), new Date(NOW));
    expect(retained).toContain("Cooling down · showing data from");
    expect(retained).toContain("(30m ago)");

    const bare = providerSourceNote(usage({
      status: "error",
      health: "cooldown",
      limits: [],
    }), new Date(NOW));
    expect(bare).toBe("Waiting for the server's retry window");
  });

  it("notes unavailable providers without inventing data age", () => {
    const note = providerSourceNote(usage({
      status: "error",
      health: "unavailable",
      limits: [],
      errorCategory: "credential_missing",
    }), new Date(NOW));
    expect(note).toBe("Provider data unavailable");
  });

  it("treats unknown cache timestamps as stale, never fresh or live", () => {
    expect(providerStatusPresentation(usage({
      dataFreshness: "fresh",
      sourceUpdatedAt: undefined,
    })).label).toBe("Stale");
    expect(providerStatusPresentation(usage({
      dataFreshness: "fresh",
      sourceUpdatedAt: "not-a-date",
    })).label).toBe("Stale");
    expect(providerSourceNote(usage({
      dataFreshness: "fresh",
      sourceUpdatedAt: undefined,
    }), new Date(NOW))).toBe("Source update time unknown");
  });

  it("shows the retained observation time after a failed refresh", () => {
    const note = providerSourceNote(usage({
      status: "error",
      health: "error",
      checkedAt: "2026-09-28T11:30:00.000Z",
    }), new Date(NOW));
    expect(note).toContain("Showing last good data from");
    expect(note).toContain("(30m ago)");
  });

  it("ages a failed refresh from the source snapshot, not the cache read", () => {
    // checkedAt is when the cache was last read successfully; the retained
    // values are only as new as sourceUpdatedAt says they are.
    const note = providerSourceNote(usage({
      status: "error",
      health: "error",
      checkedAt: "2026-09-28T11:55:00.000Z",
      sourceUpdatedAt: "2026-09-28T10:00:00.000Z",
      dataFreshness: "fresh",
    }), new Date(NOW));
    expect(note).toContain("Showing last good data from");
    expect(note).toContain(formatTime("2026-09-28T10:00:00.000Z"));
    expect(note).toContain("(2h ago)");
    expect(note).not.toContain(formatTime("2026-09-28T11:55:00.000Z"));
  });

  it("marks stale retained data as stale after a failed refresh", () => {
    const note = providerSourceNote(usage({
      status: "error",
      health: "error",
      checkedAt: "2026-09-28T11:55:00.000Z",
      sourceUpdatedAt: "2026-09-28T08:00:00.000Z",
      dataFreshness: "stale",
    }), new Date(NOW));
    expect(note).toContain("Refresh failed · stale data from");
    expect(note).toContain(formatTime("2026-09-28T08:00:00.000Z"));
    expect(note).toContain("(4h ago)");
  });

  it("falls back to the checked time when a failed refresh has no usable snapshot time", () => {
    const note = providerSourceNote(usage({
      status: "error",
      health: "error",
      checkedAt: "2026-09-28T11:30:00.000Z",
      sourceUpdatedAt: "not-a-date",
    }), new Date(NOW));
    expect(note).toContain("Showing last good data from");
    expect(note).toContain(formatTime("2026-09-28T11:30:00.000Z"));
    expect(note).toContain("(30m ago)");
  });

  it("appends masked account attribution to live and retained notes", () => {
    const note = providerSourceNote(usage({
      account: { label: "7a2d5abe… · opencode", identity: "xai:7a2d5abe…" },
    }), new Date(NOW));
    expect(note).toContain("account 7a2d5abe…");

    const retained = providerSourceNote(usage({
      status: "error",
      health: "error",
      checkedAt: "2026-09-28T11:30:00.000Z",
      account: { label: "7a2d5abe… · opencode", identity: "xai:7a2d5abe…" },
    }), new Date(NOW));
    expect(retained).toContain("Showing last good data from");
    expect(retained).toContain("account 7a2d5abe…");
  });

  it("never appends account attribution to mocked providers", () => {
    const note = providerSourceNote(usage({
      simulated: true,
      account: { label: "7a2d5abe… · opencode", identity: "xai:7a2d5abe…" },
    }), new Date(NOW));
    expect(note).toBe("Deterministic sample data");
  });
});

describe("global refresh outcome", () => {
  it("treats all-error and empty cycles as overdue", () => {
    expect(refreshCycleSucceeded([])).toBe(false);
    expect(refreshCycleSucceeded([
      usage({ id: "a", status: "error", health: "error" }),
      usage({ id: "b", status: "error", health: "error" }),
    ])).toBe(false);
  });

  it("does not let mock successes hide rejection of every real provider", () => {
    expect(refreshCycleSucceeded([
      usage({ id: "codex", status: "error", health: "error" }),
      usage({ id: "claude", simulated: true }),
      usage({ id: "grok", simulated: true }),
    ])).toBe(false);
  });

  it("accepts live, cached/stale, and mixed successful cycles", () => {
    expect(refreshCycleSucceeded([usage()])).toBe(true);
    expect(refreshCycleSucceeded([usage({ status: "stale", health: "stale" })])).toBe(true);
    expect(refreshCycleSucceeded([
      usage({ id: "a", status: "error", health: "error" }),
      usage({ id: "b" }),
    ])).toBe(true);
  });
});
