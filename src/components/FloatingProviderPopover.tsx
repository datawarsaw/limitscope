import { useEffect, useRef } from "react";
import type { CSSProperties, FocusEvent } from "react";
import { formatCountdown } from "../lib/format";
import type { FloatingQuotaItem } from "../lib/floatingQuota";
import { FLOATING_QUOTA_PERSPECTIVE } from "../lib/floatingQuota";
import {
  floatingQuotaPresentation,
  quotaColorLevel,
} from "../lib/quotaPresentation";
import { ProviderMark } from "./FloatingProviderItem";
import { GrokBotSection } from "./GrokBotSection";

/**
 * The primary reset line follows the approved card: window label, "reset
 * in", countdown. Missing reset data stays visibly unavailable — never
 * invented.
 */
function primaryResetLine(
  item: FloatingQuotaItem,
  now: Date,
): { muted: string; strong: string | null } {
  const primary = item.primaryReset;
  if (primary) {
    const countdown = formatCountdown(primary.resetAt, now);
    if (countdown) {
      return { muted: primary.label + " reset in ", strong: countdown };
    }
    return { muted: primary.label + " · Reset time passed", strong: null };
  }
  if (!item.resetAt) return { muted: "No reset time", strong: null };
  const countdown = formatCountdown(item.resetAt, now);
  return countdown
    ? { muted: "Resets in ", strong: countdown }
    : { muted: "Reset time passed", strong: null };
}

/**
 * One window inventory row: label, remaining value with countdown, and a
 * thin semantic meter. Values stay neutral text; the meter carries the
 * quota color, matching the approved card.
 */
function WindowRow({
  item,
  label,
  usedPercent,
  resetAt,
  now,
}: {
  item: FloatingQuotaItem;
  label: string;
  usedPercent: number;
  resetAt: string | null;
  now: Date;
}) {
  const presentation = floatingQuotaPresentation(
    usedPercent,
    FLOATING_QUOTA_PERSPECTIVE,
  );
  const countdown = resetAt ? formatCountdown(resetAt, now) : null;
  return (
    <div className="fq-window-row" key={label}>
      <span className="fq-window-label">{label}</span>
      <div className="fq-window-body">
        <p className="fq-window-line">
          <span className="fq-window-value">
            {presentation.displayPercentText}
          </span>
          {presentation.displayPercent !== null ? (
            <span> remaining</span>
          ) : null}
          {countdown ? (
            <span className="fq-window-count">
              {" · resets in "}
              <span className="fq-window-strong">{countdown}</span>
            </span>
          ) : null}
        </p>
        <span
          className="fq-meter fq-meter-row"
          data-quota={quotaColorLevel(presentation)}
          role="progressbar"
          aria-label={`${item.name} ${label} quota · ${presentation.ariaLabel}`}
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
      </div>
    </div>
  );
}

/**
 * The one provider detail card. Hover previews and pinned clicks render
 * exactly this component — same layout, data, meters, reset line, and
 * account footer; only the pin (a held-open click) may move focus into it.
 */
export function FloatingProviderPopover({
  item,
  pinned,
  now,
  note,
  align,
  exiting,
  onMouseEnter,
  onMouseLeave,
  onFocus,
  onBlur,
  onOpenMain,
}: {
  item: FloatingQuotaItem;
  pinned: boolean;
  now: Date;
  note: string | null;
  align: number;
  exiting: boolean;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  onFocus: (event: FocusEvent<HTMLDivElement>) => void;
  onBlur: (event: FocusEvent<HTMLDivElement>) => void;
  onOpenMain: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!pinned) return;
    // Target the footer action specifically: the Grok Bot section adds its
    // own Refresh control above, and pinning must still land focus on Open.
    const button = ref.current?.querySelector(".fq-actions button");
    // preventScroll: the window is sized to the card, but until that resize
    // lands a focus-driven scroll would push the pill out of the viewport —
    // the bar must stay visible no matter how tall the card is.
    if (button instanceof HTMLButtonElement) button.focus({ preventScroll: true });
  }, [pinned, item.providerId]);

  const presentation = floatingQuotaPresentation(
    item.usedPercent ?? item.percent,
    FLOATING_QUOTA_PERSPECTIVE,
  );
  const reset = primaryResetLine(item, now);

  return (
    <div
      ref={ref}
      className={"fq-popover" + (exiting ? " is-exiting" : "")}
      data-tauri-drag-region="false"
      data-provider={item.providerId}
      data-quota={quotaColorLevel(presentation)}
      role="dialog"
      aria-label={item.name}
      style={{ "--fq-align": `${align}px` } as CSSProperties}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      onFocus={onFocus}
      onBlur={onBlur}
    >
      {/* Re-keyed per provider: switching segments remounts this body so the
          swap crossfades while the card itself glides to its new anchor. */}
      <div className="fq-card" key={item.providerId}>
        <header className="fq-popover-head">
          {/* The muted mark ties the card back to its collapsed segment; like
             on the bar, it carries identity only, never quota state. */}
          <span className="fq-popover-mark" aria-hidden="true">
            <ProviderMark id={item.providerId} />
          </span>
          <strong>{item.name}</strong>
          {/* The healthy "Live" chip is clutter; degraded statuses still badge. */}
          {item.statusClass !== "ok" ? (
            <span className={"fq-status status-" + item.statusClass}>
              {item.statusLabel}
            </span>
          ) : null}
        </header>

        <p className="fq-value">
          <span className="fq-value-num">{presentation.displayPercentText}</span>
          {presentation.displayPercent !== null ? (
            <span className="fq-value-unit">remaining</span>
          ) : null}
        </p>
        <p className="fq-primary-reset">
          {reset.strong ? (
            <>
              {reset.muted}
              <strong>{reset.strong}</strong>
            </>
          ) : (
            reset.muted
          )}
        </p>
        {presentation.displayPercent !== null ? (
          <span
            className="fq-meter fq-meter-primary"
            role="progressbar"
            aria-label={`${item.name} quota · ${presentation.ariaLabel}`}
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

        {item.windows.length > 0 ? (
          <div className="fq-windows">
            {item.windows.map((window) => (
              <WindowRow
                key={window.label}
                item={item}
                label={window.label}
                usedPercent={window.usedPercent}
                resetAt={window.resetAt}
                now={now}
              />
            ))}
          </div>
        ) : null}

        {/* Grok Bot's own manual read lives below a hairline, separate from
            the Grok Weekly quota above: different source, different meter,
            explicit Refresh only. */}
        {item.providerId === "grok" ? <GrokBotSection now={now} /> : null}

        {item.error ? (
          <p className="fq-popover-error" role="alert">
            {item.error}
          </p>
        ) : null}
        {note ? <p className="fq-popover-note">{note}</p> : null}

        {item.accountLabel ? (
          <footer className="fq-footer">
            <span className="fq-footer-icon" aria-hidden="true">
              <svg viewBox="0 0 24 24">
                <path
                  fill="currentColor"
                  d="M12 12a4.4 4.4 0 1 0-4.4-4.4A4.4 4.4 0 0 0 12 12Zm0 2.1c-3.9 0-7.4 2-7.4 4.6v.9h14.8v-.9c0-2.6-3.5-4.6-7.4-4.6Z"
                />
              </svg>
            </span>
            <p className="fq-popover-account">account: {item.accountLabel}</p>
          </footer>
        ) : null}

        {/* Informational plus one Open action. Pin and hide commands live
            only in the right-click menu; the single sanctioned in-card
            command is the Grok Bot section's manual Refresh. */}
        <div className="fq-actions">
          <button type="button" onClick={onOpenMain}>
            Open
          </button>
        </div>
      </div>
    </div>
  );
}
