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
 * One small multiple: a single logical window's observed usage over the
 * query range. Native SVG, fixed 0–100 percent scale, shared time domain —
 * no chart dependency, no smoothing, and the line is mechanically broken at
 * reset boundaries and not-observed gaps by `trendGeometry`.
 *
 * Every chart carries a visible one-line summary plus a spoken equivalent;
 * reset boundaries are diamond markers (shape + tooltip + count in the
 * summary — never color alone); gaps render as hatched floor spans labeled
 * "Not observed" and are never zero-filled.
 *
 * The quota-perspective preference never touches the plot: the line, the
 * 0-100 scale, and the 80% near-limit guide stay canonical on the used
 * percentage. Only the summary headline and the spoken label follow the
 * perspective, and Remaining mode says the plot is used-complement.
 */
const CHART_WIDTH = 300;
const CHART_HEIGHT = 76;
const CHART_AXIS = 14;

function diamond(marker: { x: number; y: number }): string {
  return `M${marker.x.toFixed(2)} ${(marker.y - 3.2).toFixed(2)} L${(marker.x + 3.2).toFixed(2)} ${marker.y.toFixed(2)} L${marker.x.toFixed(2)} ${(marker.y + 3.2).toFixed(2)} L${(marker.x - 3.2).toFixed(2)} ${marker.y.toFixed(2)} Z`;
}

export function UsageTrendChart({
  series,
  analytics,
  compactLabels,
  perspective = "used",
}: {
  series: UsageTrendSeries;
  analytics: UsageAnalytics;
  compactLabels: boolean;
  perspective?: QuotaPerspective;
}) {
  const title = seriesTitle(series, shortProviderId);
  const summary = trendSummaryText(series, perspective);
  const geometry = trendGeometry(
    series,
    Date.parse(analytics.rangeStart),
    Date.parse(analytics.rangeEnd),
    CHART_WIDTH,
    CHART_HEIGHT,
  );

  const firstTick = series.points[0];
  const lastTick = series.points[series.points.length - 1];
  const lastTickX =
    lastTick === undefined
      ? 0
      : geometry.edgeTicks[geometry.edgeTicks.length - 1]?.x ?? 0;
  const firstTickX = geometry.edgeTicks[0]?.x ?? 0;
  const showBothTicks =
    !compactLabels && firstTick !== undefined && lastTickX - firstTickX > 70;

  return (
    <figure className="usage-trend-card">
      <figcaption className="usage-trend-title">{title}</figcaption>
      <svg
        viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT + CHART_AXIS}`}
        className="usage-chart"
        role="img"
        aria-label={
          perspective === "used"
            ? `${title} usage, percent used 0 to 100 over the ${analytics.range} range. ${summary}.`
            : `${title} usage, plotted as percent used 0 to 100 (remaining is the complement) over the ${analytics.range} range. ${summary}.`
        }
      >
        {/* Fixed 25% gridlines; the y scale is always 0–100. */}
        {[25, 50, 75].map((percent) => (
          <line
            key={percent}
            className="usage-chart-gridline"
            x1={0}
            x2={CHART_WIDTH}
            y1={(CHART_HEIGHT * (100 - percent)) / 100}
            y2={(CHART_HEIGHT * (100 - percent)) / 100}
          />
        ))}
        <line
          className="usage-chart-gridline baseline"
          x1={0}
          x2={CHART_WIDTH}
          y1={CHART_HEIGHT}
          y2={CHART_HEIGHT}
        />
        {/* Near-limit guide: dashed, labeled in the legend, never severity. */}
        <line
          className="usage-chart-guide"
          x1={0}
          x2={CHART_WIDTH}
          y1={geometry.guideY}
          y2={geometry.guideY}
        />
        {geometry.gapSpans.map((span, index) => (
          <rect
            key={index}
            className="usage-chart-gap"
            x={span.x}
            y={CHART_HEIGHT - 3}
            width={span.width}
            height={3}
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
            r={3}
          />
        ))}
        {geometry.resetMarkers.map((marker, index) => (
          <path key={index} className="usage-chart-reset" d={diamond(marker)}>
            <title>Reset boundary</title>
          </path>
        ))}
        {!compactLabels && firstTick !== undefined ? (
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
            x={CHART_WIDTH - 1}
            y={CHART_HEIGHT + CHART_AXIS - 3}
          >
            {formatTrendTick(lastTick.observedAt, analytics.range)}
          </text>
        ) : null}
      </svg>
      <p className="usage-trend-summary">{summary}</p>
    </figure>
  );
}
