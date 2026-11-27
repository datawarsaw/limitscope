import { useMemo, useState } from "react";
import {
  codexFenceScope,
  loadUsageAnalytics,
  type CodexScope,
  type UsageAnalyticsLoader,
  type UsageAnalyticsRange,
} from "../../lib/usageAnalytics";
import { useUsageAnalytics } from "../../hooks/useUsageAnalytics";
import { hasAnyObservation } from "../../lib/usagePresentation";
import { shortProviderName } from "../../lib/dashboard";
import { quotaHistoryPresentation, type QuotaPerspective } from "../../lib/quotaPresentation";
import type { ProviderUsage } from "../../types";
import { isCodexProviderId } from "../../lib/providerIds";
import { NEAR_LIMIT_PERCENT } from "../../lib/thresholds";
import { LargeTrendChart } from "../charts/LargeTrendChart";

/**
 * The Status view's hero: a prominent, readable history chart for the
 * selected provider, straight from the same backend analytics projection the
 * Insights view uses. One large chart per quota window; the same honesty
 * contract applies (missing observations stay gaps, resets stay boundaries,
 * nothing is zero-filled or aggregated).
 *
 * Insufficient data is a first-class state, not a broken one: a provider with
 * no recorded observations yet says so calmly, and a failed analytics query
 * never touches the live provider status above it.
 */
const MAX_WINDOW_CHARTS = 4;

type StatusHistoryChartProps = {
  usage: ProviderUsage;
  historyRevision: number | null;
  perspective?: QuotaPerspective;
  loader?: UsageAnalyticsLoader;
  codexIdentityStates?: CodexScope;
};

export function StatusHistoryChart(props: StatusHistoryChartProps) {
  // A different provider must never display the previous provider's payload
  // while its own query is pending.
  return <ProviderHistoryChart key={props.usage.id} {...props} />;
}

function ProviderHistoryChart({
  usage,
  historyRevision,
  perspective = "remaining",
  loader = loadUsageAnalytics,
  codexIdentityStates,
}: StatusHistoryChartProps) {
  const [range, setRange] = useState<UsageAnalyticsRange>("24h");

  const query = useMemo(
    () => ({ range, providerId: usage.id }),
    [range, usage.id],
  );
  // The selected provider's Codex identity scope (empty for non-Codex
  // providers, whose analytics are not session-fenced). The runtime's
  // per-slot observation is authoritative; a slot it has not observed yet
  // still fences to the visible account identity (generation 0) — never to
  // "no slot expected", which would accept a prior session's projection.
  const identityScope: CodexScope = isCodexProviderId(usage.id)
    ? codexFenceScope(
        [usage],
        codexIdentityStates?.filter((state) => state.providerId === usage.id),
      )
    : [];
  const analytics = useUsageAnalytics(
    query,
    true,
    historyRevision,
    loader,
    identityScope,
  );

  const data = analytics.data;
  const empty = data !== null && !hasAnyObservation(data);
  const trends = data?.trends ?? [];
  const hiddenWindowCount = Math.max(0, trends.length - MAX_WINDOW_CHARTS);
  const providerName = shortProviderName(usage);

  return (
    <section className="status-history" aria-label="Quota history">
      <div className="status-history-head">
        <h3 className="panel-heading">History</h3>
        <div className="usage-range" role="group" aria-label="History range">
          <button
            type="button"
            aria-pressed={range === "24h"}
            onClick={() => setRange("24h")}
          >
            24h
          </button>
          <button
            type="button"
            aria-pressed={range === "7d"}
            onClick={() => setRange("7d")}
          >
            7d
          </button>
        </div>
      </div>

      {analytics.error && data === null ? (
        <div className="usage-panel usage-error" role="alert">
          <p>Usage history could not be loaded. {analytics.error}</p>
          <button
            type="button"
            className="usage-retry"
            onClick={() => analytics.retry()}
          >
            Retry
          </button>
        </div>
      ) : null}

      {analytics.loading && data === null ? (
        <div className="usage-loading" role="status">
          Loading history…
        </div>
      ) : null}

      {data !== null && empty ? (
        <div className="usage-panel usage-empty">
          <p>No usage history yet for {providerName}.</p>
          <p className="usage-empty-note">
            The chart appears as quota observations are recorded.
          </p>
        </div>
      ) : null}

      {data !== null && !empty ? (
        <>
          {analytics.error ? (
            <p className="usage-refresh-error" role="alert">
              Could not refresh history: {analytics.error}
            </p>
          ) : null}
          {analytics.refreshing ? (
            <span className="usage-refreshing" role="status">
              Updating…
            </span>
          ) : null}
          <div className="status-history-charts">
            {trends.slice(0, MAX_WINDOW_CHARTS).map((series) => (
              <LargeTrendChart
                key={`${series.providerId}|${series.account ?? ""}|${series.windowLabel}`}
                series={series}
                analytics={data}
                perspective={perspective}
                plotPerspective={perspective}
              />
            ))}
          </div>
          {hiddenWindowCount > 0 ? (
            <p className="usage-trend-more">
              …and {hiddenWindowCount} more{" "}
              {hiddenWindowCount === 1 ? "window" : "windows"} — see Insights
              for the full history.
            </p>
          ) : null}
          <p className="usage-chart-legend">
            dashed guide = {quotaHistoryPresentation(NEAR_LIMIT_PERCENT, perspective).label} near-limit threshold · ◇ reset boundary ·
            hatched floor = not observed · line breaks at resets and gaps
          </p>
        </>
      ) : null}
    </section>
  );
}
