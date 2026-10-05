import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./floatingWindowChrome";

export type UsageAnalyticsRange = "24h" | "7d";
export type UsageResultExactness = "exact" | "lowerBound";
export type UsagePointResolution = "detailed" | "compacted";

export type UsageAnalyticsQuery = {
  range: UsageAnalyticsRange;
  providerId?: string;
  account?: string | null;
  windowLabel?: string;
  exactAccount?: boolean;
};

export type ObservedPeak = {
  usedPercent: number;
  providerId: string;
  account?: string;
  windowLabel: string;
  observedAt: string;
  exactness: UsageResultExactness;
};

export type UsageSummary = {
  peakObservedUsage?: ObservedPeak;
  mostConstrainedWindow?: ObservedPeak;
  observedResetCycles: {
    count: number;
    exactness: UsageResultExactness;
    detection: "historyResetBoundaryTransitions";
  };
  observedDays: {
    count: number;
    timezoneOffsetMinutes: number;
  };
  timeNearLimit?: {
    estimated: true;
    method: "piecewiseLinearBetweenAdjacentSameCycleObservations";
    intervalPolicy: "onlyIntervalsAtOrBelowPerSeriesGapThreshold";
    comparableSpanMs: number;
    comparableSpanRatio: number;
    estimates: Array<{
      thresholdPercent: 80 | 95;
      estimatedDurationMs: number;
      estimatedShare: number;
    }>;
  };
};

export type UsageHeatmapDay = {
  date: string;
  observed: boolean;
  peakUsedPercent?: number;
  peakObservedAt?: string;
  providerId?: string;
  account?: string;
  windowLabel?: string;
  /** Absolute bands at 0/25/50/75/100; absent means not observed. */
  band?: 1 | 2 | 3 | 4;
  peakExactness?: UsageResultExactness;
};

export type UsageTrendPoint = {
  observedAt: string;
  usedPercent: number;
  resetAt?: string;
  cycleId: string;
  cycleStart: boolean;
  resetBoundary: boolean;
  resolution: UsagePointResolution;
};

export type NotObservedGap = {
  from: string;
  to: string;
  durationMs: number;
  kind: "notObserved";
};

export type UsageTrendSeries = {
  providerId: string;
  account?: string;
  windowLabel: string;
  points: UsageTrendPoint[];
  coverage: {
    firstObservedAt?: string;
    lastObservedAt?: string;
    gapThresholdMs: number;
    comparableSpanMs: number;
    comparableSpanRatio: number;
    gaps: NotObservedGap[];
  };
};

export type UsageAnalytics = {
  schemaVersion: 1;
  range: UsageAnalyticsRange;
  rangeStart: string;
  rangeEnd: string;
  generatedAt: string;
  timezoneOffsetMinutes: number;
  summary: UsageSummary;
  heatmap: UsageHeatmapDay[];
  trends: UsageTrendSeries[];
  gapSemantics: "notObservedNeverZeroFilled";
  availabilityInference: "none";
};

/** Reads the deterministic analytics projection from the Rust history store. */
export async function loadUsageAnalytics(
  query: UsageAnalyticsQuery,
): Promise<UsageAnalytics> {
  if (!isTauriRuntime()) {
    throw new Error("Usage analytics requires the Tauri runtime");
  }
  return invoke<UsageAnalytics>("get_usage_analytics", { query });
}

/** The one analytics source; injectable in tests and dev fixtures. */
export type UsageAnalyticsLoader = typeof loadUsageAnalytics;
