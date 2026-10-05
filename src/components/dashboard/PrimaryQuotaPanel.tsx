import { PredictionSummary } from "./PredictionBlocks";
import {
  primaryQuotaWindow,
} from "../../lib/quotaStrip";
import { formatResetLine } from "../../lib/format";
import { formatResetCredits } from "../../lib/resetCredits";
import {
  providerSourceNote,
  providerStatusPresentation,
  visiblePrediction,
} from "../../lib/v03Integration";
import type { QuotaPrediction } from "../../lib/prediction/types";
import { toneFor } from "../../lib/dashboard";
import {
  mainQuotaPresentation,
  type QuotaPerspective,
} from "../../lib/quotaPresentation";
import type { ProviderUsage } from "../../types";

/**
 * Zone A — the visual anchor: the selected provider, its primary quota
 * window (deterministically the highest usable percent, exactly the quota
 * strip's concept), one large percentage, one strong meter, the reset
 * countdown, freshness, account/source attribution, and — only when the
 * prediction engine's visibility gate accepts it — the single pace line.
 * The big number stays neutral; tone lives on the meter, so high usage
 * never turns the whole panel red.
 */
export function PrimaryQuotaPanel({
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
  const status = providerStatusPresentation(usage);
  const primary = primaryQuotaWindow(usage);
  const presentation = mainQuotaPresentation(primary?.usedPercent, perspective);
  const tone = primary === undefined ? "calm" : toneFor(primary.usedPercent);
  const resetLine = primary?.resetAt ? formatResetLine(primary.resetAt, now) : null;
  const resetCredits = formatResetCredits(usage, now.getTime());
  const prediction =
    primary === undefined
      ? undefined
      : visiblePrediction(
          usage,
          primary.usedPercent,
          predictionFor(usage.id, primary.label.trim()),
        );

  return (
    <section className="primary-panel" aria-label={`${usage.name} primary quota`}>
      <header className="primary-head">
        <h2 className="primary-name">{usage.name}</h2>
        <span className="status-label" role="status">
          <span className={`status-dot status-${status.className}`} />
          <span>{status.label}</span>
        </span>
      </header>

      {primary === undefined || presentation.displayPercent === null ? (
        // An errored provider's own source note already says "No saved
        // provider data"; only a quiet provider needs the empty line here.
        usage.error ? null : <p className="primary-empty">No quota windows</p>
      ) : (
        <>
          <p className="primary-window">{primary.label.trim()}</p>
          <div className="primary-metric-row">
            {/* Neutral by design: severity is carried by the meter (and the
                attention rail), so a high quota never floods the panel red. */}
            <span className="primary-percent">{presentation.label}</span>
            {presentation.isLimitReached ? (
              <span className="primary-limit-reached">Limit reached</span>
            ) : null}
          </div>
          <div
            className="bar primary-bar"
            role="progressbar"
            aria-label={`${primary.label.trim()} quota · ${presentation.label}`}
            aria-valuenow={presentation.meterAriaValueNow ?? undefined}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuetext={presentation.meterAriaValueText}
          >
            <div
              className={`bar-fill tone-${tone}`}
              style={{
                width: `${Math.min(100, Math.max(0, presentation.meterPercent ?? 0))}%`,
              }}
            />
          </div>
          {resetLine ? <p className="primary-reset">{resetLine}</p> : null}
        </>
      )}

      {usage.error ? (
        <p className="provider-error" role="alert">
          {usage.error}
        </p>
      ) : null}
      {resetCredits ? (
        <div className="reset-credits" role="group" aria-label="Codex reset credits">
          <span>{resetCredits.bankLine}</span>
          <span>{resetCredits.applicabilityLine}</span>
        </div>
      ) : null}
      <p className="source-note primary-source">
        {providerSourceNote(usage, now)}
      </p>
      {usage.account?.note ? (
        <p className="account-note">{usage.account.note}</p>
      ) : null}
      {prediction ? (
        <PredictionSummary prediction={prediction} perspective={perspective} />
      ) : null}
    </section>
  );
}
