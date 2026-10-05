import { describe, expect, it } from "vitest";
import type {
  UsageAnalytics,
  UsageHeatmapDay,
  UsageTrendPoint,
  UsageTrendSeries,
} from "./usageAnalytics";
import {
  exactnessCaption,
  formatEstimateDuration,
  formatDayCell,
  formatDayKey,
  hasAnyObservation,
  heatmapBandLabel,
  heatmapCellAriaLabel,
  nearLimitPresentation,
  peakDisplayValue,
  remainingPercent,
  severityTone,
  severityWord,
  trendGeometry,
  trendShape,
  trendSummaryText,
  visibleTrendSeries,
} from "./usagePresentation";

const DAY_MS = 24 * 60 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const RANGE_START = Date.parse("2026-09-28T00:00:00Z");
const RANGE_END = RANGE_START + DAY_MS;

function point(
  at: string,
  usedPercent: number,
  cycleId = "c1",
  extra: Partial<UsageTrendPoint> = {},
): UsageTrendPoint {
  return {
    observedAt: at,
    usedPercent,
    cycleId,
    cycleStart: false,
    resetBoundary: false,
    resolution: "detailed",
    ...extra,
  };
}

function makeSeries(
  points: UsageTrendPoint[],
  gapThresholdMs = 2 * HOUR_MS,
): UsageTrendSeries {
  const gaps = [];
  for (let index = 1; index < points.length; index += 1) {
    const from = Date.parse(points[index - 1].observedAt);
    const to = Date.parse(points[index].observedAt);
    if (
      points[index - 1].cycleId === points[index].cycleId &&
      to - from > gapThresholdMs
    ) {
      gaps.push({
        from: points[index - 1].observedAt,
        to: points[index].observedAt,
        durationMs: to - from,
        kind: "notObserved" as const,
      });
    }
  }
  return {
    providerId: "openai-codex",
    windowLabel: "Weekly credits",
    points,
    coverage: {
      gapThresholdMs,
      comparableSpanMs: 0,
      comparableSpanRatio: 0,
      gaps,
    },
  };
}

describe("exact vs lower-bound semantics", () => {
  it("adds no qualifier text to exact peaks", () => {
    expect(exactnessCaption("exact")).toBeUndefined();
  });

  it("always renders 'lower bound' as text for lower-bound peaks", () => {
    expect(exactnessCaption("lowerBound")).toBe("lower bound");
  });

  it("shows the + suffix only on lower-bound peak values", () => {
    expect(
      peakDisplayValue({
        usedPercent: 84,
        providerId: "openai-codex",
        windowLabel: "Weekly credits",
        observedAt: "2026-09-28T14:10:00Z",
        exactness: "exact",
      }),
    ).toBe("84%");
    expect(
      peakDisplayValue({
        usedPercent: 84.4,
        providerId: "openai-codex",
        windowLabel: "Weekly credits",
        observedAt: "2026-09-28T14:10:00Z",
        exactness: "lowerBound",
      }),
    ).toBe("84%+");
  });
});

describe("used/remaining and canonical severity", () => {
  it("derives remaining as 100 − used without mutating the analytics value", () => {
    expect(remainingPercent(84)).toBe(16);
    expect(remainingPercent(100)).toBe(0);
  });

  it("keeps severity canonical on the used percentage", () => {
    // 95% used = 5% remaining: still Critical.
    expect(severityWord(95)).toBe("Critical");
    expect(severityWord(94.9)).toBe("High");
    expect(severityWord(80)).toBe("High");
    expect(severityWord(79.9)).toBeUndefined();
    expect(severityTone(97)).toBe("critical");
  });
});

describe("estimated near-limit durations", () => {
  it("formats honest compact durations", () => {
    expect(formatEstimateDuration(0)).toBe("0m");
    expect(formatEstimateDuration(30 * 1000)).toBe("<1m");
    expect(formatEstimateDuration(18 * 60 * 1000)).toBe("18m");
    expect(formatEstimateDuration(84 * 60 * 1000)).toBe("1h 24m");
    expect(formatEstimateDuration(26 * HOUR_MS)).toBe("1d 2h");
  });

  it("is absent when the backend omits the estimate", () => {
    expect(nearLimitPresentation(undefined)).toBeUndefined();
  });

  it("keeps the estimated flag and both threshold lines from the backend", () => {
    const presentation = nearLimitPresentation({
      estimated: true,
      method: "piecewiseLinearBetweenAdjacentSameCycleObservations",
      intervalPolicy: "onlyIntervalsAtOrBelowPerSeriesGapThreshold",
      comparableSpanMs: 20 * HOUR_MS,
      comparableSpanRatio: 0.8,
      estimates: [
        { thresholdPercent: 80, estimatedDurationMs: 84 * 60 * 1000, estimatedShare: 0.07 },
        { thresholdPercent: 95, estimatedDurationMs: 18 * 60 * 1000, estimatedShare: 0.015 },
      ],
    });
    expect(presentation?.estimated).toBe(true);
    expect(presentation?.primary).toEqual({ thresholdPercent: 80, text: "1h 24m" });
    expect(presentation?.secondary).toEqual({ thresholdPercent: 95, text: "18m" });
  });

  it("survives a missing ≥95% estimate", () => {
    const presentation = nearLimitPresentation({
      estimated: true,
      method: "piecewiseLinearBetweenAdjacentSameCycleObservations",
      intervalPolicy: "onlyIntervalsAtOrBelowPerSeriesGapThreshold",
      comparableSpanMs: 5 * HOUR_MS,
      comparableSpanRatio: 0.2,
      estimates: [
        { thresholdPercent: 80, estimatedDurationMs: 0, estimatedShare: 0 },
      ],
    });
    expect(presentation?.primary.text).toBe("0m");
    expect(presentation?.secondary).toBeUndefined();
  });
});

describe("heatmap fixed absolute bands", () => {
  const day = (
    overrides: Partial<UsageHeatmapDay>,
  ): UsageHeatmapDay => ({
    date: "2026-09-28",
    observed: true,
    peakUsedPercent: 84,
    band: 4,
    peakExactness: "exact",
    ...overrides,
  });

  it("labels the four fixed physical bands", () => {
    expect(heatmapBandLabel(day({ band: 1 }))).toBe("0–25%");
    expect(heatmapBandLabel(day({ band: 2 }))).toBe("25–50%");
    expect(heatmapBandLabel(day({ band: 3 }))).toBe("50–75%");
    expect(heatmapBandLabel(day({ band: 4 }))).toBe("75–100%");
  });

  it("trusts the backend band instead of recomputing from values", () => {
    // The band field is the contract; presentation never re-derives it from
    // the peak (and so can never normalize against the user's own maximum).
    expect(heatmapBandLabel(day({ peakUsedPercent: 90, band: 2 }))).toBe("25–50%");
  });

  it("reads an unobserved day as not observed, never zero", () => {
    expect(heatmapBandLabel(day({ observed: false, band: undefined, peakUsedPercent: undefined }))).toBe(
      "Not observed",
    );
  });

  it("builds accessible cell labels with band, value, and exactness", () => {
    const label = heatmapCellAriaLabel(day({}));
    expect(label).toMatch(/peak observed 84% used/);
    expect(label).toMatch(/band 75–100%/);
    expect(heatmapCellAriaLabel(day({ peakExactness: "lowerBound" }))).toContain(
      "(lower bound)",
    );
    const unobserved = heatmapCellAriaLabel(
      day({ observed: false, band: undefined, peakUsedPercent: undefined }),
    );
    expect(unobserved).toMatch(/not observed$/);
    expect(unobserved).not.toMatch(/peak observed/);
  });

  it("parses day keys on the local calendar", () => {
    const parsed = formatDayCell("2026-09-28");
    expect(parsed.day).toBe("28");
    expect(formatDayKey("2026-09-28")).toContain("28");
  });
});

describe("trend segmentation — resets, gaps, single points", () => {
  it("connects same-cycle points within the gap threshold", () => {
    const shape = trendShape(
      makeSeries([
        point("2026-09-28T01:00:00Z", 10, "c1", { cycleStart: true }),
        point("2026-09-28T02:00:00Z", 20),
        point("2026-09-28T03:00:00Z", 30),
      ]),
    );
    expect(shape.segments).toHaveLength(1);
    expect(shape.resets).toHaveLength(0);
    expect(shape.gaps).toHaveLength(0);
  });

  it("breaks the line at a reset boundary and never bridges it", () => {
    const shape = trendShape(
      makeSeries([
        point("2026-09-28T01:00:00Z", 88, "c1", { cycleStart: true }),
        point("2026-09-28T02:00:00Z", 93, "c1"),
        point("2026-09-28T03:00:00Z", 4, "c2", { cycleStart: true, resetBoundary: true }),
        point("2026-09-28T04:00:00Z", 11, "c2"),
      ]),
    );
    expect(shape.segments).toHaveLength(2);
    expect(shape.segments[1].breakBefore).toBe("reset");
    expect(shape.resets).toHaveLength(1);
    expect(shape.resets[0].usedPercent).toBe(4);
  });

  it("breaks the line at over-threshold gaps on one cycle", () => {
    const shape = trendShape(
      makeSeries(
        [
          point("2026-09-28T01:00:00Z", 10, "c1", { cycleStart: true }),
          point("2026-09-28T01:30:00Z", 20, "c1"),
          // 4.5h later, same cycle: beyond the gap threshold.
          point("2026-09-28T06:00:00Z", 40, "c1"),
        ],
        45 * 60 * 1000,
      ),
    );
    expect(shape.segments).toHaveLength(2);
    expect(shape.segments[1].breakBefore).toBe("gap");
  });

  it("draws a single observation as a dot, never a joined line", () => {
    const shape = trendShape(makeSeries([point("2026-09-28T01:00:00Z", 42)]));
    expect(shape.segments).toHaveLength(1);
    expect(shape.singlePoints).toHaveLength(1);
  });

  it("sorts defensively by observation time", () => {
    const shape = trendShape(
      makeSeries([
        point("2026-09-28T03:00:00Z", 30),
        point("2026-09-28T01:00:00Z", 10),
        point("2026-09-28T02:00:00Z", 20),
      ]),
    );
    expect(shape.segments).toHaveLength(1);
    expect(shape.segments[0].points.map((p) => p.usedPercent)).toEqual([10, 20, 30]);
  });
});

describe("trend geometry — fixed 0–100 scale over the full range", () => {
  const WIDTH = 300;
  const HEIGHT = 76;

  it("never normalizes the y scale to the observed range", () => {
    const series = makeSeries(
      [
        point("2026-09-28T01:00:00Z", 80, "c1", { cycleStart: true }),
        point("2026-09-28T12:00:00Z", 90, "c1"),
      ],
      25 * HOUR_MS,
    );
    const geometry = trendGeometry(series, RANGE_START, RANGE_END, WIDTH, HEIGHT);
    // 90% used sits 10% from the top of a fixed 0–100 chart at mid-range x,
    // even though 90 is the observed maximum.
    const onlyPath = geometry.paths[0];
    expect(onlyPath).toContain(`L150.00 ${(HEIGHT * 0.1).toFixed(2)}`);
  });

  it("uses the full query range as the x-domain", () => {
    const series = makeSeries(
      [
        point(new Date(RANGE_START).toISOString(), 10, "c1", { cycleStart: true }),
        point(new Date(RANGE_END).toISOString(), 20, "c1"),
      ],
      25 * HOUR_MS,
    );
    const geometry = trendGeometry(series, RANGE_START, RANGE_END, WIDTH, HEIGHT);
    expect(geometry.paths[0]).toContain(`M0.00 ${(HEIGHT * 0.9).toFixed(2)}`);
    expect(geometry.paths[0]).toContain(`L${WIDTH}.00 ${(HEIGHT * 0.8).toFixed(2)}`);
  });

  it("emits one path per segment so resets and gaps are visibly broken", () => {
    const series = makeSeries([
      point("2026-09-28T01:00:00Z", 80, "c1", { cycleStart: true }),
      point("2026-09-28T02:00:00Z", 90, "c1"),
      point("2026-09-28T03:00:00Z", 5, "c2", { cycleStart: true, resetBoundary: true }),
      point("2026-09-28T04:00:00Z", 9, "c2"),
    ]);
    const geometry = trendGeometry(series, RANGE_START, RANGE_END, WIDTH, HEIGHT);
    expect(geometry.paths).toHaveLength(2);
    expect(geometry.resetMarkers).toHaveLength(1);
  });

  it("maps not-observed gaps to clamped hatched floor spans", () => {
    const series = makeSeries([
      point("2026-09-28T01:00:00Z", 10, "c1", { cycleStart: true }),
      point("2026-09-28T02:00:00Z", 20),
      point("2026-09-28T20:00:00Z", 40),
    ]);
    const geometry = trendGeometry(series, RANGE_START, RANGE_END, WIDTH, HEIGHT);
    expect(geometry.gapSpans).toHaveLength(1);
    expect(geometry.gapSpans[0].width).toBeGreaterThan(0);
  });

  it("places the 80% near-limit guide at 20% from the top", () => {
    const series = makeSeries([point("2026-09-28T01:00:00Z", 50)]);
    const geometry = trendGeometry(series, RANGE_START, RANGE_END, WIDTH, HEIGHT);
    expect(geometry.guideY).toBeCloseTo(HEIGHT * 0.2, 5);
  });
});

describe("textual chart summaries", () => {
  it("summarizes peak, resets, and not-observed spans", () => {
    const series = makeSeries([
      point("2026-09-28T01:00:00Z", 70, "c1", { cycleStart: true }),
      point("2026-09-28T02:00:00Z", 84),
      point("2026-09-28T03:00:00Z", 4, "c2", { cycleStart: true, resetBoundary: true }),
    ]);
    expect(trendSummaryText(series)).toBe("peak 84% used · 1 reset");
  });

  it("names unobserved spans explicitly", () => {
    const series = makeSeries([
      point("2026-09-28T01:00:00Z", 10, "c1", { cycleStart: true }),
      point("2026-09-28T02:00:00Z", 20),
      point("2026-09-28T20:00:00Z", 40),
    ]);
    const withGaps = {
      ...series,
      coverage: {
        ...series.coverage,
        gaps: [
          {
            from: "2026-09-28T02:00:00Z",
            to: "2026-09-28T20:00:00Z",
            durationMs: 18 * HOUR_MS,
            kind: "notObserved" as const,
          },
        ],
      },
    };
    expect(trendSummaryText(withGaps)).toBe(
      "peak 40% used · 1 span not observed",
    );
  });

  it("says so when a series has no observations", () => {
    expect(trendSummaryText(makeSeries([]))).toBe("no observations");
  });
});

describe("aggregate view helpers", () => {
  it("detects a completely empty history", () => {
    const empty: UsageAnalytics = {
      schemaVersion: 1,
      range: "24h",
      rangeStart: new Date(RANGE_START).toISOString(),
      rangeEnd: new Date(RANGE_END).toISOString(),
      generatedAt: new Date(RANGE_END).toISOString(),
      timezoneOffsetMinutes: 0,
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
        { date: "2026-09-29", observed: false },
      ],
      trends: [],
      gapSemantics: "notObservedNeverZeroFilled",
      availabilityInference: "none",
    };
    expect(hasAnyObservation(empty)).toBe(false);
  });

  it("bounds the small-multiples grid deterministically", () => {
    const trends = Array.from({ length: 8 }, (_, index) =>
      makeSeries([point("2026-09-28T01:00:00Z", index)]),
    );
    const { visible, hiddenCount } = visibleTrendSeries(trends);
    expect(visible).toHaveLength(6);
    expect(hiddenCount).toBe(2);
  });
});
