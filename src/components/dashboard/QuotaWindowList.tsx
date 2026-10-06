import { PredictionDetail } from "./PredictionBlocks";
import { ZCodePlanGroups } from "./ZCodePlanGroups";
import { toneFor, usableQuotaWindows } from "../../lib/dashboard";
import { formatResetLine } from "../../lib/format";
import { visiblePrediction } from "../../lib/v03Integration";
import type { QuotaPrediction } from "../../lib/prediction/types";
import type { LimitWindow, ProviderUsage } from "../../types";
import {
  mainQuotaPresentation,
  type QuotaPerspective,
} from "../../lib/quotaPresentation";

/**
 * Zone B — explanation, not repetition: every valid quota window of the
 * selected provider as a compact row (label, percent, meter, reset), plus
 * the per-window projection detail behind the same visibility gate the card
 * era used. Malformed windows (empty label, non-finite or out-of-range
 * percent) are excluded here exactly as the quota strip excludes them, so a
 * broken payload can neither render nor feed the primary panel.
 */
export function QuotaWindowList({
  usage,
  now,
  predictionFor,
  perspective,
}: {
  usage: ProviderUsage;
  now: Date;
  predictionFor: (
    providerId: string,
    windowLabel: string,
  ) => QuotaPrediction | undefined;
  perspective: QuotaPerspective;
}) {
  const windows = usableQuotaWindows(usage);
  return (
    <section className="window-panel" aria-label={`${usage.name} quota windows`}>
      <h3 className="panel-heading">Quota windows</h3>
      {windows.length === 0 ? (
        <p className="window-empty">No quota windows available</p>
      ) : (
        <div>
          {windows.map((limit) => (
            <WindowRow
              key={limit.label}
              usage={usage}
              limit={limit}
              now={now}
              predictionFor={predictionFor}
              perspective={perspective}
            />
          ))}
        </div>
      )}
      <ZCodePlanGroups usage={usage} now={now} />
    </section>
  );
}

function WindowRow({
  usage,
  limit,
  now,
  predictionFor,
  perspective,
}: {
  usage: ProviderUsage;
  limit: LimitWindow;
  now: Date;
  predictionFor: (
    providerId: string,
    windowLabel: string,
  ) => QuotaPrediction | undefined;
  perspective: QuotaPerspective;
}) {
  const presentation = mainQuotaPresentation(limit.usedPercent, perspective);
  const tone = toneFor(limit.usedPercent);
  const resetLine = limit.resetAt ? formatResetLine(limit.resetAt, now) : null;
  const prediction = visiblePrediction(
    usage,
    limit.usedPercent,
    predictionFor(usage.id, limit.label),
  );
  return (
    <div className="limit">
      <div className="limit-row">
        <span className="limit-label">{limit.label}</span>
        <span className={`limit-percent tone-${tone}`}>{presentation.label}</span>
      </div>
      <div
        className="bar"
        role="progressbar"
        aria-label={`${limit.label} quota · ${presentation.label}`}
        aria-valuenow={presentation.meterAriaValueNow ?? undefined}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuetext={presentation.meterAriaValueText}
      >
        <div
          className={`bar-fill tone-${tone}`}
          style={{ width: `${Math.min(100, Math.max(0, presentation.meterPercent ?? 0))}%` }}
        />
      </div>
      {resetLine ? <span className="limit-reset">{resetLine}</span> : null}
      {presentation.isLimitReached ? (
        <span className="limit-reached">Limit reached</span>
      ) : prediction ? (
        <PredictionDetail prediction={prediction} perspective={perspective} now={now} />
      ) : null}
    </div>
  );
}
