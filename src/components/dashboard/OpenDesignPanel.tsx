import { useCallback, useRef, useState } from "react";
import { formatAge, formatTime } from "../../lib/format";
import {
  openDesignEffectiveReading,
  openDesignStatusMessage,
  openDesignWindow,
  refreshOpenDesignUsage,
  type OpenDesignRefreshResult,
  type OpenDesignWindowLabel,
} from "../../lib/openDesignManual";

const WINDOW_LABELS: readonly OpenDesignWindowLabel[] = ["5-hour", "7-day"];

/**
 * Two decimals at most, and no trailing zeros: the page reports a percentage
 * that may be whole ("91% left") or fractional (90.85). The value shown is
 * always the remaining share the page itself states.
 */
function formatRemaining(percent: number): string {
  return String(Math.round(percent * 100) / 100) + "%";
}

/** Remaining-share severity: only the meter fill uses it. */
function remainingTone(percent: number): "calm" | "warn" | "critical" {
  if (percent <= 15) return "critical";
  if (percent <= 35) return "warn";
  return "calm";
}

function clampPercent(percent: number): number {
  return Math.max(0, Math.min(100, percent));
}

/**
 * OpenDesign Cloud detail card: the Design Plan's 5-hour and 7-day remaining
 * shares, read from Chrome's accessibility tree by an explicit Refresh.
 *
 * Nothing here schedules the read: the command runs only from the button.
 * The percentages are shown as the page states them (remaining), each kept
 * with its own reset line. The pay-as-you-go wallet is a separate balance
 * and never appears here or feeds these meters.
 */
export function OpenDesignPanel({ now }: { now: Date }) {
  const [result, setResult] = useState<OpenDesignRefreshResult | null>(null);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const resultRef = useRef(result);
  resultRef.current = result;

  const refresh = useCallback(() => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    const previous = resultRef.current;
    refreshOpenDesignUsage(previous)
      .then((next) => setResult(next))
      .finally(() => {
        busyRef.current = false;
        setBusy(false);
      });
  }, []);

  const effective = openDesignEffectiveReading(result);
  const reading = effective.reading;
  const message = openDesignStatusMessage(result);
  const stale = reading !== null && effective.source !== "fresh";
  const plan = reading?.plan ?? "Go";

  return (
    <section className="opendesign-panel" aria-label="OpenDesign usage">
      <div className="opendesign-head">
        <h3 className="panel-heading opendesign-heading">OpenDesign</h3>
        <button
          type="button"
          className="opendesign-refresh"
          onClick={refresh}
          aria-disabled={busy ? true : undefined}
          aria-busy={busy ? true : undefined}
          aria-label="Refresh OpenDesign usage"
        >
          {busy ? "Refreshing…" : "Refresh"}
        </button>
      </div>

      <p className="opendesign-plan">
        {"OpenDesign " + plan}
        {reading?.periodEndsText ? (
          <span className="opendesign-period">
            {" · " + reading.periodEndsText}
          </span>
        ) : null}
      </p>

      {reading ? (
        <div className="opendesign-windows">
          {WINDOW_LABELS.map((label) => {
            const window = openDesignWindow(reading, label);
            if (!window) return null;
            const text = formatRemaining(window.remainingPercent);
            return (
              <div
                key={label}
                className="opendesign-window"
                role="group"
                aria-label={"OpenDesign " + label + " limit"}
              >
                <div className="opendesign-window-head">
                  <span className="opendesign-window-label">
                    {label + " limit"}
                  </span>
                  <span className="opendesign-window-value">
                    {text}
                    <span className="opendesign-window-unit"> remaining</span>
                  </span>
                </div>
                <span
                  className="opendesign-meter"
                  role="progressbar"
                  aria-label={label + " limit remaining"}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={Math.round(clampPercent(window.remainingPercent) * 100) / 100}
                  aria-valuetext={text + " remaining"}
                >
                  <span
                    className={
                      "opendesign-meter-fill tone-" +
                      remainingTone(window.remainingPercent)
                    }
                    style={{
                      width: clampPercent(window.remainingPercent) + "%",
                    }}
                  />
                </span>
                {window.resetText ? (
                  <span className="opendesign-reset">{window.resetText}</span>
                ) : null}
              </div>
            );
          })}
        </div>
      ) : null}

      {message ? <p className="opendesign-status">{message}</p> : null}

      {reading ? (
        <p className={"opendesign-note" + (stale ? " is-stale" : "")}>
          {stale ? "Last known" : "Updated"} {formatTime(reading.observedAt)}
          {stale ? " (" + formatAge(reading.observedAt, now) + ")" : ""}
        </p>
      ) : null}
    </section>
  );
}
