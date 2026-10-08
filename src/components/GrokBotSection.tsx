import { useCallback, useState } from "react";
import { formatAge, formatTime } from "../lib/format";
import { FLOATING_QUOTA_PERSPECTIVE } from "../lib/floatingQuota";
import { floatingQuotaPresentation, quotaColorLevel } from "../lib/quotaPresentation";
import {
  grokBotEffectiveReading,
  grokBotRemainingPercent,
  grokBotStatusMessage,
  refreshGrokBotUsage,
} from "../lib/grokBotManual";

/**
 * The Grok Bot block of the Grok detail card: below the quota windows, under
 * its own hairline, a secondary meter fed by the manual accessibility read.
 * The Refresh action here is the read's only trigger — nothing schedules it.
 */
export function GrokBotSection({ now }: { now: Date }) {
  const [result, setResult] = useState<Awaited<ReturnType<typeof refreshGrokBotUsage>> | null>(
    null,
  );
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    if (busy) return;
    setBusy(true);
    refreshGrokBotUsage()
      .then(setResult)
      .finally(() => setBusy(false));
  }, [busy]);

  const effective = grokBotEffectiveReading(result);
  const reading = effective.reading;
  // The displayed value is the remaining quota derived from the verified
  // used percentage; when that derivation has nothing valid to work from,
  // the section shows no value rather than a fabricated 0%.
  const remaining = grokBotRemainingPercent(reading?.usedPercent);
  const presentation =
    remaining !== null && reading?.usedPercent !== undefined
      ? floatingQuotaPresentation(reading.usedPercent, FLOATING_QUOTA_PERSPECTIVE)
      : null;
  const message = grokBotStatusMessage(result);
  const stale = effective.source === "last-known";

  return (
    <section className="fq-grokbot" aria-label="Grok Bot usage">
      <div className="fq-grokbot-head">
        <span className="fq-grokbot-label">Grok Bot</span>
        <button
          type="button"
          className="fq-grokbot-refresh"
          onClick={refresh}
          disabled={busy}
          aria-label="Refresh Grok Bot usage"
        >
          {busy ? "Refreshing…" : "Refresh"}
        </button>
      </div>

      {reading ? (
        <>
          <p className="fq-grokbot-line">
            {presentation ? (
              <>
                <span className="fq-window-value">
                  {presentation.displayPercentText}
                </span>
                <span> remaining</span>
              </>
            ) : null}
            {reading.resetText ? (
              <span className="fq-grokbot-count">{" · " + reading.resetText}</span>
            ) : null}
          </p>
          {presentation ? (
            <span
              className="fq-meter fq-meter-row"
              data-quota={quotaColorLevel(presentation)}
              role="progressbar"
              aria-label={"Grok Bot weekly quota · " + presentation.ariaLabel}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={presentation.meterAriaValueNow ?? undefined}
              aria-valuetext={presentation.meterAriaValueText}
            >
              <span
                className="fq-meter-fill"
                style={{ width: `${presentation.meterPercent ?? 0}%` }}
              />
            </span>
          ) : null}
        </>
      ) : null}

      {message ? <p className="fq-grokbot-status">{message}</p> : null}

      {reading ? (
        <p className={"fq-grokbot-note" + (stale ? " is-stale" : "")}>
          {stale ? "Last known" : "Updated"} {formatTime(reading.observedAt)}
          {stale ? ` (${formatAge(reading.observedAt, now)})` : ""}
          {reading.appVersion ? ` · Grok Bot ${reading.appVersion}` : ""}
        </p>
      ) : null}
    </section>
  );
}
