import { useCallback, useRef, useState } from "react";
import { formatAge, formatTime } from "../lib/format";
import { FLOATING_QUOTA_PERSPECTIVE } from "../lib/floatingQuota";
import { floatingQuotaPresentation, quotaColorLevel } from "../lib/quotaPresentation";
import {
  grokBotEffectiveReading,
  grokBotRemainingPercent,
  grokBotStatusMessage,
  refreshGrokBotUsage,
  type GrokBotReading,
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
  const [lastSuccess, setLastSuccess] = useState<GrokBotReading | null>(null);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const resultRef = useRef(result);
  resultRef.current = result;

  const refresh = useCallback(() => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    const previous = resultRef.current;
    refreshGrokBotUsage(previous)
      .then((next) => {
        setResult(next);
        if (next.status === "ok" && typeof next.usedPercent === "number") {
          setLastSuccess({
            observedAt: next.observedAt,
            usedPercent: next.usedPercent,
            resetText: next.resetText,
            appVersion: next.appVersion,
          });
        }
      })
      .finally(() => {
        busyRef.current = false;
        setBusy(false);
      });
  }, []);

  const effective = grokBotEffectiveReading(result);
  const reading = effective.reading ?? lastSuccess;
  // The displayed value is the remaining quota derived from the verified
  // used percentage; when that derivation has nothing valid to work from,
  // the section shows no value rather than a fabricated 0%.
  const remaining = grokBotRemainingPercent(reading?.usedPercent);
  const presentation =
    remaining !== null && reading?.usedPercent !== undefined
      ? floatingQuotaPresentation(reading.usedPercent, FLOATING_QUOTA_PERSPECTIVE)
      : null;
  const message = grokBotStatusMessage(result);
  const stale = reading !== null && effective.source !== "fresh";

  return (
    <section className="fq-grokbot" aria-label="Grok Bot usage">
      <div className="fq-grokbot-head">
        <span className="fq-grokbot-label">Grok Bot</span>
        <button
          type="button"
          className="fq-grokbot-refresh"
          onClick={refresh}
          aria-disabled={busy ? true : undefined}
          aria-busy={busy ? true : undefined}
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
