import type { ProviderUsage } from "../types";
import {
  QUOTA_PERSPECTIVES,
  THEMES,
  SETTINGS_STORAGE_KEY,
  loadSettings,
  type QuotaPerspective,
  type Theme,
} from "../lib/settings";
import type {
  UsageAnalytics,
  UsageAnalyticsQuery,
  UsageHeatmapDay,
  UsageSummary,
  UsageTrendPoint,
  UsageTrendSeries,
} from "../lib/usageAnalytics";
import type {
  UsageIntelligence,
  UsageIntelligenceQuery,
  UsageIntelligenceRange,
  UsageIntelligenceLoader,
  UsageModelBreakdown,
  UsageProviderBreakdown,
  UsageSourceDiagnostics,
  UsageTokenSums,
} from "../lib/usageIntelligence";

/**
 * DEV-ONLY visual-acceptance fixtures for the Usage view (`npm run dev` +
 * `?fixture=…`). Production builds drop this module: the only call sites are
 * guarded by `import.meta.env.DEV`, so the bundler never ships it.
 *
 * Fixtures hand-author backend-shaped `UsageAnalytics` payloads for the
 * accepted visual states (normal 24h, 7d lower-bound, reset boundary, gaps,
 * empty) — they are presentation inputs, not a second analytics engine, and
 * the product code path is identical to production (same components, same
 * loader contract).
 */

const MINUTE = 60 * 1000;
const HOUR = 60 * MINUTE;

export type DevFixture = {
  name: string;
  usages: ProviderUsage[];
  loader: (query: UsageAnalyticsQuery) => Promise<UsageAnalytics>;
  /** v0.9 Usage-first overview: force the opt-in state for a screenshot. */
  usageIntelligenceEnabled?: boolean;
  /** v0.9 Usage-first overview: hand-authored intelligence payloads. */
  intelligenceLoader?: UsageIntelligenceLoader;
};

function iso(ms: number): string {
  return new Date(ms).toISOString();
}

/** Local-calendar "YYYY-MM-DD" for a timestamp. */
function dayKey(ms: number): string {
  const date = new Date(ms);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

function point(
  atMs: number,
  usedPercent: number,
  cycleId: string,
  options: Partial<Pick<UsageTrendPoint, "resetBoundary" | "cycleStart" | "resolution">> = {},
): UsageTrendPoint {
  return {
    observedAt: iso(atMs),
    usedPercent,
    cycleId,
    cycleStart: options.cycleStart ?? false,
    resetBoundary: options.resetBoundary ?? false,
    resolution: options.resolution ?? "detailed",
  };
}

/** Peak per local day over the given series — fixture-side generation only. */
function heatmapFrom(
  seriesList: UsageTrendSeries[],
  rangeStartMs: number,
  rangeEndMs: number,
  exactness: "exact" | "lowerBound",
): UsageHeatmapDay[] {
  const days: UsageHeatmapDay[] = [];
  const cursor = new Date(rangeStartMs);
  cursor.setHours(0, 0, 0, 0);
  const end = new Date(rangeEndMs);
  end.setHours(0, 0, 0, 0);
  for (let day = cursor; day <= end; day.setDate(day.getDate() + 1)) {
    const key = dayKey(day.getTime());
    const nextDayStart = day.getTime() + 24 * HOUR;
    let best: { used: number; at: string; series: UsageTrendSeries } | undefined;
    for (const series of seriesList) {
      for (const candidate of series.points) {
        const at = Date.parse(candidate.observedAt);
        if (at < day.getTime() || at >= nextDayStart) continue;
        if (!best || candidate.usedPercent > best.used) {
          best = { used: candidate.usedPercent, at: candidate.observedAt, series };
        }
      }
    }
    days.push(
      best
        ? {
            date: key,
            observed: true,
            peakUsedPercent: best.used,
            peakObservedAt: best.at,
            providerId: best.series.providerId,
            account: best.series.account,
            windowLabel: best.series.windowLabel,
            band: best.used <= 25 ? 1 : best.used <= 50 ? 2 : best.used <= 75 ? 3 : 4,
            peakExactness: exactness,
          }
        : { date: key, observed: false },
    );
  }
  return days;
}

/** Adjacent-pair spans over the per-series gap threshold. */
function gapsFor(
  points: UsageTrendPoint[],
  gapThresholdMs: number,
): Array<{ from: string; to: string; durationMs: number; kind: "notObserved" }> {
  const gaps = [];
  for (let index = 1; index < points.length; index += 1) {
    const from = Date.parse(points[index - 1].observedAt);
    const to = Date.parse(points[index].observedAt);
    if (
      points[index - 1].cycleId === points[index].cycleId &&
      to - from > gapThresholdMs
    ) {
      gaps.push({ from: iso(from), to: iso(to), durationMs: to - from, kind: "notObserved" as const });
    }
  }
  return gaps;
}

function series(
  providerId: string,
  windowLabel: string,
  account: string | undefined,
  points: UsageTrendPoint[],
  rangeStartMs: number,
  rangeEndMs: number,
  gapThresholdMs = 45 * MINUTE,
): UsageTrendSeries {
  const comparable = points.reduce((total, point, index) => {
    if (index === 0) return total;
    const previous = points[index - 1];
    const duration = Date.parse(point.observedAt) - Date.parse(previous.observedAt);
    return previous.cycleId === point.cycleId && duration > 0 && duration <= gapThresholdMs
      ? total + duration
      : total;
  }, 0);
  return {
    providerId,
    ...(account ? { account } : {}),
    windowLabel,
    points,
    coverage: {
      firstObservedAt: points[0]?.observedAt,
      lastObservedAt: points[points.length - 1]?.observedAt,
      gapThresholdMs,
      comparableSpanMs: comparable,
      comparableSpanRatio: comparable / Math.max(1, rangeEndMs - rangeStartMs),
      gaps: gapsFor(points, gapThresholdMs),
    },
  };
}

/** Piecewise-linear ≥T time between adjacent same-cycle points (fixture-side). */
function nearLimitEstimate(
  seriesList: UsageTrendSeries[],
  thresholds: ReadonlyArray<80 | 95>,
): UsageSummary["timeNearLimit"] {
  let comparableSpanMs = 0;
  const totals = new Map<80 | 95, number>(thresholds.map((t) => [t, 0]));
  for (const list of seriesList) {
    for (let index = 1; index < list.points.length; index += 1) {
      const from = list.points[index - 1];
      const to = list.points[index];
      const duration = Date.parse(to.observedAt) - Date.parse(from.observedAt);
      if (
        from.cycleId !== to.cycleId ||
        duration <= 0 ||
        duration > list.coverage.gapThresholdMs
      ) {
        continue;
      }
      comparableSpanMs += duration;
      for (const threshold of thresholds) {
        const above = (value: number) => (value >= threshold ? 1 : 0);
        const both = above(from.usedPercent) + above(to.usedPercent);
        let durationAbove: number;
        if (both === 2) durationAbove = duration;
        else if (both === 0) durationAbove = 0;
        else {
          const other = from.usedPercent >= threshold ? to : from;
          const crossing = from.usedPercent >= threshold ? from : to;
          const fraction =
            Math.abs(crossing.usedPercent - threshold) /
            Math.max(0.001, Math.abs(crossing.usedPercent - other.usedPercent));
          durationAbove = duration * Math.min(1, Math.max(0, fraction));
        }
        totals.set(threshold, (totals.get(threshold) ?? 0) + durationAbove);
      }
    }
  }
  if (comparableSpanMs === 0) return undefined;
  return {
    estimated: true,
    method: "piecewiseLinearBetweenAdjacentSameCycleObservations",
    intervalPolicy: "onlyIntervalsAtOrBelowPerSeriesGapThreshold",
    comparableSpanMs,
    comparableSpanRatio: 1,
    estimates: thresholds.map((threshold) => {
      const estimatedDurationMs = Math.round(totals.get(threshold) ?? 0);
      return {
        thresholdPercent: threshold,
        estimatedDurationMs,
        estimatedShare: estimatedDurationMs / comparableSpanMs,
      };
    }),
  };
}

function summary(
  seriesList: UsageTrendSeries[],
  heatmap: UsageHeatmapDay[],
  exactness: "exact" | "lowerBound",
): UsageSummary {
  let peak: { used: number; at: string; series: UsageTrendSeries } | undefined;
  let resets = 0;
  for (const list of seriesList) {
    for (const candidate of list.points) {
      if (
        !peak ||
        candidate.usedPercent > peak.used ||
        (candidate.usedPercent === peak.used &&
          Date.parse(candidate.observedAt) < Date.parse(peak.at))
      ) {
        peak = { used: candidate.usedPercent, at: candidate.observedAt, series: list };
      }
      if (candidate.resetBoundary) resets += 1;
    }
  }
  const observedPeak = peak
    ? {
        usedPercent: peak.used,
        providerId: peak.series.providerId,
        ...(peak.series.account ? { account: peak.series.account } : {}),
        windowLabel: peak.series.windowLabel,
        observedAt: peak.at,
        exactness,
      }
    : undefined;
  return {
    peakObservedUsage: observedPeak,
    mostConstrainedWindow: observedPeak,
    observedResetCycles: {
      count: resets,
      exactness: "lowerBound",
      detection: "historyResetBoundaryTransitions",
    },
    observedDays: {
      count: heatmap.filter((day) => day.observed).length,
      timezoneOffsetMinutes: -new Date().getTimezoneOffset(),
    },
    timeNearLimit: nearLimitEstimate(seriesList, [80, 95]),
  };
}

function analytics(
  range: "24h" | "7d",
  rangeStartMs: number,
  rangeEndMs: number,
  seriesList: UsageTrendSeries[],
  exactness: "exact" | "lowerBound",
): UsageAnalytics {
  const heatmap = heatmapFrom(seriesList, rangeStartMs, rangeEndMs, exactness);
  return {
    schemaVersion: 1,
    range,
    rangeStart: iso(rangeStartMs),
    rangeEnd: iso(rangeEndMs),
    generatedAt: iso(rangeEndMs),
    timezoneOffsetMinutes: -new Date().getTimezoneOffset(),
    summary: summary(seriesList, heatmap, exactness),
    heatmap,
    trends: seriesList,
    gapSemantics: "notObservedNeverZeroFilled",
    availabilityInference: "none",
  };
}

function usage(
  id: string,
  name: string,
  limits: Array<{ label: string; usedPercent: number; resetAt?: string }>,
  account?: { label: string },
): ProviderUsage {
  return {
    id,
    name,
    status: "ok",
    health: "live",
    checkedAt: iso(Date.now()),
    limits,
    ...(account ? { account: { label: account.label, note: "Primary workspace" } } : {}),
  };
}

/** Rises to the day's peak, one cycle, ending "now". */
function rising(
  startMs: number,
  endMs: number,
  stepMs: number,
  from: number,
  to: number,
  cycleId: string,
): UsageTrendPoint[] {
  const points: UsageTrendPoint[] = [];
  for (let at = startMs, index = 0; at <= endMs; at += stepMs, index += 1) {
    points.push(
      point(
        at,
        Math.round(from + ((to - from) * index) / ((endMs - startMs) / stepMs)),
        cycleId,
        index === 0 ? { cycleStart: true } : {},
      ),
    );
  }
  return points;
}

function buildFixtures(): Record<string, DevFixture> {
  const now = Date.now();

  // A — normal 24h: two Codex windows, a Grok reset overnight, flat Z.ai.
  // Series gap thresholds sit above the authoring step sizes so intended
  // observations connect (only the Grok cycle change breaks the line).
  const a24Start = now - 24 * HOUR;
  const a24 = analytics(
    "24h",
    a24Start,
    now,
    [
      series("openai-codex", "Weekly credits", undefined, rising(a24Start + HOUR, now - 10 * MINUTE, 2 * HOUR, 38, 84, "codex-weekly"), a24Start, now, 2.5 * HOUR),
      series("openai-codex", "5-hour window", undefined, rising(now - 5 * HOUR, now - 15 * MINUTE, 45 * MINUTE, 20, 62, "codex-5h"), a24Start, now),
      series("grok", "Weekly", undefined, [
        point(now - 20 * HOUR, 34, "grok-w0", { cycleStart: true }),
        point(now - 18 * HOUR, 51, "grok-w0"),
        point(now - 16 * HOUR, 66, "grok-w0"),
        // Reset: usage drops 66 → 4 with a moved resetAt — the boundary opens
        // the new cycle and the line breaks here.
        point(now - 12 * HOUR, 4, "grok-w1", { cycleStart: true, resetBoundary: true }),
        point(now - 8 * HOUR, 9, "grok-w1"),
        point(now - 4 * HOUR, 13, "grok-w1"),
      ], a24Start, now, 4.5 * HOUR),
      series("zai", "30-day credits", "key:3456", rising(a24Start + HOUR, now - 20 * MINUTE, 6 * HOUR, 40, 44, "zai-30d"), a24Start, now, 6.5 * HOUR),
    ],
    "exact",
  );

  // B — 7d compacted/lower-bound: half-day steps, lower-bound peaks, one
  // unobserved day.
  const b7Start = now - 7 * 24 * HOUR;
  const b7Points = [] as UsageTrendPoint[];
  for (let at = b7Start + 6 * HOUR, index = 0; at <= now; at += 12 * HOUR, index += 1) {
    if (index === 4) continue; // one whole unobserved day mid-range
    b7Points.push(
      point(at, [22, 41, 58, 73, 0, 67, 88, 91, 74][index] ?? 30, "codex-weekly-7d", {
        resolution: "compacted",
        cycleStart: index === 0,
        resetBoundary: index === 5,
      }),
    );
  }
  const b7 = analytics(
    "7d",
    b7Start,
    now,
    [
      series("openai-codex", "Weekly credits", undefined, b7Points, b7Start, now, 13 * HOUR),
      series("grok", "Weekly", undefined, b7Points.map((p) => ({
        ...p,
        usedPercent: Math.max(0, p.usedPercent - 17),
        cycleId: `${p.cycleId}-grok`,
      })), b7Start, now, 13 * HOUR),
    ],
    "lowerBound",
  );

  // C — reset boundary in focus: Codex weekly rolls over mid-range.
  const cStart = now - 24 * HOUR;
  const c = analytics(
    "24h",
    cStart,
    now,
    [
      series("openai-codex", "Weekly credits", undefined, [
        point(cStart + HOUR, 88, "cw-old", { cycleStart: true }),
        point(cStart + 4 * HOUR, 93, "cw-old"),
        point(cStart + 8 * HOUR, 97, "cw-old"),
        point(cStart + 12 * HOUR, 3, "cw-new", { cycleStart: true, resetBoundary: true }),
        point(cStart + 16 * HOUR, 11, "cw-new"),
        point(cStart + 20 * HOUR, 19, "cw-new"),
      ], cStart, now, 5 * HOUR),
    ],
    "exact",
  );

  // D — gaps: a 3.5h not-observed span mid-series plus unobserved heatmap days.
  const dStart = now - 24 * HOUR;
  const d = analytics(
    "24h",
    dStart,
    now,
    [
      series("openai-codex", "5-hour window", undefined, [
        point(dStart + 30 * MINUTE, 12, "d1", { cycleStart: true }),
        point(dStart + 75 * MINUTE, 21, "d1"),
        point(dStart + 2 * HOUR, 33, "d1"),
        // 3.5h of nothing — beyond the gap threshold, never zero-filled.
        point(dStart + 5.5 * HOUR, 47, "d1"),
        point(dStart + 7 * HOUR, 58, "d1"),
        point(dStart + 9 * HOUR, 64, "d1"),
      ], dStart, now, 2.5 * HOUR),
    ],
    "exact",
  );

  // E — empty history.
  const eStart = now - 24 * HOUR;
  const e = analytics("24h", eStart, now, [], "exact");
  const e7 = analytics("7d", now - 7 * 24 * HOUR, now, [], "lowerBound");

  const codexUsage = usage("openai-codex", "OpenAI / Codex", [
    { label: "5-hour window", usedPercent: 62, resetAt: iso(now + 3 * HOUR) },
    { label: "Weekly credits", usedPercent: 84, resetAt: iso(now + 3 * 24 * HOUR) },
  ]);
  const usages: ProviderUsage[] = [
    codexUsage,
    usage("zai", "Z.ai", [{ label: "30-day credits", usedPercent: 44 }], {
      label: "key:3456",
    }),
    usage("grok", "Grok (xAI)", [{ label: "Weekly", usedPercent: 13, resetAt: iso(now + 4 * 24 * HOUR) }]),
  ];

  // Focused quota-perspective geometry: one ordinary window and one critical
  // window for deterministic provider-card and floating-bar screenshots.
  const quota20Usage = usage("openai-codex", "OpenAI / Codex", [
    { label: "Weekly credits", usedPercent: 20, resetAt: iso(now + 3 * 24 * HOUR) },
  ]);
  const quota95Usage = usage("openai-codex", "OpenAI / Codex", [
    { label: "Weekly credits", usedPercent: 95, resetAt: iso(now + 4 * 24 * HOUR) },
  ]);
  quota20Usage.planType = "team";
  quota20Usage.resetCredits = {
    bankedCredits: 3,
    currentlyApplicable: 0,
    checkedAt: iso(now),
    source: "codex-wham-usage",
  };
  quota95Usage.planType = "team";
  quota95Usage.resetCredits = {
    bankedCredits: 3,
    currentlyApplicable: 0,
    checkedAt: iso(now),
    source: "codex-wham-usage",
  };

  const byName: Record<string, { usages: ProviderUsage[]; byRange: Record<"24h" | "7d", UsageAnalytics> }> = {
    default: { usages, byRange: { "24h": a24, "7d": b7 } },
    "lower-bound": { usages, byRange: { "24h": a24, "7d": b7 } },
    "reset-boundary": {
      usages: [codexUsage],
      byRange: { "24h": c, "7d": c },
    },
    gaps: {
      usages: [codexUsage],
      byRange: { "24h": d, "7d": d },
    },
    empty: {
      usages,
      byRange: { "24h": e, "7d": e7 },
    },
    "quota-bars": {
      usages: [quota20Usage, quota95Usage],
      byRange: { "24h": e, "7d": e7 },
    },
    "quota-20": {
      usages: [quota20Usage],
      byRange: { "24h": e, "7d": e7 },
    },
    "quota-95": {
      usages: [quota95Usage],
      byRange: { "24h": e, "7d": e7 },
    },
  };

  const built: Record<string, DevFixture> = {};
  for (const [name, fixture] of Object.entries(byName)) {
    built[name] = {
      name,
      usages: fixture.usages,
      loader: async (query: UsageAnalyticsQuery) => {
        const base = fixture.byRange[query.range ?? "24h"];
        if (!query.providerId && !query.windowLabel) return base;
        const filtered = base.trends.filter(
          (list) =>
            (!query.providerId || list.providerId === query.providerId) &&
            (!query.windowLabel || list.windowLabel === query.windowLabel),
        );
        const exactness = query.range === "7d" ? "lowerBound" : "exact";
        const startMs = Date.parse(base.rangeStart);
        const endMs = Date.parse(base.rangeEnd);
        return analytics(query.range ?? "24h", startMs, endMs, filtered, exactness);
      },
    };
  }

  Object.assign(built, intelligenceFixtures(now));
  return built;
}

/* ----------------------------------------------------------------- */
/* v0.9 Usage-first overview fixtures (Usage Intelligence payloads).  */
/* ----------------------------------------------------------------- */

function sums(
  events: number,
  input: number,
  cacheRead: number,
  cacheWrite: number,
  output: number,
  reasoning: number,
): UsageTokenSums {
  return {
    events,
    inputTokens: input,
    cacheReadTokens: cacheRead,
    cacheWriteTokens: cacheWrite,
    outputTokens: output,
    reasoningTokens: reasoning,
    totalTokens: input + cacheRead + cacheWrite + output + reasoning,
  };
}

function group(
  provider: string,
  models: UsageModelBreakdown[],
): UsageProviderBreakdown {
  const totals = models.reduce(
    (acc, model) => ({
      events: acc.events + model.events,
      inputTokens: acc.inputTokens + model.inputTokens,
      cacheReadTokens: acc.cacheReadTokens + model.cacheReadTokens,
      cacheWriteTokens: acc.cacheWriteTokens + model.cacheWriteTokens,
      outputTokens: acc.outputTokens + model.outputTokens,
      reasoningTokens: acc.reasoningTokens + model.reasoningTokens,
      totalTokens: acc.totalTokens + model.totalTokens,
    }),
    sums(0, 0, 0, 0, 0, 0),
  );
  return { provider, models, ...totals };
}

function intelligencePayload(
  range: UsageIntelligenceRange,
  rangeStartMs: number,
  rangeEndMs: number,
  groups: UsageProviderBreakdown[],
  options: {
    collectionStartedAt?: string;
    incompleteHistory?: boolean;
    sources?: UsageSourceDiagnostics[];
  } = {},
): UsageIntelligence {
  const totals = groups.reduce(
    (acc, g) => ({
      events: acc.events + g.events,
      inputTokens: acc.inputTokens + g.inputTokens,
      cacheReadTokens: acc.cacheReadTokens + g.cacheReadTokens,
      cacheWriteTokens: acc.cacheWriteTokens + g.cacheWriteTokens,
      outputTokens: acc.outputTokens + g.outputTokens,
      reasoningTokens: acc.reasoningTokens + g.reasoningTokens,
      totalTokens: acc.totalTokens + g.totalTokens,
    }),
    sums(0, 0, 0, 0, 0, 0),
  );
  return {
    schemaVersion: 1,
    range,
    rangeStart: iso(rangeStartMs),
    rangeEnd: iso(rangeEndMs),
    generatedAt: iso(rangeEndMs),
    enabled: true,
    eventsInRange: totals.events,
    groups,
    totals,
    ...(options.collectionStartedAt
      ? { collectionStartedAt: options.collectionStartedAt }
      : {}),
    incompleteHistory: options.incompleteHistory ?? false,
    sources: options.sources ?? [],
    semantics: "reportedTokenUsageCollectedLocally",
  };
}

function rangeSpan(now: number, range: UsageIntelligenceRange): [number, number] {
  if (range === "today") {
    const midnight = new Date(now);
    midnight.setHours(0, 0, 0, 0);
    return [midnight.getTime(), now];
  }
  const days = range === "7d" ? 7 : 30;
  return [now - days * 24 * HOUR, now];
}

/** Rich, partial, empty, and off states for the Usage-first overview. */
function intelligenceFixtures(now: number): Record<string, DevFixture> {
  const started = iso(now - 9 * 24 * HOUR);
  const watermark = iso(now - 40 * MINUTE);

  const richGroups = [
    group("zai", [
      { model: "GLM-5.3", ...sums(96, 412_000, 3_150_000, 148_000, 530_000, 302_000) },
      { model: "GLM-5.3-Air", ...sums(41, 120_000, 640_000, 22_000, 141_000, 34_000) },
      { model: "GLM-4.6", ...sums(9, 21_000, 40_000, 4_000, 26_000, 6_000) },
    ]),
    group("openai-codex", [
      { model: "gpt-5.1-codex", ...sums(58, 980_000, 2_240_000, 96_000, 410_000, 0) },
      { model: "gpt-5.1", ...sums(17, 84_000, 130_000, 8_000, 52_000, 0) },
    ]),
  ];
  const healthySources: UsageSourceDiagnostics[] = [
    {
      source: "zcode",
      state: "ok",
      watermarkAt: watermark,
      baselinedAt: started,
      lastObservedAt: iso(now - 12 * MINUTE),
      lastScan: { accepted: 12, duplicates: 0, rejected: 0 },
    },
    {
      source: "codex",
      state: "collecting",
      watermarkAt: watermark,
      baselinedAt: started,
      lastObservedAt: iso(now - 3 * MINUTE),
      lastScan: { accepted: 7, duplicates: 1, rejected: 0 },
    },
    {
      source: "opencode",
      state: "ok",
      watermarkAt: watermark,
      baselinedAt: started,
      lastObservedAt: iso(now - 2 * HOUR),
      lastScan: { accepted: 5, duplicates: 0, rejected: 1, usageNotReported: 23 },
    },
  ];

  const partialGroups = [group("zai", richGroups[0].models.slice(0, 2))];
  const partialSources: UsageSourceDiagnostics[] = [
    healthySources[0],
    {
      source: "codex",
      state: "readFailure",
      watermarkAt: watermark,
      baselinedAt: started,
      lastObservedAt: iso(now - 26 * HOUR),
      detail: "history file locked by another process",
    },
    {
      source: "opencode",
      state: "sourceAbsent",
      watermarkAt: started,
      baselinedAt: started,
    },
  ];

  const byName: Record<
    string,
    {
      usageIntelligenceEnabled: boolean;
      payload: (range: UsageIntelligenceRange) => UsageIntelligence;
    }
  > = {
    "usage-first": {
      usageIntelligenceEnabled: true,
      payload: (range) => {
        const [start, end] = rangeSpan(now, range);
        return intelligencePayload(range, start, end, richGroups, {
          collectionStartedAt: started,
          incompleteHistory: range === "30d",
          sources: healthySources,
        });
      },
    },
    "usage-first-partial": {
      usageIntelligenceEnabled: true,
      payload: (range) => {
        const [start, end] = rangeSpan(now, range);
        return intelligencePayload(range, start, end, partialGroups, {
          collectionStartedAt: started,
          incompleteHistory: true,
          sources: partialSources,
        });
      },
    },
    "usage-first-empty": {
      usageIntelligenceEnabled: true,
      payload: (range) => {
        const [start, end] = rangeSpan(now, range);
        return intelligencePayload(range, start, end, [], {
          collectionStartedAt: iso(now - 3 * 24 * HOUR),
          sources: healthySources,
        });
      },
    },
    "usage-first-off": {
      usageIntelligenceEnabled: false,
      payload: (range) => {
        const [start, end] = rangeSpan(now, range);
        return intelligencePayload(range, start, end, []);
      },
    },
  };

  const built: Record<string, DevFixture> = {};
  for (const [name, fixture] of Object.entries(byName)) {
    built[name] = {
      name,
      usages: [],
      loader: async () =>
        analytics("24h", now - 24 * HOUR, now, [], "exact"),
      usageIntelligenceEnabled: fixture.usageIntelligenceEnabled,
      intelligenceLoader: async (query: UsageIntelligenceQuery) =>
        fixture.payload(query.range),
    };
  }
  return built;
}

let fixtures: Record<string, DevFixture> | null = null;

/** `?fixture=name` → fixture; anything else (or production) → null. */
export function readDevFixture(search: string): DevFixture | null {
  const name = new URLSearchParams(search).get("fixture");
  if (!name) return null;
  fixtures ??= buildFixtures();
  return fixtures[name] ?? null;
}

/**
 * `?theme=glass|graphite|oled` and `?perspective=used|remaining` persist the
 * corresponding settings before the app mounts so visual acceptance can
 * screenshot each state without UI clicks. Runs in dev only; unknown values
 * are ignored (settings validation agrees).
 */
export function applyDevFixtureTheme(search: string): void {
  const params = new URLSearchParams(search);
  const theme = params.get("theme") as Theme | null;
  const perspective = params.get("perspective") as QuotaPerspective | null;
  const hasTheme = theme !== null && (THEMES as readonly string[]).includes(theme);
  const hasPerspective =
    perspective !== null &&
    (QUOTA_PERSPECTIVES as readonly string[]).includes(perspective);
  if (!hasTheme && !hasPerspective) return;
  const settings = loadSettings();
  try {
    window.localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({
        ...settings,
        ...(hasTheme ? { theme } : {}),
        ...(hasPerspective ? { quotaPerspective: perspective } : {}),
      }),
    );
  } catch {
    // Storage unavailable (private mode) — the default theme renders.
  }
}
