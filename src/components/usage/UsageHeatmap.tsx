import type { UsageAnalytics } from "../../lib/usageAnalytics";
import {
  formatDayCell,
  formatDayKey,
  HEATMAP_BANDS,
  heatmapCellAriaLabel,
  heatmapCellValueText,
} from "../../lib/usagePresentation";
import type { QuotaPerspective } from "../../lib/quotaPresentation";

/**
 * Daily peak observed usage, exactly the accepted semantic: one cell per
 * local calendar day, colored by the backend's fixed absolute band
 * (0/25/50/75/100 — never a share of the user's own maximum). A day with no
 * observations is hatched, dash-marked, and labeled "not observed" — it is
 * never drawn as a 0% day, and the number in each observed cell keeps the
 * band from ever being the only signal.
 *
 * Bands and colors always track the canonical USED percentage (never a
 * reversed scale). The quota-perspective preference changes the printed
 * number and the spoken label only: Remaining mode shows the complement
 * ("16%" for an 84%-used day) and the spoken label names both sides.
 */
export function UsageHeatmap({
  analytics,
  perspective = "used",
}: {
  analytics: UsageAnalytics;
  perspective?: QuotaPerspective;
}) {
  return (
    <section className="usage-panel" aria-label="Daily peak usage">
      <h3 className="panel-heading">Daily peak</h3>
      <div
        className="usage-heatmap"
        role="list"
        aria-label="Daily peak observed usage"
      >
        {analytics.heatmap.map((day) => {
          const cell = formatDayCell(day.date);
          const observed = day.observed && day.peakUsedPercent !== undefined;
          return (
            <div
              key={day.date}
              role="listitem"
              className={`usage-day ${observed ? `usage-band-${day.band ?? 1}` : "usage-day-none"}`}
              aria-label={heatmapCellAriaLabel(day, perspective)}
              title={heatmapCellAriaLabel(day, perspective)}
            >
              <span className="usage-day-weekday" aria-hidden="true">
                {cell.weekday}
              </span>
              <span className="usage-day-number" aria-hidden="true">
                {cell.day}
              </span>
              <span className="usage-day-value" aria-hidden="true">
                {heatmapCellValueText(day, perspective)}
              </span>
            </div>
          );
        })}
      </div>
      <div className="usage-heatmap-legend">
        {HEATMAP_BANDS.map((band) => (
          <span key={band.band} className="usage-legend-item">
            <span
              className={`usage-legend-swatch usage-band-${band.band}`}
              aria-hidden="true"
            />
            {band.label}
          </span>
        ))}
        <span className="usage-legend-item">
          <span className="usage-legend-swatch usage-day-none" aria-hidden="true" />
          not observed
        </span>
      </div>
      <p className="usage-heatmap-note">
        Daily peak of observed {analytics.range} usage ·{" "}
        {formatDayKey(analytics.heatmap[0]?.date ?? "")} –{" "}
        {formatDayKey(analytics.heatmap[analytics.heatmap.length - 1]?.date ?? "")}
        {perspective === "remaining"
          ? " · labels show remaining; colors and bands still track used"
          : ""}
      </p>
    </section>
  );
}
