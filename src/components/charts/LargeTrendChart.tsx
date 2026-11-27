import type { UsageAnalytics, UsageTrendSeries } from "../../lib/usageAnalytics";
import {
  formatTrendTick,
  seriesTitle,
  trendGeometry,
  trendSummaryText,
} from "../../lib/usagePresentation";
import { shortProviderId } from "../../lib/dashboard";
import type { QuotaPerspective } from "../../lib/quotaPresentation";

/**
 * The redesign's hero chart: one logical window's observed usage over the
 * query range, drawn large and readable. Same honesty contract as the small
 * multiples in `UsageTrendChart` — the segmentation, gap hatching, reset
 * diamonds, fixed 0–100 scale, and the shared time domain all come from the
 * same `trendGeometry`/`trendShape` source, so the two chart sizes can never
 * disagree about the data. The only differences are size, a labeled y-axis,
 * and a thicker line.
 *
 * Status plots remaining capacity. Insights retains the consumption plot.
 */
const CHART_WIDTH = 640;
const CHART_HEIGHT = 176;
const CHART_AXIS = 16;
const Y_AXIS = 36;

function diamond(marker: { x: number; y: number }): string {
  const r = 4;
  return `M${marker.x.toFixed(2)} ${(marker.y - r).toFixed(2)} L${(marker.x + r).toFixed(2)} ${marker.y.toFixed(2)} L${marker.x.toFixed(2)} ${(marker.y + r).toFixed(2)} L${(marker.x - r).toFixed(2)} ${marker.y.toFixed(2)} Z`;
}

export function LargeTrendChart({
  series,
  analytics,
  perspective = "used",
  plotPerspective = "used",
}: {
  series: UsageTrendSeries;
  analytics: UsageAnalytics;
  perspective?: QuotaPerspective;
  plotPerspective?: QuotaPerspective;
}) {
  const title = seriesTitle(series, shortProviderId);
  const summary = trendSummaryText(series, perspective);
  const plotWidth = CHART_WIDTH - Y_AXIS;
  const geometry = trendGeometry(
    series,
    Date.parse(analytics.rangeStart),
    Date.parse(analytics.rangeEnd),
    plotWidth,
    CHART_HEIGHT,
    plotPerspective,
  );

  const points = series.points;
  const firstTick = points[0];
  const lastTick = points[points.length - 1];
  const lastTickX =
    lastTick === undefined
      ? 0
      : geometry.edgeTicks[geometry.edgeTicks.length - 1]?.x ?? 0;
  const firstTickX = geometry.edgeTicks[0]?.x ?? 0;
  const showBothTicks = firstTick !== undefined && lastTickX - firstTickX > 90;

  return (
    <figure className="large-trend-chart">
      <figcaption className="large-trend-title">{title}</figcaption>
      <svg
        viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT + CHART_AXIS}`}
        className="usage-chart large-chart"
        role="img"
        aria-label={`${title} quota, percent ${plotPerspective} 0 to 100 over the ${analytics.range} range. ${summary}.`}
      >
        {[100, 50, 0].map((percent) => (
          <text
            key={percent}
            className="usage-chart-axis-label"
            x={Y_AXIS - 9}
            y={(CHART_HEIGHT * (100 - percent)) / 100 + 4}
          >
            {percent}
          </text>
        ))}
        <g transform={`translate(${Y_AXIS} 0)`}>
          {[25, 50, 75].map((percent) => (
            <line
              key={percent}
              className="usage-chart-gridline"
              x1={0}
              x2={plotWidth}
              y1={(CHART_HEIGHT * (100 - percent)) / 100}
              y2={(CHART_HEIGHT * (100 - percent)) / 100}
            />
          ))}
          <line
            className="usage-chart-gridline baseline"
            x1={0}
            x2={plotWidth}
            y1={CHART_HEIGHT}
            y2={CHART_HEIGHT}
          />
          {/* Near-limit guide: dashed, labeled in the legend, never severity. */}
          <line
            className="usage-chart-guide"
            x1={0}
            x2={plotWidth}
            y1={geometry.guideY}
            y2={geometry.guideY}
          />
          {geometry.gapSpans.map((span, index) => (
            <rect
              key={index}
              className="usage-chart-gap"
              x={span.x}
              y={CHART_HEIGHT - 4}
              width={span.width}
              height={4}
            >
              <title>Not observed</title>
            </rect>
          ))}
          {geometry.paths.map((path, index) => (
            <path key={index} className="usage-chart-line" d={path} fill="none" />
          ))}
          {geometry.singleDots.map((dot, index) => (
            <circle
              key={index}
              className="usage-chart-dot"
              cx={dot.x}
              cy={dot.y}
              r={3.5}
            />
          ))}
          {geometry.resetMarkers.map((marker, index) => (
            <path key={index} className="usage-chart-reset" d={diamond(marker)}>
              <title>Reset boundary</title>
            </path>
          ))}
          {firstTick !== undefined ? (
            <text
              className="usage-chart-tick"
              x={1}
              y={CHART_HEIGHT + CHART_AXIS - 3}
            >
              {formatTrendTick(firstTick.observedAt, analytics.range)}
            </text>
          ) : null}
          {showBothTicks && lastTick !== undefined ? (
            <text
              className="usage-chart-tick end"
              x={plotWidth - 1}
              y={CHART_HEIGHT + CHART_AXIS - 3}
            >
              {formatTrendTick(lastTick.observedAt, analytics.range)}
            </text>
          ) : null}
        </g>
      </svg>
      <p className="usage-trend-summary">{summary}</p>
    </figure>
  );
}
