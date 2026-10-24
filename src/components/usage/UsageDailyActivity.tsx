import type { UsageDailyDay } from "../../lib/usageIntelligence";
import {
  DAILY_ACTIVITY_LEVELS,
  dailyActivityLevel,
  dailyActivityMax,
  dailyCellAriaLabel,
} from "../../lib/usageDaily";
import { formatDayKey } from "../../lib/usagePresentation";

/**
 * Daily token activity — one cell per local calendar day over the selected
 * Today / 7d / 30d range, colored by relative reported tokens (scaled to
 * the busiest observed day in the range; see `lib/usageDaily.ts` for the
 * five-level normalization).
 *
 * Honesty, carried by the labels and the note below: the colors show
 * relative reported activity, never quota consumption or total account
 * usage; a hatched cell is "no recorded events — not confirmed zero
 * usage"; days before local collection started state that they can have
 * no data at all.
 */
export function UsageDailyActivity({ days }: { days: readonly UsageDailyDay[] }) {
  const maxTokens = dailyActivityMax(days);
  const first = days[0]?.date;
  const last = days[days.length - 1]?.date;

  return (
    <section className="usage-panel usage-daily" aria-label="Daily activity">
      <h3 className="panel-heading">Daily activity</h3>
      <div
        className="usage-daily-grid"
        role="list"
        aria-label="Daily reported token activity"
      >
        {days.map((day) => {
          const level = dailyActivityLevel(day, maxTokens);
          const label = dailyCellAriaLabel(day);
          return (
            <div
              key={day.date}
              role="listitem"
              className={`usage-daily-cell ${
                level === null ? "usage-daily-cell-none" : `usage-daily-l${level}`
              }`}
              aria-label={label}
              title={label}
            />
          );
        })}
      </div>
      <div className="usage-heatmap-legend">
        {DAILY_ACTIVITY_LEVELS.map((entry) => (
          <span key={entry.level} className="usage-legend-item">
            <span
              className={`usage-legend-swatch usage-daily-l${entry.level}`}
              aria-hidden="true"
            />
            {entry.label}
          </span>
        ))}
        <span className="usage-legend-item">
          <span
            className="usage-legend-swatch usage-daily-cell-none"
            aria-hidden="true"
          />
          no recorded events
        </span>
      </div>
      <p className="usage-daily-note">
        One cell per local day
        {first && last ? ` · ${formatDayKey(first)} – ${formatDayKey(last)}` : ""} ·
        colors scale to the busiest observed day in the range — relative
        reported activity, not quota consumption or total account usage
      </p>
      <p className="usage-daily-note">
        Hatched days have no recorded events, which is not confirmed zero
        usage; days before local collection started have no data at all.
      </p>
    </section>
  );
}
