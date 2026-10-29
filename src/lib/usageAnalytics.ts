import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./floatingWindowChrome";
import type { CodexIdentityState } from "../types";

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
  /**
   * The Codex identity scope this projection was computed under. The
   * consumer compares it with the scope it currently observes and discards
   * a response that belongs to an older session.
   */
  codexIdentityScope?: CodexScope;
};

/** The observed identity state of the configured Codex slots. */
export type CodexScope = ReadonlyArray<CodexIdentityState>;

/**
 * True when the scope a projection was computed under is at least as
 * current as the scope the consumer observes.
 *
 * The comparison is per slot and monotonic: a response computed after the
 * consumer's last observation is accepted, a response computed before it
 * is not. One slot's older scope never suppresses another slot.
 */
export function codexScopeIsCurrentOrNewer(response: CodexScope | undefined, expected: CodexScope): boolean {
  if (!response) {
    // A response without the scope cannot be fenced against the current
    // session, so it is only accepted when no Codex slot is expected.
    return expected.length === 0;
  }
  const observed = new Map(response.map((state) => [state.providerId, state]));
  return expected.every((state) => {
    const seen = observed.get(state.providerId);
    // The response says nothing about this slot, so it cannot be proven to
    // belong to the expected session.
    if (!seen) return false;
    // A newer generation means a newer observed session: the response
    // describes the current one and is accepted.
    if (seen.generation > state.generation) return true;
    // The same generation must carry the same identity.
    if (seen.generation < state.generation) return false;
    return seen.identity === state.identity;
  });
}

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
