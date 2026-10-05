import type {
  ConfidenceLevel,
  ExhaustionRisk,
  PredictAllInput,
  PredictionBasis,
  PredictionOptions,
  PredictWindowInput,
  QuotaObservation,
  QuotaPrediction,
  QuotaSegment,
  ThresholdEstimate,
} from "./types";
import {
  CRITICAL_PERCENT,
  NEAR_LIMIT_PERCENT,
  type ThresholdPercent,
} from "../thresholds";

const MINUTE_MS = 60_000;
const HOUR_MS = 3_600_000;
// Largest epoch-milliseconds value ECMAScript Dates can represent; beyond it
// `new Date(ms)` is an Invalid Date and `toISOString` throws.
const MAX_DATE_MS = 8.64e15;

// Confidence thresholds. Module constants rather than options on purpose: the
// prototype ships one calibrated rule set (docs/prediction-design.md) instead
// of a tuning surface nobody has data for yet.
const MEDIUM_SPAN_MS = 60 * MINUTE_MS; // 1h of span
const HIGH_SPAN_MS = 180 * MINUTE_MS; // 3h of span
const HIGH_MIN_SAMPLES = 5;
const HIGH_MAX_MEAN_GAP_MS = 30 * MINUTE_MS;

/** Projected percent consumed at reset that separates low from medium risk. */
const RISK_MEDIUM_PROJECTED_PERCENT = 80;

export const DEFAULT_PREDICTION_OPTIONS: Required<PredictionOptions> = {
  rateWindowMs: 6 * HOUR_MS,
  maxSampleAgeMs: 24 * HOUR_MS,
  staleAfterMs: 30 * MINUTE_MS,
  resetDropPoints: 5,
  resetAtChangeToleranceMs: MINUTE_MS,
  clockSkewToleranceMs: 5 * MINUTE_MS,
};

/** A sample with its timestamps parsed and its percentage clamped. */
type Sample = {
  observedAtMs: number;
  usedPercent: number;
  resetAtMs?: number;
};

/** A point in the burn-rate fit: hours are relative to the fit's first sample. */
type FitPoint = { observedAtMs: number; usedPercent: number };

type Bucket = {
  providerId: string;
  windowLabel: string;
  samples: Sample[];
};

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, value));
}

function parseTime(value: string | undefined): number | undefined {
  if (!value) return undefined;
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : undefined;
}

function groupKey(providerId: string, windowLabel: string): string {
  return `${providerId}\u0000${windowLabel}`;
}

/**
 * Normalizes one raw observation, or rejects it. Rejected: a non-finite
 * percentage, a blank window label, or an unparseable `observedAt`. An
 * unparseable `resetAt` is not fatal — it is simply treated as absent.
 */
function normalizeSample(observation: QuotaObservation): Sample | undefined {
  if (!Number.isFinite(observation.usedPercent)) return undefined;
  if (!observation.windowLabel) return undefined;
  const observedAtMs = parseTime(observation.observedAt);
  if (observedAtMs === undefined) return undefined;
  const resetAtMs = parseTime(observation.resetAt);
  return {
    observedAtMs,
    usedPercent: clampPercent(observation.usedPercent),
    ...(resetAtMs !== undefined ? { resetAtMs } : {}),
  };
}

function toObservation(
  providerId: string,
  windowLabel: string,
  sample: Sample,
): QuotaObservation {
  return {
    providerId,
    windowLabel,
    usedPercent: sample.usedPercent,
    observedAt: new Date(sample.observedAtMs).toISOString(),
    ...(sample.resetAtMs !== undefined
      ? { resetAt: new Date(sample.resetAtMs).toISOString() }
      : {}),
  };
}

/**
 * True when `sample` opens a new quota cycle relative to `previous`.
 *
 * Two signals, both deliberately cheap and deterministic:
 * 1. the percentage dropped by more than `resetDropPoints` — a smaller drop
 *    is provider rounding noise, not a reset;
 * 2. `resetAt` moved *forward* by more than the tolerance — the provider
 *    announced a new cycle even if the percentage had not visibly dropped yet.
 *
 * A backward `resetAt` movement with no consumption drop is not a reset; the
 * newest value simply supersedes the older one, as a provider correction.
 */
function opensNewSegment(
  previous: Sample,
  sample: Sample,
  options: Required<PredictionOptions>,
): boolean {
  if (sample.usedPercent < previous.usedPercent - options.resetDropPoints) {
    return true;
  }
  if (
    previous.resetAtMs !== undefined &&
    sample.resetAtMs !== undefined &&
    sample.resetAtMs - previous.resetAtMs > options.resetAtChangeToleranceMs
  ) {
    return true;
  }
  return false;
}

function compareSegments(a: QuotaSegment, b: QuotaSegment): number {
  if (a.providerId !== b.providerId) return a.providerId < b.providerId ? -1 : 1;
  if (a.windowLabel !== b.windowLabel) return a.windowLabel < b.windowLabel ? -1 : 1;
  return a.segmentIndex - b.segmentIndex;
}

/**
 * Splits a raw observation list into reset-bounded segments: one series per
 * provider/window pair, one segment per quota cycle. Pure structure — no
 * `now`, no age filtering, no predictions. The engine never computes a burn
 * rate across two segments.
 */
export function buildSegments(
  observations: readonly QuotaObservation[],
  options: PredictionOptions = {},
): QuotaSegment[] {
  const resolved: Required<PredictionOptions> = {
    ...DEFAULT_PREDICTION_OPTIONS,
    ...options,
  };

  const buckets = new Map<string, Bucket>();
  for (const observation of observations) {
    const sample = normalizeSample(observation);
    if (!sample) continue;
    const key = groupKey(observation.providerId, observation.windowLabel);
    let bucket = buckets.get(key);
    if (!bucket) {
      bucket = {
        providerId: observation.providerId,
        windowLabel: observation.windowLabel,
        samples: [],
      };
      buckets.set(key, bucket);
    }
    bucket.samples.push(sample);
  }

  const segments: QuotaSegment[] = [];
  for (const bucket of buckets.values()) {
    // Ascending by time only; `sort` is stable, so samples sharing a
    // timestamp keep their input order and the last one wins below.
    const ordered = [...bucket.samples].sort(
      (a, b) => a.observedAtMs - b.observedAtMs,
    );
    const deduped: Sample[] = [];
    for (const sample of ordered) {
      const previous = deduped[deduped.length - 1];
      if (previous && previous.observedAtMs === sample.observedAtMs) {
        deduped[deduped.length - 1] = sample;
      } else {
        deduped.push(sample);
      }
    }

    let current: Sample[] = [];
    let openedByReset = false;
    let index = 0;
    const flush = (): void => {
      if (current.length === 0) return;
      index += 1;
      // The newest resetAt inside the segment wins: a window that was
      // rescheduled mid-flight is reported with its latest known bound.
      let resetAtMs: number | undefined;
      for (const sample of current) {
        if (sample.resetAtMs !== undefined) resetAtMs = sample.resetAtMs;
      }
      segments.push({
        segmentId: `${bucket.providerId}|${bucket.windowLabel}|${index}`,
        providerId: bucket.providerId,
        windowLabel: bucket.windowLabel,
        segmentIndex: index,
        samples: current.map((sample) =>
          toObservation(bucket.providerId, bucket.windowLabel, sample),
        ),
        ...(resetAtMs !== undefined
          ? { resetAt: new Date(resetAtMs).toISOString() }
          : {}),
        openedByReset,
      });
      current = [];
    };

    for (const sample of deduped) {
      const previous = current[current.length - 1];
      if (previous && opensNewSegment(previous, sample, resolved)) {
        flush();
        openedByReset = true;
      }
      current.push(sample);
    }
    flush();
  }

  return segments.sort(compareSegments);
}

/**
 * Ordinary least-squares slope in percent-per-hour over `points`, which must
 * be ascending by time and at least two long. Returns `undefined` when every
 * point shares one timestamp (no slope exists).
 */
function leastSquaresSlopePerHour(
  points: readonly FitPoint[],
): number | undefined {
  const origin = points[0].observedAtMs;
  const n = points.length;
  let sumX = 0;
  let sumY = 0;
  let sumXY = 0;
  let sumXX = 0;
  for (const point of points) {
    const x = (point.observedAtMs - origin) / HOUR_MS;
    sumX += x;
    sumY += point.usedPercent;
    sumXY += x * point.usedPercent;
    sumXX += x * x;
  }
  const denominator = n * sumXX - sumX * sumX;
  if (!(denominator > 0)) return undefined;
  return (n * sumXY - sumX * sumY) / denominator;
}

/** Deterministic confidence ladder — see docs/prediction-design.md. */
function confidenceFor(input: {
  burnRatePerHour: number | undefined;
  fitSampleCount: number;
  fitSpanMs: number;
  fitMeanGapMs: number;
  isStale: boolean;
  hasResetBound: boolean;
}): ConfidenceLevel {
  if (input.burnRatePerHour === undefined) return "insufficient";
  if (input.isStale) return "low";
  if (!input.hasResetBound) return "low";
  if (input.fitSpanMs < MEDIUM_SPAN_MS) return "low";
  if (
    input.fitSpanMs < HIGH_SPAN_MS ||
    input.fitSampleCount < HIGH_MIN_SAMPLES ||
    input.fitMeanGapMs > HIGH_MAX_MEAN_GAP_MS
  ) {
    return "medium";
  }
  return "high";
}

function emptyBasis(): PredictionBasis {
  return {
    segmentId: "none",
    segmentCount: 0,
    segmentSampleCount: 0,
    fitSampleCount: 0,
    fitSpanMinutes: 0,
    fitMeanGapMinutes: 0,
    usedWholeSegment: false,
    isStale: false,
    resetExpired: false,
  };
}

/**
 * Estimates one quota window from its historical percentage snapshots.
 *
 * Steps: drop samples that are ancient or future-dated, split them into
 * reset-bounded segments, fit a least-squares line over the newest segment's
 * recent window, then project that line onto the reset bound. Deterministic:
 * `now` is an argument and no wall clock or randomness is read.
 */
export function predictWindow(input: PredictWindowInput): QuotaPrediction {
  const options: Required<PredictionOptions> = {
    ...DEFAULT_PREDICTION_OPTIONS,
    ...input.options,
  };
  const nowMs = parseTime(input.now);
  if (nowMs === undefined) {
    throw new TypeError(`predictWindow: "now" is not a timestamp: ${input.now}`);
  }

  const usable = input.observations.filter((observation) => {
    if (
      observation.providerId !== input.providerId ||
      observation.windowLabel !== input.windowLabel
    ) {
      return false;
    }
    const observedAtMs = parseTime(observation.observedAt);
    if (observedAtMs === undefined) return false;
    if (nowMs - observedAtMs > options.maxSampleAgeMs) return false;
    if (observedAtMs - nowMs > options.clockSkewToleranceMs) return false;
    return true;
  });

  const segments = buildSegments(usable, options);
  if (segments.length === 0) {
    return {
      providerId: input.providerId,
      windowLabel: input.windowLabel,
      confidence: "insufficient",
      basis: emptyBasis(),
    };
  }

  // `usable` holds a single provider/window pair, so the last segment is the
  // newest one of that pair.
  const segment = segments[segments.length - 1];
  const samples = segment.samples;
  const timeline: FitPoint[] = samples.map((sample) => ({
    observedAtMs: Date.parse(sample.observedAt),
    usedPercent: sample.usedPercent,
  }));
  const latest = timeline[timeline.length - 1];
  const latestObservedAt = samples[samples.length - 1].observedAt;

  // Recent-sample window, with a documented fallback: a sparse tail must not
  // blank the estimate when the wider segment still holds usable samples.
  const cutoff = latest.observedAtMs - options.rateWindowMs;
  let fit = timeline.filter((point) => point.observedAtMs >= cutoff);
  let usedWholeSegment = false;
  if (fit.length < 2) {
    fit = timeline;
    usedWholeSegment = true;
  }

  const fitSpanMs = fit[fit.length - 1].observedAtMs - fit[0].observedAtMs;
  const fitMeanGapMs = fit.length > 1 ? fitSpanMs / (fit.length - 1) : 0;
  const slope =
    fit.length >= 2 && fitSpanMs > 0 ? leastSquaresSlopePerHour(fit) : undefined;
  // Clamped at zero: inside one segment usage does not un-burn. Remaining
  // small negative slopes are provider noise, and they can only mean idle.
  const burnRatePerHour = slope === undefined ? undefined : Math.max(0, slope);

  const latestAgeMs = Math.max(0, nowMs - latest.observedAtMs);
  const isStale = latestAgeMs > options.staleAfterMs;

  const resetAtMs = parseTime(segment.resetAt);
  const resetExpired = resetAtMs !== undefined && resetAtMs <= nowMs;
  const hasResetBound = resetAtMs !== undefined && !resetExpired;
  const minutesToReset = hasResetBound
    ? (resetAtMs - nowMs) / MINUTE_MS
    : undefined;
  // Projection anchor: the newest observation, not the ticking clock. A
  // projection recomputed against `now` would drift between provider samples
  // even though no new usage was observed.
  const minutesFromLatestToReset =
    hasResetBound && resetAtMs !== undefined
      ? Math.max(0, (resetAtMs - latest.observedAtMs) / MINUTE_MS)
      : undefined;

  const exhaustedNow = latest.usedPercent >= 100;

  const basis: PredictionBasis = {
    segmentId: segment.segmentId,
    segmentCount: segments.length,
    segmentSampleCount: timeline.length,
    fitSampleCount: fit.length,
    fitSpanMinutes: fitSpanMs / MINUTE_MS,
    fitMeanGapMinutes: fitMeanGapMs / MINUTE_MS,
    usedWholeSegment,
    latestUsedPercent: latest.usedPercent,
    firstObservedAt: samples[0].observedAt,
    lastObservedAt: latestObservedAt,
    latestAgeMinutes: latestAgeMs / MINUTE_MS,
    isStale,
    ...(segment.resetAt !== undefined ? { resetAt: segment.resetAt } : {}),
    resetExpired,
    ...(minutesToReset !== undefined ? { minutesToReset } : {}),
  };

  const confidence = confidenceFor({
    burnRatePerHour,
    fitSampleCount: fit.length,
    fitSpanMs,
    fitMeanGapMs,
    isStale,
    hasResetBound,
  });

  let projectedPercentAtReset: number | undefined;
  if (minutesFromLatestToReset !== undefined) {
    if (exhaustedNow) projectedPercentAtReset = 100;
    else if (burnRatePerHour !== undefined) {
      projectedPercentAtReset = clampPercent(
        latest.usedPercent + burnRatePerHour * (minutesFromLatestToReset / 60),
      );
    }
  }

  let estimatedExhaustionAt: string | undefined;
  if (exhaustedNow) {
    // An observation, not a projection: the window is already spent.
    estimatedExhaustionAt = latestObservedAt;
  } else if (burnRatePerHour !== undefined && burnRatePerHour > 0) {
    const hoursToExhaustion = (100 - latest.usedPercent) / burnRatePerHour;
    const exhaustionMs = latest.observedAtMs + hoursToExhaustion * HOUR_MS;
    // A near-zero positive burn rate — floating-point noise on an idle
    // (constant-percentage) series is enough — projects exhaustion absurdly
    // far out, past the largest representable Date. Constructing a Date from
    // that epoch would be an Invalid Date whose toISOString throws, taking
    // the whole render down with it. On that horizon there is no meaningful
    // exhaustion estimate, so the field is simply omitted.
    if (Number.isFinite(exhaustionMs) && Math.abs(exhaustionMs) <= MAX_DATE_MS) {
      estimatedExhaustionAt = new Date(exhaustionMs).toISOString();
    }
  }

  let willExhaustBeforeReset: boolean | undefined;
  if (hasResetBound && resetAtMs !== undefined) {
    if (exhaustedNow) {
      willExhaustBeforeReset = true;
    } else if (burnRatePerHour === 0) {
      willExhaustBeforeReset = false;
    } else if (estimatedExhaustionAt !== undefined) {
      const exhaustionMs = Date.parse(estimatedExhaustionAt);
      // estimatedExhaustionAt is anchored on the newest observation: once
      // the clock passes it without a confirming sample, exhaustion is
      // unconfirmed — suppress the claim instead of warning about a past
      // event, until the next real observation resolves it.
      willExhaustBeforeReset =
        exhaustionMs > nowMs ? exhaustionMs < resetAtMs : undefined;
    }
  }

  // Threshold estimates (per canonical level, ascending): display-layer
  // knowledge only — they never alter risk, fire a notification, or enter
  // history. Rule order matters: an observed crossing is a fact and wins
  // even without a fit; every projected claim defers to the reset bound.
  const thresholds: ThresholdEstimate[] = [];
  const thresholdLevels: ThresholdPercent[] = [
    NEAR_LIMIT_PERCENT,
    CRITICAL_PERCENT,
  ];
  for (const level of thresholdLevels) {
    if (latest.usedPercent >= level) {
      // An observation, not a projection: the newest sample established it
      // (exhaustedNow lands here for both levels).
      thresholds.push({
        thresholdPercent: level,
        crossesBeforeReset: true,
        alreadyCrossed: true,
        estimatedAt: latestObservedAt,
      });
    } else if (burnRatePerHour === undefined) {
      // No fit: nothing knowable, so the level is omitted entirely.
      continue;
    } else if (burnRatePerHour === 0) {
      // A clamped-zero burn cannot reach any higher level.
      thresholds.push({ thresholdPercent: level, crossesBeforeReset: false });
    } else if (resetAtMs === undefined || !hasResetBound || resetExpired) {
      // No usable cycle boundary to be "before" of; confidence is low
      // anyway, so nothing renders.
      continue;
    } else {
      const crossingMs =
        latest.observedAtMs +
        ((level - latest.usedPercent) / burnRatePerHour) * HOUR_MS;
      if (crossingMs > resetAtMs) {
        // Reset occurs first; the engine never projects across a cycle
        // boundary, and the honest statement is "not before reset", not a
        // post-reset time.
        thresholds.push({
          thresholdPercent: level,
          crossesBeforeReset: false,
        });
      } else if (
        crossingMs >= nowMs &&
        Number.isFinite(crossingMs) &&
        crossingMs <= MAX_DATE_MS
      ) {
        thresholds.push({
          thresholdPercent: level,
          crossesBeforeReset: true,
          estimatedAt: new Date(crossingMs).toISOString(),
        });
      } else {
        // The fit over-promised: a projected crossing already in the past
        // without a confirming sample reaching the level is suppressed
        // (mirrors willExhaustBeforeReset's anchor rule), as is anything
        // past the largest representable Date.
        thresholds.push({
          thresholdPercent: level,
          crossesBeforeReset: undefined,
        });
      }
    }
  }

  let risk: ExhaustionRisk | undefined;
  if (exhaustedNow) {
    risk = "high";
  } else if (willExhaustBeforeReset === true) {
    risk = "high";
  } else if (projectedPercentAtReset !== undefined) {
    risk =
      projectedPercentAtReset >= RISK_MEDIUM_PROJECTED_PERCENT
        ? "medium"
        : "low";
  }

  return {
    providerId: input.providerId,
    windowLabel: input.windowLabel,
    ...(burnRatePerHour !== undefined ? { burnRatePerHour } : {}),
    ...(projectedPercentAtReset !== undefined ? { projectedPercentAtReset } : {}),
    ...(estimatedExhaustionAt !== undefined ? { estimatedExhaustionAt } : {}),
    ...(willExhaustBeforeReset !== undefined ? { willExhaustBeforeReset } : {}),
    confidence,
    ...(risk !== undefined ? { risk } : {}),
    basis,
    ...(thresholds.length > 0 ? { thresholds } : {}),
  };
}

/**
 * Estimates every window present in one observation list, sorted by provider
 * then window label. A convenience wrapper over `predictWindow`.
 */
export function predictWindows(input: PredictAllInput): QuotaPrediction[] {
  const identities = new Map<string, { providerId: string; windowLabel: string }>();
  for (const observation of input.observations) {
    if (!observation.windowLabel) continue;
    const key = groupKey(observation.providerId, observation.windowLabel);
    if (!identities.has(key)) {
      identities.set(key, {
        providerId: observation.providerId,
        windowLabel: observation.windowLabel,
      });
    }
  }
  return [...identities.values()]
    .sort((a, b) => {
      if (a.providerId !== b.providerId) return a.providerId < b.providerId ? -1 : 1;
      if (a.windowLabel !== b.windowLabel) {
        return a.windowLabel < b.windowLabel ? -1 : 1;
      }
      return 0;
    })
    .map((identity) =>
      predictWindow({
        observations: input.observations,
        providerId: identity.providerId,
        windowLabel: identity.windowLabel,
        now: input.now,
        ...(input.options !== undefined ? { options: input.options } : {}),
      }),
    );
}
