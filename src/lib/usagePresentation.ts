import type {
  NotObservedGap,
  ObservedPeak,
  UsageAnalytics,
  UsageHeatmapDay,
  UsageResultExactness,
  UsageSummary,
  UsageTrendPoint,
  UsageTrendSeries,
} from "./usageAnalytics";
import { toneFor, type UsageTone } from "./dashboard";
import type { QuotaPerspective } from "./quotaPresentation";
import {
  CRITICAL_PERCENT,
  NEAR_LIMIT_PERCENT,
  type ThresholdPercent,
} from "./thresholds";

/**
 * Presentation-only derivations for the v0.7 Usage view. The backend analytics
 * contract (docs/usage-analytics-backend-v0.7.md) is the source of truth for
 * every number: nothing here recomputes peaks, resets, bands, or threshold
 * durations — these helpers only shape backend values into text, tones, and
 * chart geometry, and keep the exact/lower-bound and estimated semantics
 * impossible to drop on the floor.
 *
 * v0.7 product integration: the v0.6 quota-perspective preference is part of
 * the product now. It reaches this module as a `QuotaPerspective` argument and
 * shapes LABELS only — severity words, near-limit thresholds, heatmap bands,
 * and trend geometry stay canonical on the used percentage (the inversion
 * boundary for values remains `quotaPresentation.ts`; this module never
 * rewrites an analytics number).
 */

export type Exactness = UsageResultExactness;

/** "lower bound" must exist as text whenever a value is not exact. */
export function exactnessCaption(exactness: Exactness): string | undefined {
  return exactness === "lowerBound" ? "lower bound" : undefined;
}

/**
 * Perspective-aware exactness caption. A lower bound on used implies an upper
 * bound on the remaining complement (used ≥ 84 ⟺ remaining ≤ 16), so the
 * qualifier survives the perspective instead of silently becoming wrong.
 */
export function exactnessCaptionFor(
  exactness: Exactness,
  perspective: QuotaPerspective,
): string | undefined {
  if (exactness !== "lowerBound") return undefined;
  return perspective === "used" ? "lower bound" : "upper bound";
}

/** Whole-percent display for a used percent, with "+" when it is a lower bound. */
export function peakDisplayValue(peak: ObservedPeak): string {
  const percent = Math.round(peak.usedPercent);
  return peak.exactness === "lowerBound" ? `${percent}%+` : `${percent}%`;
}

/**
 * Remaining capacity is always derived (100 − used); the canonical analytics
 * value is never mutated.
 */
export function remainingPercent(usedPercent: number): number {
  return Math.round(100 - usedPercent);
}

/**
 * Perspective-shaped headline for a peak observation. Used mode shows the
 * canonical peak ("84%", "84%+"); Remaining mode shows the derived complement
 * as a remaining share ("16% remaining") — the peak of usage is the low point
 * of remaining, which the wording says out loud.
 */
export function peakPresentation(
  peak: ObservedPeak,
  perspective: QuotaPerspective,
): { value: string; badge: string | undefined } {
  if (perspective === "used") {
    return {
      value: peakDisplayValue(peak),
      badge: exactnessCaption(peak.exactness),
    };
  }
  const remaining = remainingPercent(peak.usedPercent);
  return {
    value: `${remaining}% remaining`,
    badge: exactnessCaptionFor(peak.exactness, perspective),
  };
}

/**
 * Perspective-shaped value for the most constrained window. The value text
 * follows the perspective ("84% used" / "16% remaining"); severity is the
 * caller's and stays canonical on used.
 */
export function constrainedValuePresentation(
  usedPercent: number,
  perspective: QuotaPerspective,
): string {
  return perspective === "used"
    ? `${Math.round(usedPercent)}% used`
    : `${remainingPercent(usedPercent)}% remaining`;
}

/** The canonical counterpart shown as a sub-line, whichever mode is active. */
export function counterpartPercentText(
  usedPercent: number,
  perspective: QuotaPerspective,
): string {
  return perspective === "used"
    ? `${remainingPercent(usedPercent)}% left`
    : `${Math.round(usedPercent)}% used`;
}

/**
 * Severity stays canonical on the USED percentage — 95% used reads Critical
 * even when 5% remains. Words keep severity from ever being color-only.
 */
export type SeverityWord = "High" | "Critical";

export function severityWord(usedPercent: number): SeverityWord | undefined {
  if (usedPercent >= CRITICAL_PERCENT) return "Critical";
  if (usedPercent >= NEAR_LIMIT_PERCENT) return "High";
  return undefined;
}

export function severityTone(usedPercent: number): UsageTone {
  return toneFor(Math.round(usedPercent));
}

/** "1h 24m", "18m", "3d 2h", "<1m", "0m" — honest compact durations. */
export function formatEstimateDuration(ms: number): string {
  if (ms <= 0) return "0m";
  const totalMinutes = Math.floor(ms / 60000);
  if (totalMinutes < 1) return "<1m";
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

export type NearLimitEstimate = {
  thresholdPercent: ThresholdPercent;
  text: string;
};

export type NearLimitPresentation = {
  /** Always true when the metric exists — the backend marks it estimated. */
  estimated: true;
  /** The ≥80% headline, e.g. "1h 24m". */
  primary: NearLimitEstimate;
  /** The ≥95% secondary line when the backend returned it. */
  secondary?: NearLimitEstimate;
};

/** Shapes the backend's estimated threshold durations; never recomputes them. */
export function nearLimitPresentation(
  timeNearLimit: UsageSummary["timeNearLimit"],
): NearLimitPresentation | undefined {
  if (!timeNearLimit) return undefined;
  const byThreshold = new Map(
    timeNearLimit.estimates.map((estimate) => [estimate.thresholdPercent, estimate]),
  );
  const primary = byThreshold.get(NEAR_LIMIT_PERCENT);
  if (!primary) return undefined;
  const secondary = byThreshold.get(CRITICAL_PERCENT);
  return {
    estimated: true,
    primary: {
      thresholdPercent: NEAR_LIMIT_PERCENT,
      text: formatEstimateDuration(primary.estimatedDurationMs),
    },
    secondary: secondary
      ? {
          thresholdPercent: CRITICAL_PERCENT,
          text: formatEstimateDuration(secondary.estimatedDurationMs),
        }
      : undefined,
  };
}

/**
 * Optional equivalence note for the near-limit metric. The thresholds stay
 * canonical on USED (≥80% / ≥95%) in both perspectives; in Remaining mode the
 * note spells out the complement so the numbers stay verifiable, without any
 * code re-deriving thresholds on remaining values.
 */
export function nearLimitEquivalenceNote(
  perspective: QuotaPerspective,
): string | undefined {
  return perspective === "used"
    ? undefined
    : "thresholds measure used: ≤20% remaining ≈ ≥80% used";
}

/**
 * Fixed absolute heatmap bands from the backend contract. Labels state the
 * physical percent range; a day is colored by its backend `band`, never by a
 * share of the user's own maximum.
 */
export const HEATMAP_BANDS: ReadonlyArray<{
  band: 1 | 2 | 3 | 4;
  label: string;
}> = [
  { band: 1, label: "0–25%" },
  { band: 2, label: "25–50%" },
  { band: 3, label: "50–75%" },
  { band: 4, label: "75–100%" },
];

export function heatmapBandLabel(day: UsageHeatmapDay): string {
  if (!day.observed || day.band === undefined) return "Not observed";
  return HEATMAP_BANDS.find((entry) => entry.band === day.band)?.label ?? "—";
}

/** Local-calendar parse for a backend "YYYY-MM-DD" day key. */
export function parseDayKey(date: string): Date {
  const [year, month, day] = date.split("-").map(Number);
  return new Date(year, (month ?? 1) - 1, day ?? 1);
}

const dayKeyFormat = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  month: "short",
  day: "numeric",
});

const observedAtFormat = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});

/** "Sep 28, 2:10 PM" — observation timestamp for metric sub-lines. */
export function formatObservedAt(iso: string): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return "—";
  return observedAtFormat.format(parsed);
}

const tickTimeFormat = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
});

const tickDayFormat = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
});

/** Edge time label: clock time on 24h, calendar day on 7d. */
export function formatTrendTick(
  iso: string,
  range: UsageAnalytics["range"],
): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return "—";
  return range === "24h"
    ? tickTimeFormat.format(parsed)
    : tickDayFormat.format(parsed);
}

export function formatDayKey(date: string): string {
  const parsed = parseDayKey(date);
  if (Number.isNaN(parsed.getTime())) return date;
  return dayKeyFormat.format(parsed);
}

/** Compact cell headline: weekday initial + day-of-month. */
export function formatDayCell(date: string): { weekday: string; day: string } {
  const parsed = parseDayKey(date);
  if (Number.isNaN(parsed.getTime())) return { weekday: "—", day: "—" };
  const weekday = parsed.toLocaleDateString(undefined, { weekday: "narrow" });
  return { weekday, day: String(parsed.getDate()) };
}

export function heatmapCellAriaLabel(
  day: UsageHeatmapDay,
  perspective: QuotaPerspective = "used",
): string {
  const when = formatDayKey(day.date);
  if (!day.observed || day.peakUsedPercent === undefined) {
    return `${when}: not observed`;
  }
  const exactness =
    day.peakExactness === "lowerBound"
      ? perspective === "used"
        ? " (lower bound)"
        : " (upper bound)"
      : "";
  if (perspective === "remaining") {
    // The band stays the canonical USED band; the label states both sides of
    // the equivalence so the color never reads as a remaining-scale band.
    return `${when}: peak observed ${remainingPercent(day.peakUsedPercent)}% remaining (${Math.round(day.peakUsedPercent)}% used), band ${heatmapBandLabel(day)}${exactness}`;
  }
  return `${when}: peak observed ${Math.round(day.peakUsedPercent)}% used, band ${heatmapBandLabel(day)}${exactness}`;
}

/** Cell value text; Remaining mode labels the complement, band untouched. */
export function heatmapCellValueText(
  day: UsageHeatmapDay,
  perspective: QuotaPerspective = "used",
): string {
  if (!day.observed || day.peakUsedPercent === undefined) return "–";
  return perspective === "used"
    ? `${Math.round(day.peakUsedPercent)}%`
    : `${remainingPercent(day.peakUsedPercent)}%`;
}

/** True when the response carries at least one real observation anywhere. */
export function hasAnyObservation(analytics: UsageAnalytics): boolean {
  return (
    analytics.summary.peakObservedUsage !== undefined ||
    analytics.heatmap.some((day) => day.observed) ||
    analytics.trends.some((series) => series.points.length > 0)
  );
}

/**
 * Trend segmentation: consecutive points connect only while they share a
 * reset cycle AND the interval stays at or below the series' gap threshold.
 * A cycle change breaks at the reset boundary; an over-threshold interval on
 * one cycle breaks at a not-observed gap. Nothing is ever drawn across a
 * break, and gaps never become zero-filled segments.
 */
export type TrendSegment = {
  points: UsageTrendPoint[];
  /** Why the line broke before this segment; undefined for the first one. */
  breakBefore?: "reset" | "gap";
};

export type TrendShape = {
  segments: TrendSegment[];
  /** resetBoundary points in this series — the observed reset markers. */
  resets: UsageTrendPoint[];
  /** Single unconnected observations (drawn as dots, never joined). */
  singlePoints: UsageTrendPoint[];
  /** Internal not-observed spans reported by the backend coverage. */
  gaps: NotObservedGap[];
};

export function trendShape(series: UsageTrendSeries): TrendShape {
  const points = [...series.points].sort(
    (a, b) => Date.parse(a.observedAt) - Date.parse(b.observedAt),
  );
  const segments: TrendSegment[] = [];
  let current: UsageTrendPoint[] = [];
  let pendingBreak: TrendSegment["breakBefore"];
  const flush = () => {
    if (current.length > 0) {
      segments.push({ points: current, breakBefore: pendingBreak });
    }
    current = [];
    pendingBreak = undefined;
  };
  for (let index = 0; index < points.length; index += 1) {
    const point = points[index];
    if (index > 0) {
      const previous = points[index - 1];
      const interval = Date.parse(point.observedAt) - Date.parse(previous.observedAt);
      const crossed =
        previous.cycleId !== point.cycleId
          ? ("reset" as const)
          : interval > series.coverage.gapThresholdMs
            ? ("gap" as const)
            : undefined;
      if (crossed) {
        flush();
        pendingBreak = crossed;
      }
    }
    current.push(point);
  }
  flush();

  return {
    segments,
    resets: points.filter((point) => point.resetBoundary),
    singlePoints: segments.filter((segment) => segment.points.length === 1).map((s) => s.points[0]),
    gaps: series.coverage.gaps.filter((gap) => gap.kind === "notObserved"),
  };
}

/** Visible one-line summary under each chart (and its spoken summary). */
export function trendSummaryText(
  series: UsageTrendSeries,
  perspective: QuotaPerspective = "used",
): string {
  const shape = trendShape(series);
  const parts: string[] = [];
  const peak = series.points.reduce<number | undefined>(
    (best, point) =>
      best === undefined || point.usedPercent > best ? point.usedPercent : best,
    undefined,
  );
  if (peak !== undefined) {
    // The plotted line stays canonical (percent used); the headline follows
    // the perspective — peak usage is the lowest remaining point.
    parts.push(
      perspective === "used"
        ? `peak ${Math.round(peak)}% used`
        : `lowest remaining ${remainingPercent(peak)}%`,
    );
  }
  if (shape.resets.length > 0) {
    parts.push(
      shape.resets.length === 1 ? "1 reset" : `${shape.resets.length} resets`,
    );
  }
  if (shape.gaps.length > 0) {
    parts.push(
      shape.gaps.length === 1
        ? "1 span not observed"
        : `${shape.gaps.length} spans not observed`,
    );
  }
  if (parts.length === 0) return "no observations";
  return parts.join(" · ");
}

/** "Codex · Weekly credits" — rail-style short name with the window label. */
export function seriesTitle(
  series: Pick<UsageTrendSeries, "providerId" | "account" | "windowLabel">,
  shortProviderId: (id: string) => string,
): string {
  const account = series.account ? ` (${series.account})` : "";
  return `${shortProviderId(series.providerId)}${account} · ${series.windowLabel}`;
}

export type TrendGeometryPoint = { x: number; y: number };

export type TrendGeometry = {
  /** One polyline `d` per connected segment; never joined across breaks. */
  paths: string[];
  /** Hollow diamond markers on reset-boundary points. */
  resetMarkers: TrendGeometryPoint[];
  /** Dots for single unconnected observations. */
  singleDots: TrendGeometryPoint[];
  /** Faint hatched spans at the chart floor for not-observed gaps. */
  gapSpans: Array<{ x: number; width: number }>;
  /** x positions of the first and last observation, for edge time labels. */
  edgeTicks: TrendGeometryPoint[];
  /** y of the 80% near-limit guide line. */
  guideY: number;
};

const PERCENT_MAX = 100;

/**
 * Maps a series into a fixed 0–100 percent viewBox. The x-domain is the
 * query's full range so every small multiple shares one time axis; the
 * y-domain is always 0–100 so a half-full chart always means 50% used.
 */
export function trendGeometry(
  series: UsageTrendSeries,
  rangeStartMs: number,
  rangeEndMs: number,
  width: number,
  height: number,
): TrendGeometry {
  const span = Math.max(1, rangeEndMs - rangeStartMs);
  const x = (ms: number) => ((ms - rangeStartMs) / span) * width;
  const y = (percent: number) =>
    height - (Math.min(PERCENT_MAX, Math.max(0, percent)) / PERCENT_MAX) * height;
  const shape = trendShape(series);

  const paths = shape.segments
    .filter((segment) => segment.points.length > 1)
    .map((segment) =>
      segment.points
        .map((point, index) => {
          const px = x(Date.parse(point.observedAt)).toFixed(2);
          const py = y(point.usedPercent).toFixed(2);
          return `${index === 0 ? "M" : "L"}${px} ${py}`;
        })
        .join(" "),
    );

  const singleDots = shape.singlePoints.map((point) => ({
    x: x(Date.parse(point.observedAt)),
    y: y(point.usedPercent),
  }));

  const resetMarkers = shape.resets.map((point) => ({
    x: x(Date.parse(point.observedAt)),
    y: y(point.usedPercent),
  }));

  const gapSpans = shape.gaps.map((gap) => {
    const from = Math.max(rangeStartMs, Date.parse(gap.from));
    const to = Math.min(rangeEndMs, Date.parse(gap.to));
    return { x: x(from), width: Math.max(0, x(to) - x(from)) };
  });

  return {
    paths,
    resetMarkers,
    singleDots,
    gapSpans,
    edgeTicks:
      shape.segments.length > 0
        ? [
            { x: x(Date.parse(series.points[0].observedAt)), y: 0 },
            {
              x: x(
                Date.parse(series.points[series.points.length - 1].observedAt),
              ),
              y: 0,
            },
          ]
        : [],
    guideY: y(NEAR_LIMIT_PERCENT),
  };
}

/**
 * Deterministic display bound for the "All providers" small-multiples grid:
 * backend series order (sorted identity), no ranking, with an honest count of
 * anything left out so the grid never silently hides windows.
 */
export const TREND_SMALL_MULTIPLES_MAX = 6;

export function visibleTrendSeries(trends: UsageTrendSeries[]): {
  visible: UsageTrendSeries[];
  hiddenCount: number;
} {
  return {
    visible: trends.slice(0, TREND_SMALL_MULTIPLES_MAX),
    hiddenCount: Math.max(0, trends.length - TREND_SMALL_MULTIPLES_MAX),
  };
}
