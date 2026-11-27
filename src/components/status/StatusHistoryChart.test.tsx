// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { StatusHistoryChart } from "./StatusHistoryChart";
import type {
  UsageAnalytics,
  UsageAnalyticsQuery,
  UsageTrendSeries,
} from "../../lib/usageAnalytics";
import type { ProviderUsage } from "../../types";

const HOUR = 60 * 60 * 1000;
const NOW = Date.parse("2026-09-28T22:00:00Z");

const usage: ProviderUsage = {
  id: "openai-codex",
  name: "OpenAI / Codex",
  status: "ok",
  health: "live",
  checkedAt: new Date(NOW).toISOString(),
  limits: [{ label: "Weekly credits", usedPercent: 78 }],
};

function series(overrides: Partial<UsageTrendSeries> = {}): UsageTrendSeries {
  return {
    providerId: "openai-codex",
    windowLabel: "Weekly credits",
    points: [
      {
        observedAt: new Date(NOW - 12 * HOUR).toISOString(),
        usedPercent: 38,
        cycleId: "c1",
        cycleStart: true,
        resetBoundary: false,
        resolution: "detailed",
      },
      {
        observedAt: new Date(NOW - 4 * HOUR).toISOString(),
        usedPercent: 78,
        cycleId: "c1",
        cycleStart: false,
        resetBoundary: false,
        resolution: "detailed",
      },
    ],
    coverage: {
      gapThresholdMs: 9 * HOUR,
      comparableSpanMs: 8 * HOUR,
      comparableSpanRatio: 0.33,
      gaps: [],
    },
    ...overrides,
  };
}

function analytics(trends: UsageTrendSeries[]): UsageAnalytics {
  return {
    schemaVersion: 1,
    range: "24h",
    rangeStart: new Date(NOW - 24 * HOUR).toISOString(),
    rangeEnd: new Date(NOW).toISOString(),
    generatedAt: new Date(NOW).toISOString(),
    timezoneOffsetMinutes: 0,
    // The projection's Codex session scope. Generation 1 is newer than the
    // provider's fallback expectation (generation 0), so the payload is
    // accepted by the session fence.
    codexIdentityScope: [
      { providerId: "openai-codex", generation: 1, identity: null },
    ],
    summary: {
      observedResetCycles: {
        count: 0,
        exactness: "lowerBound",
        detection: "historyResetBoundaryTransitions",
      },
      observedDays: { count: 1, timezoneOffsetMinutes: 0 },
    },
    heatmap: [],
    trends,
    gapSemantics: "notObservedNeverZeroFilled",
    availabilityInference: "none",
  };
}

function emptyAnalytics(): UsageAnalytics {
  return analytics([]);
}

describe("Status history chart", () => {
  let loader: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    loader = vi.fn(async (_query: UsageAnalyticsQuery) => analytics([series()]));
  });
  afterEach(cleanup);

  function renderChart(props: Partial<Parameters<typeof StatusHistoryChart>[0]> = {}) {
    return render(
      <StatusHistoryChart
        usage={usage}
        historyRevision={1}
        loader={loader}
        {...props}
      />,
    );
  }

  it("queries 24h scoped to the provider on mount and renders one large chart per window", async () => {
    renderChart();
    const region = await screen.findByLabelText("Quota history");
    expect(loader).toHaveBeenCalledWith({ range: "24h", providerId: "openai-codex" });
    const chart = within(region).getByText("Codex · Weekly credits");
    expect(chart).toBeTruthy();
    // The large chart carries the labeled y-axis.
    const svg = document.querySelector("svg.usage-chart")!;
    expect(svg.getAttribute("aria-label")).toContain("percent remaining 0 to 100");
    expect(svg.getAttribute("aria-label")).toContain("lowest remaining 22%");
    expect(svg.querySelector(".usage-chart-line")?.getAttribute("d")).toBe(
      "M302.00 66.88 L503.33 137.28",
    );
    expect(Number(svg.querySelector(".usage-chart-guide")?.getAttribute("y1"))).toBeCloseTo(140.8);
    expect(within(region).getByText(/20% remaining near-limit threshold/)).toBeTruthy();
    const axisLabels = Array.from(
      svg.querySelectorAll(".usage-chart-axis-label"),
    ).map((label) => label.textContent);
    expect(axisLabels).toEqual(["100", "50", "0"]);
  });

  it("renders one large chart per quota window", async () => {
    loader = vi.fn(async () =>
      analytics([
        series({ windowLabel: "5-hour window" }),
        series(),
      ]),
    );
    renderChart();
    await screen.findByLabelText("Quota history");
    const figures = document.querySelectorAll("figure.large-trend-chart");
    expect(figures).toHaveLength(2);
  });

  it("re-queries on the range toggle", async () => {
    renderChart();
    await screen.findByLabelText("Quota history");
    fireEvent.click(screen.getByRole("button", { name: "7d" }));
    await waitFor(() =>
      expect(loader).toHaveBeenLastCalledWith({
        range: "7d",
        providerId: "openai-codex",
      }),
    );
  });

  it("re-queries when the provider changes", async () => {
    const { rerender } = renderChart();
    await screen.findByLabelText("Quota history");
    const grok: ProviderUsage = {
      ...usage,
      id: "grok",
      name: "Grok (xAI)",
      limits: [{ label: "Weekly", usedPercent: 13 }],
    };
    rerender(
      <StatusHistoryChart usage={grok} historyRevision={1} loader={loader} />,
    );
    await waitFor(() =>
      expect(loader).toHaveBeenLastCalledWith({ range: "24h", providerId: "grok" }),
    );
  });

  it("does not show the previous provider while the next query is pending", async () => {
    loader = vi.fn()
      .mockResolvedValueOnce(analytics([series()]))
      .mockImplementationOnce(() => new Promise(() => {}));
    const { rerender } = renderChart();
    await screen.findByText("Codex · Weekly credits");
    rerender(
      <StatusHistoryChart
        usage={{ ...usage, id: "grok", name: "Grok" }}
        historyRevision={1}
        loader={loader}
      />,
    );
    expect(screen.queryByText("Codex · Weekly credits")).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("Loading history");
  });

  it("shows the insufficient-data state without inventing a chart", async () => {
    loader = vi.fn(async () => emptyAnalytics());
    renderChart();
    await screen.findByText("No usage history yet for Codex.");
    expect(
      screen.getByText(/The chart appears as quota observations are recorded/),
    ).toBeTruthy();
    expect(document.querySelectorAll("svg.usage-chart")).toHaveLength(0);
  });

  it("isolates a failed analytics query with a retry action", async () => {
    loader = vi.fn(async () => {
      throw new Error("history store unavailable");
    });
    renderChart();
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/history store unavailable/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(2));
  });

  // Account fence: the Codex history card is read under the runtime's
  // observed identity scope. A projection from another account (same
  // generation, different identity) must never render; the matching one does.
  it("discards a Codex projection stamped with a different account identity", async () => {
    loader = vi.fn(async () => ({
      ...analytics([series()]),
      codexIdentityScope: [
        { providerId: "openai-codex", generation: 3, identity: "account-b" },
      ],
    }));
    renderChart({
      codexIdentityStates: [
        { providerId: "openai-codex", generation: 3, identity: "account-a" },
      ],
    });
    await waitFor(() => expect(loader).toHaveBeenCalled());
    await vi.waitFor(() =>
      expect(screen.queryByText("Loading history…")).toBeNull(),
    );
    expect(screen.queryByText("Codex · Weekly credits")).toBeNull();
    expect(document.querySelectorAll("svg.usage-chart")).toHaveLength(0);
  });

  it("renders a Codex projection stamped with the observed account identity", async () => {
    loader = vi.fn(async () => ({
      ...analytics([series()]),
      codexIdentityScope: [
        { providerId: "openai-codex", generation: 3, identity: "account-a" },
      ],
    }));
    renderChart({
      codexIdentityStates: [
        { providerId: "openai-codex", generation: 3, identity: "account-a" },
        // Another slot's state must not widen or narrow this card's fence.
        { providerId: "openai-codex-2", generation: 9, identity: "other" },
      ],
    });
    expect(await screen.findByText("Codex · Weekly credits")).toBeTruthy();
  });

  it("keeps the history range toggle keyboard operable", async () => {
    renderChart();
    const group = await screen.findByRole("group", { name: "History range" });
    const sevenDays = within(group).getByRole("button", { name: "7d" });
    expect(sevenDays.getAttribute("type")).toBe("button");
    sevenDays.focus();
    expect(document.activeElement).toBe(sevenDays);
    // Native buttons activate on Enter/Space via a click event.
    fireEvent.click(sevenDays);
    await waitFor(() =>
      expect(loader).toHaveBeenLastCalledWith({ range: "7d", providerId: "openai-codex" }),
    );
    expect(sevenDays.getAttribute("aria-pressed")).toBe("true");
  });
});
