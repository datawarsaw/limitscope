// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import {
  beginExecutionRun,
  completeExecutionRun,
  EMPTY_EXECUTION_RUNS_STATE,
  EXECUTION_RUNS_STORAGE_KEY,
  saveExecutionRunsState,
} from "./lib/executionRuns";
import {
  createExecutionReceipt,
  formatReceiptJson,
  formatReceiptMarkdown,
} from "./lib/executionReceipt";
import type { UsageAnalytics } from "./lib/usageAnalytics";
import type { QuotaPrediction } from "./lib/prediction/types";
import type { ProviderUsage } from "./types";

/**
 * v0.7 PRODUCT integration rehearsal pins.
 *
 * These scenarios exercise the integrated product surface — Usage UI × quota
 * perspective × Local data controls × execution runs — through the real
 * App with the real (localStorage-backed) settings hook. Runtime transport
 * (provider snapshots, history store, analytics store, updater) is mocked at
 * the hook boundary, which is the same seam the production runtime uses.
 *
 * Scenario numbering matches the rehearsal task's cross-feature list.
 */

const mocks = vi.hoisted(() => ({
  usages: [] as ProviderUsage[],
  historyRevision: 1,
  historyCleared: false,
  refresh: vi.fn(),
}));

vi.mock("./hooks/useProviderUsage", () => ({
  useProviderUsage: () => ({
    usages: mocks.usages,
    loading: false,
    lastUpdatedAt: new Date("2026-09-28T21:58:00"),
    refresh: mocks.refresh,
    refreshOverdue: false,
    stale: false,
    staleMinutes: 0,
    historyRevision: mocks.historyRevision,
  }),
}));

vi.mock("./hooks/useQuotaPredictions", () => ({
  useQuotaPredictions: () => ({
    predictionFor:
      (_providerId: string, _windowLabel: string) =>
        undefined as QuotaPrediction | undefined,
    historyUnavailable: false,
    // Models the runtime contract: a clear wipes the store and bumps the
    // history revision, which re-queries every consumer (Usage included).
    clearLocalHistory: vi.fn(async () => {
      mocks.historyCleared = true;
      mocks.historyRevision += 1;
      return { ok: true, removed: true };
    }),
  }),
}));

vi.mock("./hooks/useUpdater", () => ({
  useUpdater: () => ({
    phase: "idle",
    update: null,
    error: null,
    currentVersion: "0.0.0-test",
    checkForUpdates: vi.fn(),
    installUpdate: vi.fn(),
    dismissUpdate: vi.fn(),
  }),
}));

// The analytics loader is mocked at the module boundary; the cleared flag
// stands in for the Rust store being emptied underneath the view.
vi.mock("./lib/usageAnalytics", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./lib/usageAnalytics")>();
  return {
    ...actual,
    loadUsageAnalytics: vi.fn(async () => analyticsPayload()),
  };
});

import { loadUsageAnalytics } from "./lib/usageAnalytics";
const loader = loadUsageAnalytics as ReturnType<typeof vi.fn>;

const NOW = Date.parse("2026-09-28T22:00:00Z");
const HOUR = 3600000;

function analyticsPayload(range: "24h" | "7d" = "24h"): UsageAnalytics {
  return {
    range,
    rangeStart: new Date(NOW - 24 * HOUR).toISOString(),
    rangeEnd: new Date(NOW).toISOString(),
    schemaVersion: 1,
    generatedAt: new Date(NOW).toISOString(),
    timezoneOffsetMinutes: 0,
    summary: {
      peakObservedUsage: {
        providerId: "openai-codex",
        account: "key:3456",
        windowLabel: "Weekly credits",
        observedAt: new Date(NOW - 2 * HOUR).toISOString(),
        usedPercent: 95,
        exactness: "exact",
      },
      mostConstrainedWindow: {
        providerId: "openai-codex",
        account: "key:3456",
        windowLabel: "Weekly credits",
        observedAt: new Date(NOW - 2 * HOUR).toISOString(),
        usedPercent: 95,
        exactness: "exact",
      },
      observedResetCycles: {
        count: 1,
        exactness: "exact",
        detection: "historyResetBoundaryTransitions",
      },
      observedDays: { count: 2, timezoneOffsetMinutes: 0 },
      timeNearLimit: {
        estimated: true,
        method: "piecewiseLinearBetweenAdjacentSameCycleObservations",
        intervalPolicy: "onlyIntervalsAtOrBelowPerSeriesGapThreshold",
        comparableSpanMs: 8 * HOUR,
        comparableSpanRatio: 0.33,
        estimates: [
          { thresholdPercent: 80, estimatedDurationMs: 84 * 60 * 1000, estimatedShare: 0.3 },
          { thresholdPercent: 95, estimatedDurationMs: 18 * 60 * 1000, estimatedShare: 0.06 },
        ],
      },
    },
    heatmap: [
      { date: "2026-09-28", observed: true, peakUsedPercent: 84, peakExactness: "exact", band: 4 },
      { date: "2026-09-27", observed: false },
    ],
    trends: [
      {
        providerId: "openai-codex",
        account: "key:3456",
        windowLabel: "Weekly credits",
        coverage: {
          gapThresholdMs: HOUR,
          comparableSpanMs: 3 * HOUR,
          comparableSpanRatio: 0.125,
          gaps: [],
        },
        points: [
          {
            observedAt: new Date(NOW - 3 * HOUR).toISOString(),
            usedPercent: 90,
            cycleId: "c1",
            cycleStart: true,
            resetBoundary: false,
            resolution: "detailed",
          },
          {
            observedAt: new Date(NOW - 2 * HOUR).toISOString(),
            usedPercent: 95,
            cycleId: "c1",
            cycleStart: false,
            resetBoundary: false,
            resolution: "detailed",
          },
        ],
      },
    ],
    gapSemantics: "notObservedNeverZeroFilled",
    availabilityInference: "none",
  };
}

const minutesAgo = (minutes: number) =>
  new Date(Date.now() - minutes * 60_000).toISOString();
const hoursAhead = (hours: number) =>
  new Date(Date.now() + hours * 3600000).toISOString();

function emptyAnalytics(range: "24h" | "7d" = "24h"): UsageAnalytics {
  return {
    ...analyticsPayload(range),
    summary: {
      peakObservedUsage: undefined,
      mostConstrainedWindow: undefined,
      observedResetCycles: {
        count: 0,
        exactness: "exact",
        detection: "historyResetBoundaryTransitions",
      },
      observedDays: { count: 0, timezoneOffsetMinutes: 0 },
      timeNearLimit: undefined,
    },
    heatmap: [
      { date: "2026-09-28", observed: false },
      { date: "2026-09-27", observed: false },
    ],
    trends: [],
  };
}

function codexUsage(overrides: Partial<ProviderUsage> = {}): ProviderUsage {
  return {
    id: "openai-codex",
    name: "OpenAI / Codex",
    status: "ok",
    health: "live",
    checkedAt: minutesAgo(1),
    limits: [
      { label: "5-hour", usedPercent: 23, resetAt: hoursAhead(5) },
      { label: "Weekly", usedPercent: 95, resetAt: hoursAhead(100) },
    ],
    account: { label: "key:3456", identity: "key:3456" },
    planType: "team",
    resetCredits: {
      bankedCredits: 3,
      currentlyApplicable: 2,
      checkedAt: minutesAgo(1),
      source: "codex-wham-usage",
    },
    ...overrides,
  };
}

function grokUsage(): ProviderUsage {
  return {
    id: "grok",
    name: "Grok (xAI)",
    status: "ok",
    health: "live",
    checkedAt: minutesAgo(1),
    limits: [{ label: "Weekly", usedPercent: 13 }],
  };
}

function renderApp(usages: ProviderUsage[]) {
  mocks.usages = usages;
  return render(<App />);
}

async function expandDrawer(user: ReturnType<typeof userEvent.setup>) {
  const button = screen.getByRole("button", { name: "Settings" });
  if (button.getAttribute("aria-expanded") !== "true") {
    await user.click(button);
  }
}

async function choosePerspective(
  user: ReturnType<typeof userEvent.setup>,
  perspective: "Used" | "Remaining",
) {
  await expandDrawer(user);
  await user.click(screen.getByRole("radio", { name: perspective }));
}

function persistedRuns(): {
  activeRun: unknown;
  recentRuns: unknown[];
} {
  const raw = localStorage.getItem(EXECUTION_RUNS_STORAGE_KEY);
  return raw
    ? (JSON.parse(raw) as { activeRun: unknown; recentRuns: unknown[] })
    : EMPTY_EXECUTION_RUNS_STATE;
}

/** Locale-independent access to the heatmap day cells. */
function heatmapCells() {
  return within(
    screen.getByRole("list", { name: "Daily peak observed usage" }),
  ).getAllByRole("listitem");
}

beforeEach(() => {
  localStorage.clear();
  mocks.usages = [];
  mocks.historyRevision = 1;
  mocks.historyCleared = false;
  mocks.refresh = vi.fn();
  loader.mockReset();
  loader.mockImplementation(async () =>
    mocks.historyCleared ? emptyAnalytics() : analyticsPayload(),
  );
});

afterEach(() => {
  cleanup();
});

describe("quota perspective across the product (scenarios 1-5)", () => {
  it("1. Used -> Remaining flips Overview presentation without touching canonical values", async () => {
    const user = userEvent.setup();
    const view = renderApp([codexUsage()]);

    const overviewPercent = () =>
      view.container.querySelector(".primary-percent")?.textContent ?? "";
    // The primary window is the most constrained one (Weekly, 95% used).
    expect(overviewPercent()).toBe("95% used");

    await choosePerspective(user, "Remaining");
    expect(overviewPercent()).toBe("5% remaining");

    // Canonical storage/persisted settings never see an inverted value: the
    // stored object only records the perspective preference itself.
    const stored = JSON.parse(
      localStorage.getItem("rate-limits.settings.v1") ?? "{}",
    ) as Record<string, unknown>;
    expect(stored.quotaPerspective).toBe("remaining");

    await choosePerspective(user, "Used");
    expect(overviewPercent()).toBe("95% used");
  });

  it("2. Used -> Remaining flips the Usage summary values in place", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage(), grokUsage()]);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");

    const summary = within(screen.getByLabelText("Usage summary"));
    expect(summary.getByText("95% used")).toBeTruthy();

    await choosePerspective(user, "Remaining");
    // Peak and most constrained both show the derived complement.
    expect(
      summary.getAllByText("5% remaining", { selector: ".usage-metric-value" }),
    ).toHaveLength(2);
  });

  it("3. severity stays canonical on used in Remaining mode (95% used is Critical)", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage(), grokUsage()]);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");

    const summary = within(screen.getByLabelText("Usage summary"));
    expect(summary.getByText("Critical")).toBeTruthy();

    await choosePerspective(user, "Remaining");
    // 5% remaining must NOT read as healthy.
    expect(summary.getByText("Critical")).toBeTruthy();
    expect(
      summary.getAllByText("5% remaining", { selector: ".usage-metric-value" }),
    ).toHaveLength(2);
  });

  it("4. heatmap band and color class stay the used-band equivalent in Remaining mode", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Daily peak");

    const cell = heatmapCells()[0];
    expect(cell.className).toContain("usage-band-4");
    expect(cell.getAttribute("aria-label")).toContain("84% used, band 75–100%");

    await choosePerspective(user, "Remaining");
    const remainingCell = heatmapCells()[0];
    // Same color scale: the class is still the canonical used band.
    expect(remainingCell.className).toContain("usage-band-4");
    // The label may say remaining, but it names the used equivalence too.
    expect(remainingCell.getAttribute("aria-label")).toContain(
      "16% remaining (84% used), band 75–100%",
    );
  });

  it("5. near-limit thresholds stay >=80%/>=95% used in Remaining mode, with an equivalence note", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Near limit");

    const summary = within(screen.getByLabelText("Usage summary"));
    expect(summary.getByText("≥80% used · 1h 24m")).toBeTruthy();
    expect(summary.getByText("≥95% used · 18m")).toBeTruthy();
    expect(screen.queryByText(/≤20% remaining/)).toBeNull();

    await choosePerspective(user, "Remaining");
    const remainingSummary = within(screen.getByLabelText("Usage summary"));
    // The canonical thresholds and their estimated durations are unchanged…
    expect(remainingSummary.getByText("≥80% used · 1h 24m")).toBeTruthy();
    expect(remainingSummary.getByText("≥95% used · 18m")).toBeTruthy();
    // …and the perspective only adds the explained equivalence.
    expect(remainingSummary.getByText("thresholds measure used: ≤20% remaining ≈ ≥80% used")).toBeTruthy();
  });
});

describe("local data × usage (scenarios 6-11)", () => {
  it("6+7. clearing history while Usage is open re-queries and shows the empty state", async () => {
    const user = userEvent.setup();
    const view = renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    expect(screen.queryByText(/No usage history yet/)).toBeNull();

    await expandDrawer(user);
    await user.click(screen.getByRole("button", { name: "Clear usage history" }));
    await user.click(await screen.findByRole("button", { name: "Clear usage history" }));

    // The Rust runtime answers the clear with a bumped history revision in
    // the next snapshot; the rerender stands in for that push at this seam.
    view.rerender(<App />);
    expect(mocks.historyCleared).toBe(true);

    // The open Usage view re-queries and shows the honest empty state
    // without leaving the view.
    await screen.findByText(/No usage history yet/);
  });

  it("8. settings survive a history clear", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await choosePerspective(user, "Remaining");
    const before = localStorage.getItem("rate-limits.settings.v1");

    await expandDrawer(user);
    await user.click(screen.getByRole("button", { name: "Clear usage history" }));
    await user.click(await screen.findByRole("button", { name: "Clear usage history" }));
    await screen.findByText("Usage history cleared.");

    expect(localStorage.getItem("rate-limits.settings.v1")).toBe(before);
    expect(JSON.parse(before ?? "{}")).toMatchObject({ quotaPerspective: "remaining" });
  });

  it("9. reset preferences returns the perspective to Used", async () => {
    const user = userEvent.setup();
    const view = renderApp([codexUsage()]);
    await choosePerspective(user, "Remaining");
    expect(view.container.querySelector(".primary-percent")?.textContent).toBe(
      "5% remaining",
    );

    await expandDrawer(user);
    await user.click(screen.getByRole("button", { name: "Reset preferences" }));
    await user.click(await screen.findByRole("button", { name: "Reset preferences" }));
    await screen.findByText("Preferences reset.");

    const radio = screen.getByRole("radio", { name: "Used" }) as HTMLInputElement;
    expect(radio.checked).toBe(true);
    expect(view.container.querySelector(".primary-percent")?.textContent).toBe(
      "95% used",
    );
    expect(
      (JSON.parse(localStorage.getItem("rate-limits.settings.v1") ?? "{}") as Record<string, unknown>)
        .quotaPerspective,
    ).toBe("used");
  });

  it("10. reset preferences preserves usage history", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");

    await expandDrawer(user);
    await user.click(screen.getByRole("button", { name: "Reset preferences" }));
    await user.click(await screen.findByRole("button", { name: "Reset preferences" }));
    await screen.findByText("Preferences reset.");

    expect(mocks.historyCleared).toBe(false);
    // The Usage view still shows the same observations.
    expect(screen.getByText("Peak observed")).toBeTruthy();
    expect(screen.getByLabelText("Usage summary")).toBeTruthy();
  });

  it("11. reset preferences preserves execution runs", async () => {
    const user = userEvent.setup();
    // Seed a real completed run through the store API so the persisted shape
    // is exactly what production writes (the loader parses defensively).
    const started = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: codexUsage({
        limits: [{ label: "5-hour", usedPercent: 23, resetAt: hoursAhead(5) }],
      }),
      harness: "Codex",
    });
    if (started.status !== "started") throw new Error("seed start failed");
    const finished = completeExecutionRun(
      started.state,
      [
        codexUsage({
          checkedAt: minutesAgo(0),
          limits: [{ label: "5-hour", usedPercent: 28, resetAt: hoursAhead(5) }],
        }),
      ],
      { nowMs: Date.now() },
    );
    if (finished.status !== "finished") throw new Error("seed finish failed");
    saveExecutionRunsState(finished.state);
    renderApp([codexUsage()]);

    await expandDrawer(user);
    await user.click(screen.getByRole("button", { name: "Reset preferences" }));
    await user.click(await screen.findByRole("button", { name: "Reset preferences" }));
    await screen.findByText("Preferences reset.");

    const user2 = user;
    await user2.click(screen.getByRole("button", { name: "Execution run" }));
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();
  });
});

describe("local data × execution runs (scenarios 12-14)", () => {
  it("12. clear execution runs with no active run removes the recent history", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    await user.click(screen.getByRole("button", { name: "Finish run" }));
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();

    await expandDrawer(user);
      await user.click(screen.getByRole("button", { name: "Clear execution runs" }));
    expect(screen.getByText("Clear execution runs?")).toBeTruthy();
    await user.click(await screen.findByRole("button", { name: "Clear execution runs" }));
    await screen.findByText("Execution runs cleared.");

    expect(screen.queryByText(/Recent runs/)).toBeNull();
    expect(persistedRuns().recentRuns).toHaveLength(0);
  });

  it("13. an active run survives Cancel and is discarded only by confirmed clear", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();

    await expandDrawer(user);
    await user.click(screen.getByRole("button", { name: "Discard active run and clear" }));
    // The confirmation names the in-progress consequence.
    expect(
        screen.getByText(
          "This discards the active execution run and removes all stored run records. Your quota history, settings, and provider credentials are not affected.",
        ),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();
    expect(persistedRuns().activeRun).not.toBeNull();

      await user.click(screen.getByRole("button", { name: "Discard active run and clear" }));
      await user.click(
        await screen.findByRole("button", { name: "Discard active run and clear" }),
      );
    await screen.findByText("Execution runs cleared.");
    expect(persistedRuns().activeRun).toBeNull();
    expect(screen.getByRole("button", { name: "Start run" })).toBeTruthy();
  });

  it("14. clearing execution runs does not affect Usage history", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    await expandDrawer(user);
    await user.click(screen.getByRole("button", { name: "Clear execution runs" }));
    await user.click(await screen.findByRole("button", { name: "Clear execution runs" }));
    await screen.findByText("Execution runs cleared.");

    expect(mocks.historyCleared).toBe(false);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    expect(screen.getByLabelText("Usage summary")).toBeTruthy();
  });
});

describe("execution runs × navigation × usage (scenarios 15-18)", () => {
  it("15+16. a run started on Overview stays active across the Usage round-trip", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();
    // Overview is still the active view.
    expect(screen.getByRole("tabpanel")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Usage" }));
    expect(screen.getByText("Peak observed")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /Execution run in progress, running for/ }),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Overview" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();
    expect(persistedRuns().activeRun).not.toBeNull();
  });

  it("17. finishing after navigation still binds the run to its original provider", async () => {
    const user = userEvent.setup();
    // Reset timestamps are pinned across both snapshots so the cycle
    // continuity holds and the delta stays comparable.
    const reset5 = hoursAhead(5);
    const weekly = hoursAhead(100);
    const view = renderApp([
      codexUsage({
        limits: [
          { label: "5-hour", usedPercent: 23, resetAt: reset5 },
          { label: "Weekly", usedPercent: 95, resetAt: weekly },
        ],
      }),
    ]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));

    // Reorder providers and move quota mid-run; the run's binding comes from
    // its start snapshot, not from whatever is selected at finish time.
    mocks.usages = [
      grokUsage(),
      codexUsage({
        checkedAt: minutesAgo(0),
        limits: [
          { label: "5-hour", usedPercent: 28, resetAt: reset5 },
          { label: "Weekly", usedPercent: 95, resetAt: weekly },
        ],
      }),
    ];
    view.rerender(<App />);

    await user.click(screen.getByRole("button", { name: "Usage" }));
    await user.click(screen.getByRole("button", { name: "Overview" }));
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    const summary = within(screen.getByLabelText("Execution summary"));
    summary.getByText("23% → 28%");
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();
    expect(
      (persistedRuns().recentRuns[0] as { providerId?: string } | undefined)?.providerId,
    ).toBe("openai-codex");
  });

  it("18. the recent run survives Overview/Usage switching", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    await user.click(screen.getByRole("button", { name: "Finish run" }));
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    await user.click(screen.getByRole("button", { name: "Overview" }));
    // The open panel (and its recent-run history) survives the round trip.
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();
  });
});

describe("receipts from the integrated pipeline (scenarios 19-20)", () => {
  it("19. a completed integrated run produces a deterministic receipt", async () => {
    const user = userEvent.setup();
    const reset5 = hoursAhead(5);
    const weekly = hoursAhead(100);
    const view = renderApp([
      codexUsage({
        limits: [
          { label: "5-hour", usedPercent: 23, resetAt: reset5 },
          { label: "Weekly", usedPercent: 38, resetAt: weekly },
        ],
      }),
    ]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    mocks.usages = [
      codexUsage({
        checkedAt: minutesAgo(0),
        limits: [
          { label: "5-hour", usedPercent: 28, resetAt: reset5 },
          { label: "Weekly", usedPercent: 40, resetAt: weekly },
        ],
      }),
    ];
    view.rerender(<App />);
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    const run = persistedRuns().recentRuns[0];
    expect(run).toBeTruthy();
    const receipt = createExecutionReceipt(run);
    expect(receipt).toBeTruthy();
    const json = formatReceiptJson(receipt!);
    expect(formatReceiptJson(createExecutionReceipt(persistedRuns().recentRuns[0])!)).toBe(json);
    const parsed = JSON.parse(json) as Record<string, unknown>;
    expect(parsed.kind).toBe("limitscope-execution-receipt");
    expect(parsed.schemaVersion).toBe(1);
  });

  it("20. a reset-crossing integrated run yields a reset-crossed receipt without negative deltas", async () => {
    const user = userEvent.setup();
    const view = renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));

    mocks.usages = [
      codexUsage({
        checkedAt: minutesAgo(0),
        limits: [
          { label: "5-hour", usedPercent: 4, resetAt: hoursAhead(6) },
          { label: "Weekly", usedPercent: 95, resetAt: hoursAhead(100) },
        ],
      }),
    ];
    view.rerender(<App />);
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    expect(screen.getAllByText(/not comparable/i).length).toBeGreaterThan(0);

    const run = persistedRuns().recentRuns[0];
    const receipt = createExecutionReceipt(run)!;
    const json = formatReceiptJson(receipt);
    expect(json).toContain("reset-crossed");
    // The naive 5-hour delta here is -19 (23% -> 4%). Assert on the parsed
    // windows instead of raw substrings: run IDs embed the UTC start hour, so
    // "-19" collides with any run minted in the 19:xx hour.
    const parsedReceipt = JSON.parse(json) as {
      windows: Array<{ label: string; deltaPoints: number | null; status: string }>;
    };
    expect(
      parsedReceipt.windows.every((w) => w.deltaPoints === null || w.deltaPoints >= 0),
    ).toBe(true);
    const fiveHour = parsedReceipt.windows.find((w) => w.label === "5-hour");
    expect(fiveHour?.status).toBe("reset-crossed");
    expect(fiveHour?.deltaPoints).toBeNull();
    // The disclaimer wording is the markdown-facing form of the receipt.
    expect(formatReceiptMarkdown(receipt)).toContain("Not exact task cost");
  });
});

describe("ProviderUsage DTO coexistence (scenarios 21-23)", () => {
  it("21. reset credits and planType coexist on a provider through Overview and Usage", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage(), grokUsage()]);

    // Overview renders the attributed, plan-typed provider.
    expect(screen.getByRole("tab", { name: /OpenAI \/ Codex/ })).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    // The analytics surface is unaffected by the extra DTO fields.
    expect(screen.getByText("95% used")).toBeTruthy();
    // The reset-credit payload travels with the usage object untouched.
    expect(mocks.usages[0]?.resetCredits?.bankedCredits).toBe(3);
    expect(mocks.usages[0]?.planType).toBe("team");
  });

  it("22. absent reset credits change nothing in the Usage surface", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage({ resetCredits: undefined }), grokUsage()]);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    expect(screen.getByLabelText("Usage summary")).toBeTruthy();
    expect(screen.getByText("95% used")).toBeTruthy();
  });

  it("23. the account identity surfaces only in its masked display form", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");

    const summary = within(screen.getByLabelText("Usage summary"));
    // The masked label (fingerprint scheme + tail) is display-safe…
    expect(summary.getAllByText(/key:3456/).length).toBeGreaterThan(0);
    // …and no raw identity material beyond it is rendered anywhere.
    expect(document.body.textContent).not.toContain("sk-");
  });
});

describe("settings schema robustness (scenarios 24-25)", () => {
  const FUTURE = JSON.stringify({
    launchAtStartup: false,
    refreshIntervalMinutes: 15,
    theme: "oled",
    quotaNotifications: true,
    quotaPerspective: "used",
    providerPreferences: { order: [], hidden: [] },
    futureLaneField: { nested: [1, 2, 3] },
  });

  it("24. an unknown future setting survives a quota-perspective save", async () => {
    const user = userEvent.setup();
    localStorage.setItem("rate-limits.settings.v1", FUTURE);
    renderApp([codexUsage()]);

    await choosePerspective(user, "Remaining");

    const stored = JSON.parse(
      localStorage.getItem("rate-limits.settings.v1") ?? "{}",
    ) as Record<string, unknown>;
    expect(stored.quotaPerspective).toBe("remaining");
    expect(stored.futureLaneField).toEqual({ nested: [1, 2, 3] });
    expect(stored.theme).toBe("oled");
    expect(stored.quotaNotifications).toBe(true);
  });

  it("25. an unknown future setting survives a preference reset", async () => {
    const user = userEvent.setup();
    localStorage.setItem("rate-limits.settings.v1", FUTURE.replace("used", "remaining"));
    renderApp([codexUsage()]);

    await expandDrawer(user);
    await user.click(screen.getByRole("button", { name: "Reset preferences" }));
    await user.click(await screen.findByRole("button", { name: "Reset preferences" }));
    await screen.findByText("Preferences reset.");

    const stored = JSON.parse(
      localStorage.getItem("rate-limits.settings.v1") ?? "{}",
    ) as Record<string, unknown>;
    // Canonical fields return to defaults…
    expect(stored.quotaPerspective).toBe("used");
    expect(stored.theme).toBe("graphite");
    // …while the foreign field survives untouched.
    expect(stored.futureLaneField).toEqual({ nested: [1, 2, 3] });
  });
});
