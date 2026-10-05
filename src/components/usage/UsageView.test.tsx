// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { UsageView } from "./UsageView";
import type {
  UsageAnalytics,
  UsageAnalyticsQuery,
  UsageHeatmapDay,
  UsageTrendPoint,
  UsageTrendSeries,
} from "../../lib/usageAnalytics";
import type { ProviderUsage } from "../../types";

const HOUR = 60 * 60 * 1000;
const NOW = Date.parse("2026-09-28T22:00:00Z");

function trendPoint(
  hoursAgo: number,
  usedPercent: number,
  cycleId = "c1",
  extra: Partial<UsageTrendPoint> = {},
): UsageTrendPoint {
  return {
    observedAt: new Date(NOW - hoursAgo * HOUR).toISOString(),
    usedPercent,
    cycleId,
    cycleStart: false,
    resetBoundary: false,
    resolution: "detailed",
    ...extra,
  };
}

function makeSeries(overrides: Partial<UsageTrendSeries> = {}): UsageTrendSeries {
  return {
    providerId: "openai-codex",
    windowLabel: "Weekly credits",
    points: [
      trendPoint(20, 38, "c1", { cycleStart: true }),
      trendPoint(12, 61),
      trendPoint(4, 84),
    ],
    coverage: {
      // Above the fixture's 8h point spacing so the base series connects.
      gapThresholdMs: 9 * HOUR,
      comparableSpanMs: 16 * HOUR,
      comparableSpanRatio: 0.66,
      gaps: [],
    },
    ...overrides,
  };
}

/** Full backend-shaped payload mirroring the accepted contract. */
function makeAnalytics(overrides: {
  range?: "24h" | "7d";
  peakExactness?: "exact" | "lowerBound";
  peakUsedPercent?: number;
  trends?: UsageTrendSeries[];
  heatmapObserved?: boolean[];
} = {}): UsageAnalytics {
  const trends = overrides.trends ?? [makeSeries()];
  const peakUsed = overrides.peakUsedPercent ?? 84;
  const exactness = overrides.peakExactness ?? "exact";
  const heatmap: UsageHeatmapDay[] = (overrides.heatmapObserved ?? [true, false]).map(
    (observed, index) =>
      observed
        ? {
            date: index === 0 ? "2026-09-28" : "2026-09-27",
            observed: true,
            peakUsedPercent: peakUsed,
            peakObservedAt: new Date(NOW - 4 * HOUR).toISOString(),
            providerId: "openai-codex",
            windowLabel: "Weekly credits",
            band: 4,
            peakExactness: exactness,
          }
        : { date: index === 0 ? "2026-09-28" : "2026-09-27", observed: false },
  );
  return {
    schemaVersion: 1,
    range: overrides.range ?? "24h",
    rangeStart: new Date(NOW - 24 * HOUR).toISOString(),
    rangeEnd: new Date(NOW).toISOString(),
    generatedAt: new Date(NOW).toISOString(),
    timezoneOffsetMinutes: 0,
    summary: {
      peakObservedUsage: {
        usedPercent: peakUsed,
        providerId: "openai-codex",
        windowLabel: "Weekly credits",
        observedAt: new Date(NOW - 4 * HOUR).toISOString(),
        exactness,
      },
      mostConstrainedWindow: {
        usedPercent: peakUsed,
        providerId: "openai-codex",
        windowLabel: "Weekly credits",
        observedAt: new Date(NOW - 4 * HOUR).toISOString(),
        exactness,
      },
      observedResetCycles: {
        count: 1,
        exactness: "lowerBound",
        detection: "historyResetBoundaryTransitions",
      },
      observedDays: { count: 1, timezoneOffsetMinutes: 0 },
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
    heatmap,
    trends,
    gapSemantics: "notObservedNeverZeroFilled",
    availabilityInference: "none",
  };
}

function emptyAnalytics(range: "24h" | "7d" = "24h"): UsageAnalytics {
  return {
    ...makeAnalytics({ range }),
    summary: {
      peakObservedUsage: undefined,
      mostConstrainedWindow: undefined,
      observedResetCycles: {
        count: 0,
        exactness: "lowerBound",
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

const usages: ProviderUsage[] = [
  {
    id: "openai-codex",
    name: "OpenAI / Codex",
    status: "ok",
    health: "live",
    checkedAt: new Date(NOW).toISOString(),
    limits: [
      { label: "5-hour window", usedPercent: 62 },
      { label: "Weekly credits", usedPercent: 84 },
    ],
  },
  {
    id: "grok",
    name: "Grok (xAI)",
    status: "ok",
    health: "live",
    checkedAt: new Date(NOW).toISOString(),
    limits: [{ label: "Weekly", usedPercent: 13 }],
  },
];

describe("Usage view presentation semantics", () => {
  let loader: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    loader = vi.fn(async (_query: UsageAnalyticsQuery) => makeAnalytics());
  });

  afterEach(cleanup);

  function renderView(props: Partial<Parameters<typeof UsageView>[0]> = {}) {
    return render(
      <UsageView
        usages={usages}
        scopeProviderId={null}
        onScopeProvider={vi.fn()}
        historyRevision={1}
        loader={loader}
        {...props}
      />,
    );
  }

  it("queries 24h on mount with the normalized query", async () => {
    renderView();
    await screen.findByText("Peak observed");
    expect(loader).toHaveBeenCalledTimes(1);
    expect(loader).toHaveBeenCalledWith({ range: "24h" });
  });

  it("queries 7d when the range control is pressed, keeping prior data visible", async () => {
    const user = userEvent.setup();
    // Hold the 7d response so the refreshing state is observable.
    let resolve7d!: (analytics: UsageAnalytics) => void;
    loader.mockImplementationOnce(async () => makeAnalytics());
    loader.mockImplementationOnce(
      () =>
        new Promise<UsageAnalytics>((resolve) => {
          resolve7d = resolve;
        }),
    );
    renderView();
    await screen.findByText("Peak observed");
    const sevenDay = screen.getByRole("button", { name: "7d" });
    expect(sevenDay.getAttribute("aria-pressed")).toBe("false");
    sevenDay.focus();
    await user.keyboard("{Enter}");
    expect(sevenDay.getAttribute("aria-pressed")).toBe("true");
    await waitFor(() =>
      expect(loader).toHaveBeenLastCalledWith({ range: "7d" }),
    );
    // The previous payload stays on screen while refreshing (no empty flash).
    expect(screen.getByText("Updating…")).toBeTruthy();
    resolve7d(makeAnalytics({ range: "7d" }));
    await waitFor(() => expect(screen.queryByText("Updating…")).toBeNull());
  });

  it("renders an exact peak without any lower-bound qualifier", async () => {
    renderView();
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("84%")).toBeTruthy();
    // Scope to the peak metric: the reset count is (correctly) a lower bound.
    const peakMetric = within(summary)
      .getByText("Peak observed")
      .closest(".usage-metric") as HTMLElement;
    expect(within(peakMetric).queryByText("lower bound")).toBeNull();
    expect(within(peakMetric).queryByText("84%+")).toBeNull();
  });

  it("renders a 7d lower-bound peak as '84%+' with visible 'lower bound' text", async () => {
    loader.mockImplementation(async () =>
      makeAnalytics({
        range: "7d",
        peakExactness: "lowerBound",
        peakUsedPercent: 91,
      }),
    );
    renderView({ historyRevision: 2 });
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("91%+")).toBeTruthy();
    // Peak badge + the always-lower-bound reset count: at least one each,
    // never rendered as exact.
    expect(within(summary).getAllByText("lower bound").length).toBeGreaterThanOrEqual(2);
  });

  it("renders the most constrained window with provider, window, and used wording", async () => {
    renderView();
    const summary = await screen.findByLabelText("Usage summary");
    expect(
      within(summary).getAllByText(/Codex · Weekly credits/).length,
    ).toBeGreaterThanOrEqual(2);
    expect(within(summary).getByText("84% used")).toBeTruthy();
  });

  it("labels resets as observed counts, never as resets remaining", async () => {
    renderView();
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("Observed resets")).toBeTruthy();
    expect(within(summary).queryByText(/remaining/i)).toBeNull();
    expect(within(summary).queryByText(/available/i)).toBeNull();
    // The backend marks the reset count a lower bound; the text must say so.
    expect(within(summary).getAllByText("lower bound").length).toBeGreaterThan(0);
  });

  it("shows observed days against the heatmap span", async () => {
    renderView();
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("Observed days")).toBeTruthy();
    expect(within(summary).getByText("of the last 2 days")).toBeTruthy();
  });

  it("marks near-limit time as an estimate with the ≥95% breakdown", async () => {
    renderView();
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("Near limit")).toBeTruthy();
    expect(within(summary).getByText("~1h 24m")).toBeTruthy();
    expect(within(summary).getByText("est.")).toBeTruthy();
    expect(within(summary).getByText("≥80% used · 1h 24m")).toBeTruthy();
    expect(within(summary).getByText("≥95% used · 18m")).toBeTruthy();
  });

  it("shows the explicit no-history state without inventing charts", async () => {
    loader.mockImplementation(async () => emptyAnalytics());
    renderView();
    await screen.findByText("No usage history yet.");
    expect(
      screen.getByText(
        "LimitScope will build this view as quota observations are recorded.",
      ),
    ).toBeTruthy();
    expect(document.querySelectorAll("svg.usage-chart")).toHaveLength(0);
  });

  it("names a provider with no recorded history", async () => {
    loader.mockImplementation(async () => emptyAnalytics());
    renderView({ scopeProviderId: "openai-codex" });
    await screen.findByText("No usage history yet for Codex.");
  });

  it("renders a gap as not-observed (broken line, no zero-fill)", async () => {
    const gapped = makeSeries({
      windowLabel: "5-hour window",
      points: [
        trendPoint(22, 12, "c1", { cycleStart: true }),
        trendPoint(21.5, 21, "c1"),
        // 4h of nothing: beyond the gap threshold.
        trendPoint(17, 47, "c1"),
        trendPoint(16.5, 58, "c1"),
      ],
      coverage: {
        gapThresholdMs: 45 * 60 * 1000,
        comparableSpanMs: 1 * HOUR,
        comparableSpanRatio: 0.04,
        gaps: [
          {
            from: new Date(NOW - 21.5 * HOUR).toISOString(),
            to: new Date(NOW - 17 * HOUR).toISOString(),
            durationMs: 4.5 * HOUR,
            kind: "notObserved",
          },
        ],
      },
    });
    loader.mockImplementation(async () => makeAnalytics({ trends: [gapped] }));
    renderView();
    await screen.findByText("Usage trend");
    // The gap keeps the line broken: two drawn segments, a hatched span…
    expect(document.querySelectorAll("path.usage-chart-line")).toHaveLength(2);
    expect(document.querySelectorAll("rect.usage-chart-gap")).toHaveLength(1);
    // …an explicit "not observed" tooltip, never a zero…
    const gapRect = document.querySelector("rect.usage-chart-gap")!;
    expect(gapRect.querySelector("title")?.textContent).toBe("Not observed");
    // …and no 0% point anywhere in the chart's spoken or visible summary.
    const chart = document.querySelector("svg.usage-chart")!;
    expect(chart.getAttribute("aria-label")).toContain("1 span not observed");
    expect(chart.getAttribute("aria-label")).not.toContain("0%");
  });

  it("renders an unobserved heatmap day as distinct from a 0% day", async () => {
    renderView();
    await screen.findByText("Daily peak");
    const heatmap = screen.getByLabelText("Daily peak observed usage");
    const cells = within(heatmap).getAllByRole("listitem");
    expect(cells).toHaveLength(2);
    expect(cells[1].getAttribute("aria-label")).toMatch(/not observed$/);
    expect(cells[1].className).toContain("usage-day-none");
    expect(cells[1].textContent).not.toContain("0%");
    expect(cells[0].className).toContain("usage-band-4");
  });

  it("colors heatmap days by the backend's fixed absolute band", async () => {
    loader.mockImplementation(async () => {
      const analytics = makeAnalytics();
      analytics.heatmap = [
        {
          date: "2026-09-28",
          observed: true,
          peakUsedPercent: 30,
          peakObservedAt: new Date(NOW - 4 * HOUR).toISOString(),
          providerId: "openai-codex",
          windowLabel: "Weekly credits",
          band: 2,
          peakExactness: "exact",
        },
        { date: "2026-09-27", observed: false },
      ];
      return analytics;
    });
    renderView();
    await screen.findByText("Daily peak");
    const heatmap = screen.getByLabelText("Daily peak observed usage");
    const observed = within(heatmap).getAllByRole("listitem")[0];
    // band 2 stays band 2 — presentation never re-derives from the value.
    expect(observed.className).toContain("usage-band-2");
    expect(observed.getAttribute("aria-label")).toContain("band 25–50%");
  });

  it("breaks the trend at reset boundaries and marks them as shapes", async () => {
    loader.mockImplementation(async () =>
      makeAnalytics({
        trends: [
          makeSeries({
            points: [
              trendPoint(20, 88, "cw-old", { cycleStart: true }),
              trendPoint(16, 93, "cw-old"),
              trendPoint(12, 3, "cw-new", { cycleStart: true, resetBoundary: true }),
              trendPoint(8, 11, "cw-new"),
            ],
            coverage: {
              gapThresholdMs: 5 * HOUR,
              comparableSpanMs: 8 * HOUR,
              comparableSpanRatio: 0.33,
              gaps: [],
            },
          }),
        ],
      }),
    );
    renderView();
    await screen.findByText("Usage trend");
    expect(document.querySelectorAll("path.usage-chart-line")).toHaveLength(2);
    expect(document.querySelectorAll("path.usage-chart-reset")).toHaveLength(1);
    const chart = document.querySelector("svg.usage-chart")!;
    expect(chart.getAttribute("aria-label")).toContain("1 reset");
  });

  it("scopes the query when the parent passes a provider scope", async () => {
    const onScopeProvider = vi.fn();
    renderView({ scopeProviderId: "grok", onScopeProvider });
    await screen.findByText("Peak observed");
    expect(loader).toHaveBeenCalledWith({ range: "24h", providerId: "grok" });
    // "All providers" hands the scope back to the parent.
    fireEvent.click(screen.getByRole("button", { name: "All providers" }));
    expect(onScopeProvider).toHaveBeenCalledWith(null);
  });

  it("filters by window from the runtime's own quota windows", async () => {
    const user = userEvent.setup();
    renderView();
    await screen.findByText("Peak observed");
    const select = screen.getByLabelText("Window");
    const options = Array.from(select.querySelectorAll("option")).map(
      (option) => option.value,
    );
    // Options come from the scoped providers' runtime windows.
    expect(options).toContain("Weekly credits");
    expect(options).toContain("5-hour window");
    expect(options).toContain("Weekly");
    await user.selectOptions(select, "Weekly credits");
    await waitFor(() =>
      expect(loader).toHaveBeenLastCalledWith({
        range: "24h",
        windowLabel: "Weekly credits",
      }),
    );
  });

  it("keeps the used orientation canonical and derives remaining for text", async () => {
    renderView();
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("84% used")).toBeTruthy();
    expect(within(summary).getByText("16% left")).toBeTruthy();
    const chart = document.querySelector("svg.usage-chart")!;
    expect(chart.getAttribute("aria-label")).toMatch(/84% used|percent used/);
  });

  it("keeps severity canonical on used percent — 95% used is Critical at 5% remaining", async () => {
    loader.mockImplementation(async () => makeAnalytics({ peakUsedPercent: 95 }));
    renderView();
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("Critical")).toBeTruthy();
    expect(within(summary).getByText("95% used")).toBeTruthy();
    expect(within(summary).getByText("5% left")).toBeTruthy();
  });

  it("shows the mid-range High severity for an 84% peak", async () => {
    renderView();
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("High")).toBeTruthy();
  });

  it("gives every chart a textual summary on screen and for screen readers", async () => {
    renderView();
    await screen.findByText("Usage trend");
    const chart = document.querySelector("svg.usage-chart")!;
    expect(chart.getAttribute("role")).toBe("img");
    expect(chart.getAttribute("aria-label")).toContain(
      "Codex · Weekly credits usage",
    );
    expect(chart.getAttribute("aria-label")).toContain("peak 84% used");
    const summaries = document.querySelectorAll("p.usage-trend-summary");
    expect(summaries[0].textContent).toContain("peak 84% used");
  });

  it("bounds the small-multiples grid and names the hidden remainder", async () => {
    const trends = Array.from({ length: 8 }, (_, index) =>
      makeSeries({ windowLabel: `Window ${index}` }),
    );
    loader.mockImplementation(async () => makeAnalytics({ trends }));
    renderView();
    await screen.findByText("Usage trend");
    expect(document.querySelectorAll("svg.usage-chart")).toHaveLength(6);
    expect(screen.getByText(/2 more windows/)).toBeTruthy();
  });

  it("shows the error state with retry when the analytics query fails", async () => {
    loader.mockImplementation(async () => {
      throw new Error("history store unavailable");
    });
    renderView();
    await screen.findByRole("alert");
    expect(screen.getByText(/history store unavailable/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });
});
