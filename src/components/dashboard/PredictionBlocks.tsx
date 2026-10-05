import { useState, type KeyboardEvent } from "react";
import { formatResetTime } from "../../lib/format";
import { predictionBasisLabel } from "../../lib/v03Integration";
import type { QuotaPrediction, ThresholdEstimate } from "../../lib/prediction/types";
import { formatEstimateDuration } from "../../lib/usagePresentation";
import {
  predictionQuotaPresentation,
  type QuotaPerspective,
} from "../../lib/quotaPresentation";

/**
 * A02/K04 controlled-reveal halves for one tooltip-bearing row: click/tap
 * pinning (the tooltip persists while reading, independent of hover) and
 * Escape dismissal with reopening suppression. Escape never moves focus;
 * the suppression holds until a deliberate refocus (focus leaves the row)
 * or deliberate activation (a click), never on mere pointer re-entry.
 * The reveal state is carried as `data-` attributes that styles.css turns
 * into the show/hide rules; hover/focus reveal itself stays CSS so pointer
 * travel into the tooltip cannot close it. Escape is handled at this
 * component level only — no global listener.
 */
function useTooltipReveal() {
  const [pinned, setPinned] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  return {
    "data-tooltip-pinned": pinned ? "true" : "false",
    "data-tooltip-suppressed": dismissed ? "true" : "false",
    onBlur: () => {
      // Focus moved away deliberately: the Escape suppression ends here,
      // so a deliberate refocus can reveal the tooltip again.
      setDismissed(false);
    },
    onClick: () => {
      // A click is a deliberate activation: it always ends an Escape
      // suppression, and it toggles the pin.
      if (dismissed) {
        setDismissed(false);
        setPinned(true);
      } else {
        setPinned((was) => !was);
      }
    },
    onKeyDown: (event: KeyboardEvent<HTMLElement>) => {
      if (event.key === "Escape") {
        setPinned(false);
        setDismissed(true);
      }
    },
  };
}

/**
 * Zone A's single pace line. Deliberately short and visually quiet — the
 * prediction is context for the big number, never the dominant element. It
 * renders only for a prediction that already passed the product visibility
 * gate (`visiblePrediction`), and only claims what the engine concluded:
 * exhaustion before reset when the engine says so, otherwise the projected
 * percent at reset. No fabricated precision beyond the engine's output.
 */
export function PredictionSummary({
  prediction,
  perspective,
}: {
  prediction: QuotaPrediction;
  perspective: QuotaPerspective;
}) {
  const basis = predictionBasisLabel(prediction);
  const projected = predictionQuotaPresentation(
    prediction.projectedPercentAtReset,
    perspective,
  );
  const line =
    prediction.willExhaustBeforeReset === true
      ? "At current pace · may reach the limit before reset"
      : `At current pace · ${projected.displayPercent === null ? "—" : `${projected.displayPercent}%`} ${perspective === "remaining" ? "remaining" : "used"} at reset`;
  const reveal = useTooltipReveal();
  return (
    <p
      className="pace-note"
      tabIndex={0}
      aria-label={`${line} (${basis})`}
      data-basis={basis}
      {...reveal}
    >
      {line}
      <span className="prediction-tooltip" role="tooltip">
        {basis}
      </span>
    </p>
  );
}

/**
 * The visible text for one threshold estimate, as a pure function of
 * `(estimate, nowMs)`. Durations are recomputed at render from
 * `estimatedAt − nowMs`; the freshness gate bounds how far the anchor and the
 * wall clock can diverge. The wording always speaks in used terms
 * ("X% used") so the Remaining perspective can never read it inverted.
 */
export function thresholdRowText(
  estimate: ThresholdEstimate,
  nowMs: number,
): string {
  const level = estimate.thresholdPercent;
  if (estimate.alreadyCrossed === true) {
    // An observed fact, not a projection.
    return `${level}% used reached`;
  }
  if (estimate.crossesBeforeReset === false) {
    return `${level}% used not expected before reset`;
  }
  if (estimate.crossesBeforeReset === true) {
    const estimatedAtMs = estimate.estimatedAt
      ? Date.parse(estimate.estimatedAt)
      : NaN;
    if (Number.isFinite(estimatedAtMs)) {
      const duration = formatEstimateDuration(estimatedAtMs - nowMs);
      // "~" marks the projection as approximate; "<1m" already is one.
      const approximate = duration.startsWith("<") ? duration : `~${duration}`;
      return `Est. ${level}% used in ${approximate}`;
    }
  }
  return "Estimate unavailable";
}

/**
 * One v0.8 threshold row: the exact string table text plus its own
 * basis tooltip with the full A02 reveal contract (pin, Escape,
 * suppression) — each row owns its reveal state independently.
 */
function ThresholdRow({
  estimate,
  basis,
  nowMs,
}: {
  estimate: ThresholdEstimate;
  basis: string;
  nowMs: number;
}) {
  const reveal = useTooltipReveal();
  const text = thresholdRowText(estimate, nowMs);
  return (
    <span
      className="prediction-threshold"
      tabIndex={0}
      aria-label={`${text} (${basis})`}
      data-basis={basis}
      data-threshold-percent={estimate.thresholdPercent}
      {...reveal}
    >
      {text}
      <span className="prediction-tooltip" role="tooltip">
        {basis}
      </span>
    </span>
  );
}

/**
 * Zone B's per-window projection detail (unchanged behavior from the card
 * era): burn rate, projected percent at reset, likely exhaustion, and the
 * sample basis on hover/focus — plus the v0.8 threshold rows, one per level
 * the engine resolved, ascending 80 → 95. Shown only behind the same
 * visibility gate (`visiblePrediction`), which stays the single gate: no
 * threshold text can appear at low/insufficient confidence because the whole
 * block is absent there.
 */
export function PredictionDetail({
  prediction,
  perspective,
  now,
}: {
  prediction: QuotaPrediction;
  perspective: QuotaPerspective;
  now: Date;
}) {
  const basis = predictionBasisLabel(prediction);
  const projected = predictionQuotaPresentation(
    prediction.projectedPercentAtReset,
    perspective,
  );
  const confidence = prediction.confidence === "high" ? "High" : "Medium";
  const showExhaustion =
    prediction.willExhaustBeforeReset === true &&
    prediction.estimatedExhaustionAt !== undefined;
  const thresholds = prediction.thresholds ?? [];
  const reveal = useTooltipReveal();
  return (
    <div
      className="prediction"
      tabIndex={0}
      aria-label={basis}
      data-basis={basis}
      {...reveal}
    >
      <span className="prediction-title">Projection</span>
      <div className="prediction-grid">
        <span>
          {perspective === "remaining"
            ? "Projected remaining at reset"
            : "Projected at reset"}
        </span>
        <strong>{projected.displayPercent === null ? "—" : `${projected.displayPercent}%`}</strong>
        <span>Burn rate</span>
        <strong>{(prediction.burnRatePerHour ?? 0).toFixed(1)}%/h</strong>
        {showExhaustion ? (
          <>
            <span>Likely exhaustion</span>
            <strong>{formatResetTime(prediction.estimatedExhaustionAt!)}</strong>
          </>
        ) : null}
        <span>Confidence</span>
        <strong>{confidence}</strong>
      </div>
      {thresholds.length > 0 ? (
        <div className="prediction-thresholds">
          {thresholds.map((estimate) => (
            <ThresholdRow
              key={estimate.thresholdPercent}
              estimate={estimate}
              basis={basis}
              nowMs={now.getTime()}
            />
          ))}
        </div>
      ) : null}
      <span className="prediction-tooltip" role="tooltip">
        {basis}
      </span>
    </div>
  );
}
