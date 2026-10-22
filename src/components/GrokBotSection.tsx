import { useCallback, useRef, useState } from "react";
import { formatAge, formatResetTime, formatTime } from "../lib/format";
import { FLOATING_QUOTA_PERSPECTIVE } from "../lib/floatingQuota";
import { floatingQuotaPresentation, quotaColorLevel } from "../lib/quotaPresentation";
import { grokBotLabel } from "../lib/grokBot";
import {
  grokBotEffectiveReading,
  grokBotRemainingPercent,
  grokBotStatusMessage,
  refreshGrokBotUsage,
  type GrokBotReading,
} from "../lib/grokBotManual";
import type { GrokBotUsage } from "../types";

/**
 * The Grok Bot block of the Grok detail card: below the quota windows, under
 * its own hairline, a secondary meter fed by the Grok Bot observation.
 *
 * Two sources feed this block, and they are the SAME pool seen twice:
 *
 * 1. The runtime's passive observation (the Cursor-served endpoint, polled
 *    by the ordinary Grok cycle). This is the same observation the main
 *    dashboard renders, so the floating card and the main app never
 *    disagree about Grok's usage. It arrives on the snapshot and needs no
 *    user action.
 * 2. The manual Windows accessibility read (the `Refresh` action). It is a
 *    second, independent way to observe the same Grok Bot Usage & Billing
 *    panel, useful when the passive observation is unavailable. The Refresh
 *    action here is that read's only trigger — nothing schedules it.
 *
 * The passive observation wins when both are present: it is the current
 * runtime state and needs no interaction. The manual read stays available as
 * the explicit fallback, and its own result never overwrites a live passive
 * reading.
 */
export function GrokBotSection({
  now,
  observation,
}: {
  now: Date;
  /**
   * The runtime's passive Grok Bot observation for this provider, when the
   * runtime collected one. Absent means the runtime has no observation —
   * the section then shows the manual read's result, or the guidance to
   * take one. Never a fabricated value.
   */
  observation?: GrokBotUsage;
}) {
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
  // The passive observation is the primary source: it is the current
  // runtime state, already current by construction (the backend strips stale
  // observations before the snapshot leaves the process).
  // The observation is used only when it carries a usable percentage — the
  // same contract `grokBotFor` applies on the main dashboard, so a malformed
  // payload can neither invent a value nor dominate the block. A rejected
  // observation falls back to the manual read, exactly as an absent one does.
  const passive =
    observation &&
    typeof observation.usedPercent === "number" &&
    Number.isFinite(observation.usedPercent)
      ? observation
      : null;
  const passivePercent = passive ? grokBotRemainingPercent(passive.usedPercent) : null;
  const passivePresentation =
    passive && passivePercent !== null
      ? floatingQuotaPresentation(passive.usedPercent, FLOATING_QUOTA_PERSPECTIVE)
      : null;
  // The manual read fills the block only while the runtime has no
  // observation of its own; it never displaces a live passive reading.
  // The displayed value is the remaining quota derived from the verified
  // used percentage; when that derivation has nothing valid to work from,
  // the section shows no value rather than a fabricated 0%.
  const remaining = grokBotRemainingPercent(reading?.usedPercent);
  const presentation =
    !passive && remaining !== null && reading?.usedPercent !== undefined
      ? floatingQuotaPresentation(reading.usedPercent, FLOATING_QUOTA_PERSPECTIVE)
      : null;
  const message = passive ? null : grokBotStatusMessage(result);
  const stale = reading !== null && effective.source !== "fresh";

  return (
    <section className="fq-grokbot" aria-label="Grok Bot usage">
      <div className="fq-grokbot-head">
        <span className="fq-grokbot-label">
          {passive ? grokBotLabel(passive) : "Grok Bot"}
        </span>
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

      {passive ? (
        <>
          <p className="fq-grokbot-line">
            {passivePresentation ? (
              <>
                <span className="fq-window-value">
                  {passivePresentation.displayPercentText}
                </span>
                <span> remaining</span>
              </>
            ) : null}
            {passive.resetAt ? (
              <span className="fq-grokbot-count">
                {" · Resets " + formatResetTime(passive.resetAt, now)}
              </span>
            ) : null}
          </p>
          {passivePresentation ? (
            <span
              className="fq-meter fq-meter-row"
              data-quota={quotaColorLevel(passivePresentation)}
              role="progressbar"
              aria-label={"Grok Bot weekly quota · " + passivePresentation.ariaLabel}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={passivePresentation.meterAriaValueNow ?? undefined}
              aria-valuetext={passivePresentation.meterAriaValueText}
            >
              <span
                className="fq-meter-fill"
                style={{ width: `${passivePresentation.meterPercent ?? 0}%` }}
              />
            </span>
          ) : null}
        </>
      ) : reading ? (
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

      {reading && !passive ? (
        <p className={"fq-grokbot-note" + (stale ? " is-stale" : "")}>
          {stale ? "Last known" : "Updated"} {formatTime(reading.observedAt)}
          {stale ? ` (${formatAge(reading.observedAt, now)})` : ""}
          {reading.appVersion ? ` · Grok Bot ${reading.appVersion}` : ""}
        </p>
      ) : null}
    </section>
  );
}
