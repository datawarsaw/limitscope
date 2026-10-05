import type { ThresholdPercent } from "../thresholds";

/**
 * Prototype quota-prediction model (see docs/prediction-design.md).
 *
 * The engine consumes plain historical snapshots and returns a transparent,
 * deterministic estimate. No machine learning, no provider knowledge; the
 * engine module itself stays a pure boundary — the app reaches it through
 * `src/hooks/useQuotaPredictions.ts`.
 */

/**
 * One percentage snapshot of a single quota window at a point in time.
 *
 * This is the only input the engine needs. It is deliberately a flat record
 * (no nested provider object) so a caller can build it from a `LimitWindow`,
 * a cache file, or a test fixture with equal ease.
 */
export type QuotaObservation = {
  providerId: string;
  windowLabel: string;
  /** Percent of the window already consumed. Values outside 0–100 are clamped. */
  usedPercent: number;
  /** When this snapshot was taken (RFC-3339). */
  observedAt: string;
  /**
   * When the window is scheduled to reset (RFC-3339). Optional: some
   * providers omit it. A missing reset removes the *projection* fields, not
   * the burn rate.
   */
  resetAt?: string;
};

/**
 * Confidence in the projected outcome, not a statistical claim.
 *
 * - `insufficient`: fewer than two usable samples, or no measurable span —
 *   no burn rate is reported at all.
 * - `low`: a burn rate exists but is not trustworthy enough to act on: a
 *   stale source, a span under one hour, or no usable reset bound.
 * - `medium`: at least one hour of span from at least two samples, with a
 *   fresh source and a future reset bound.
 * - `high`: at least three hours of span, at least five samples, and a mean
 *   sampling gap of at most thirty minutes.
 *
 * Exact rules live in `confidenceFor` in engine.ts and are tabulated in
 * docs/prediction-design.md.
 */
export type ConfidenceLevel = "insufficient" | "low" | "medium" | "high";

/**
 * Coarse exhaustion risk derived from the projection. `undefined` when no
 * projection could be made (no burn rate, or no reset bound to project to).
 * Exact thresholds are documented in docs/prediction-design.md.
 */
export type ExhaustionRisk = "low" | "medium" | "high";

/**
 * Tuning surface. Every field is optional and has a documented default; the
 * confidence thresholds are deliberately *not* configurable, because there is
 * no data yet to justify a tuning UI.
 */
export type PredictionOptions = {
  /** Samples older than this (relative to the newest sample) are excluded
   * from the burn-rate fit. Default: 6 hours. */
  rateWindowMs?: number;
  /** Samples older than this (relative to `now`) are ignored entirely.
   * Default: 24 hours. */
  maxSampleAgeMs?: number;
  /** If the newest usable sample is older than this, the source is stale and
   * confidence is capped at `low`. Default: 30 minutes. */
  staleAfterMs?: number;
  /** A backward percentage jump larger than this starts a new segment (a
   * reset). Smaller jumps are treated as provider rounding noise.
   * Default: 5 points. */
  resetDropPoints?: number;
  /** A forward `resetAt` movement larger than this starts a new segment.
   * Default: 60 seconds. */
  resetAtChangeToleranceMs?: number;
  /** Samples stamped further than this in the future (relative to `now`) are
   * dropped as clock skew. Default: 5 minutes. */
  clockSkewToleranceMs?: number;
};

/**
 * One reset-bounded run of samples. A reset starts a new segment, so samples
 * from different quota cycles are never blended into one burn rate.
 */
export type QuotaSegment = {
  /** Stable identity: `providerId|windowLabel|index`. */
  segmentId: string;
  providerId: string;
  windowLabel: string;
  /** 1-based position of this segment within its provider/window series. */
  segmentIndex: number;
  /** Ascending by `observedAt`, invalid samples removed, percentages clamped. */
  samples: QuotaObservation[];
  /** Newest `resetAt` reported inside this segment, when any sample had one. */
  resetAt?: string;
  /** True when a detected reset (percentage drop or advanced `resetAt`)
   * opened this segment rather than it being the first one observed. */
  openedByReset: boolean;
};

/**
 * Which samples and which segment produced the numbers. Reported so a caller
 * (or a test) can explain a prediction instead of trusting it.
 */
export type PredictionBasis = {
  /** `none` when no usable samples exist for this window. */
  segmentId: string;
  /** Number of segments found for this provider/window (resets + 1). */
  segmentCount: number;
  /** Usable samples in the newest segment. */
  segmentSampleCount: number;
  /** Samples the burn-rate fit actually used. */
  fitSampleCount: number;
  /** Span of the fit, in minutes. */
  fitSpanMinutes: number;
  /** Mean gap between fit samples, in minutes. */
  fitMeanGapMinutes: number;
  /** True when the recent-sample window held fewer than two samples and the
   * fit fell back to the whole segment. */
  usedWholeSegment: boolean;
  /** `usedPercent` of the newest usable sample, when there is one. */
  latestUsedPercent?: number;
  firstObservedAt?: string;
  lastObservedAt?: string;
  /** Age of the newest usable sample, in minutes. Never negative: a sample
   * stamped slightly in the future reads as age 0 (clock skew). */
  latestAgeMinutes?: number;
  isStale: boolean;
  resetAt?: string;
  /** True when `resetAt` is at or before `now`: the snapshot is inconsistent
   * with its own reset time, so nothing is projected against it. */
  resetExpired: boolean;
  /** Live countdown from `now` to `resetAt`, in minutes. Distinct from the
   * projection anchor, which is the newest observation — this one ticks. */
  minutesToReset?: number;
};

/**
 * When usage is projected to reach one canonical threshold level (80/95, see
 * `src/lib/thresholds.ts`) inside the current cycle, anchored on the newest
 * observation. Display-layer knowledge only: it never alters risk, fires a
 * notification, or enters history.
 */
export type ThresholdEstimate = {
  /** Which level this estimate is about. */
  thresholdPercent: ThresholdPercent;
  /** true: the fitted line reaches the level before `resetAt` (or the newest
   * observation is already there); false: it does not; undefined:
   * unresolvable from current data. */
  crossesBeforeReset: boolean | undefined;
  /** ISO-8601 UTC instant of the projected crossing, anchored on the newest
   * observation. Present only when crossesBeforeReset === true and the level
   * is not already crossed. */
  estimatedAt?: string;
  /** True when the newest observation already sits at or above the level —
   * an observed fact, not a projection. */
  alreadyCrossed?: boolean;
};

/**
 * The estimate for one quota window. The first five fields are the requested
 * output; `risk` and `basis` are diagnostic extras.
 */
export type QuotaPrediction = {
  providerId: string;
  windowLabel: string;
  /** Linear burn rate over the analysis window, in percent points per hour.
   * Never negative: within a segment, usage does not un-burn. */
  burnRatePerHour?: number;
  /** `usedPercent` at `resetAt` at the current burn rate, clamped to 0–100.
   * Projected from the newest observation, so it stays stable between
   * samples. Requires a usable (future) `resetAt`. */
  projectedPercentAtReset?: number;
  /** When usage would reach 100% at the current burn rate, anchored on the
   * newest observation. When the window is already at 100%, this is the
   * observation time itself. */
  estimatedExhaustionAt?: string;
  /** Whether exhaustion lands before `resetAt`. Requires a usable `resetAt`;
   * `undefined` when the burn rate could not be measured, or when the
   * estimated exhaustion time is already in the past without a confirming
   * observation (usage below 100%) — the claim is suppressed until the next
   * real sample rather than warning about a past event. */
  willExhaustBeforeReset?: boolean;
  confidence: ConfidenceLevel;
  risk?: ExhaustionRisk;
  basis: PredictionBasis;
  /** Per-level threshold estimates, ascending by `thresholdPercent`, at most
   * the two canonical levels. A level is omitted entirely when nothing is
   * knowable about it (no burn-rate fit, or no usable reset bound to be
   * "before" of). Populated at any confidence — rendering stays gated by the
   * single product visibility gate. */
  thresholds?: ThresholdEstimate[];
};

/** Input for a single window estimate. */
export type PredictWindowInput = {
  observations: readonly QuotaObservation[];
  providerId: string;
  windowLabel: string;
  /** Reference time (RFC-3339). Required, so results never depend on the
   * wall clock and are reproducible in tests. */
  now: string;
  options?: PredictionOptions;
};

/** Input for estimating every window present in one observation list. */
export type PredictAllInput = {
  observations: readonly QuotaObservation[];
  now: string;
  options?: PredictionOptions;
};
