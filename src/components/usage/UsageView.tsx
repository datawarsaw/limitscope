import { useMemo, useState } from "react";
import {
  loadUsageAnalytics,
  type UsageAnalyticsLoader,
  type UsageAnalyticsQuery,
  type UsageAnalyticsRange,
} from "../../lib/usageAnalytics";
import { useUsageAnalytics } from "../../hooks/useUsageAnalytics";
import { useDashboardLayout } from "../../hooks/useDashboardLayout";
import { hasAnyObservation, visibleTrendSeries } from "../../lib/usagePresentation";
import { shortProviderId, shortProviderName, usableQuotaWindows } from "../../lib/dashboard";
import type { QuotaPerspective } from "../../lib/quotaPresentation";
import { UsageSummaryStrip } from "./UsageSummaryStrip";
import { UsageTrendChart } from "./UsageTrendChart";
import { UsageHeatmap } from "./UsageHeatmap";
import type { ProviderUsage } from "../../types";

/**
 * The v0.7 Usage view: one coherent analytics surface over the backend
 * `get_usage_analytics` projection — filters, the five-metric summary strip,
 * small-multiple trends, and the daily-peak heatmap.
 *
 * Scope of honesty, enforced everywhere below: the backend response is the
 * only source of numbers; missing observations are "not observed", never 0;
 * lower-bound peaks, lower-bound reset counts, and estimated near-limit time
 * always carry their qualifier as text; and a failed analytics query dies
 * here, never on the live provider cards.
 */
export function UsageView({
  usages,
  scopeProviderId,
  onScopeProvider,
  historyRevision,
  perspective = "used",
  loader = loadUsageAnalytics,
}: {
  usages: readonly ProviderUsage[];
  /** Provider the view is scoped to; null means all providers. */
  scopeProviderId: string | null;
  onScopeProvider: (providerId: string | null) => void;
  historyRevision: number | null;
  /**
   * Quota presentation perspective (v0.6). Shapes labels only — the
   * analytics queries, severity, near-limit thresholds, and heatmap bands
   * stay canonical on the used percentage.
   */
  perspective?: QuotaPerspective;
  /** Injected analytics source (tests, dev fixtures); production default. */
  loader?: UsageAnalyticsLoader;
}) {
  const [range, setRange] = useState<UsageAnalyticsRange>("24h");
  const [windowLabel, setWindowLabel] = useState<string | null>(null);
  const layout = useDashboardLayout();

  const query = useMemo<UsageAnalyticsQuery>(
    () => ({
      range,
      ...(scopeProviderId === null ? {} : { providerId: scopeProviderId }),
      ...(windowLabel === null ? {} : { windowLabel }),
    }),
    [range, scopeProviderId, windowLabel],
  );

  const analytics = useUsageAnalytics(query, true, historyRevision, loader);

  // Window filter options come from the runtime's own quota windows — the
  // same labels history records — scoped to the current provider when one is.
  const windowOptions = useMemo(() => {
    const labels = new Set<string>();
    for (const usage of usages) {
      if (scopeProviderId !== null && usage.id !== scopeProviderId) continue;
      for (const window of usableQuotaWindows(usage)) {
        const label = window.label.trim();
        if (label) labels.add(label);
      }
    }
    return [...labels].sort((a, b) => a.localeCompare(b));
  }, [usages, scopeProviderId]);

  const scopedUsage = usages.find((usage) => usage.id === scopeProviderId);
  const scopedName =
    scopeProviderId !== null
      ? scopedUsage
        ? shortProviderName(scopedUsage)
        : shortProviderId(scopeProviderId)
      : null;

  const data = analytics.data;
  const empty = data !== null && !hasAnyObservation(data);
  const trends = data ? visibleTrendSeries(data.trends) : null;

  const gapCount =
    data?.trends.reduce((total, series) => total + series.coverage.gaps.length, 0) ?? 0;

  return (
    <div className="usage-view" aria-label="Usage analytics">
      <div className="usage-toolbar">
        <button
          type="button"
          className="usage-scope-all"
          aria-pressed={scopeProviderId === null}
          onClick={() => onScopeProvider(null)}
        >
          All providers
        </button>
        <div className="usage-range" role="group" aria-label="Range">
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
        <label className="usage-window-filter">
          <span className="usage-toolbar-label">Window</span>
          <select
            value={windowLabel ?? ""}
            onChange={(event) =>
              setWindowLabel(event.target.value === "" ? null : event.target.value)
            }
          >
            <option value="">All windows</option>
            {windowOptions.map((label) => (
              <option key={label} value={label}>
                {label}
              </option>
            ))}
          </select>
        </label>
        {analytics.refreshing ? (
          <span className="usage-refreshing" role="status">
            Updating…
          </span>
        ) : null}
      </div>

      {analytics.error && data === null ? (
        <div className="usage-panel usage-error" role="alert">
          <p>Usage history could not be loaded. {analytics.error}</p>
          <p className="usage-error-note">
            Live provider cards are unaffected — only this analytics view
            failed.
          </p>
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
          Loading usage…
        </div>
      ) : null}

      {data !== null && empty ? (
        <div className="usage-panel usage-empty">
          <p>
            {scopedName
              ? `No usage history yet for ${scopedName}.`
              : "No usage history yet."}
          </p>
          <p className="usage-empty-note">
            LimitScope will build this view as quota observations are recorded.
          </p>
        </div>
      ) : null}

      {data !== null && !empty ? (
        <>
          {analytics.error ? (
            <p className="usage-refresh-error" role="alert">
              Could not refresh usage analytics: {analytics.error}
            </p>
          ) : null}
          <UsageSummaryStrip analytics={data} perspective={perspective} />

          <section className="usage-panel" aria-label="Usage trend">
            <h3 className="panel-heading">Usage trend</h3>
            {trends && trends.visible.length > 0 ? (
              <div className="usage-trend-grid">
                {trends.visible.map((series) => (
                  <UsageTrendChart
                    key={`${series.providerId}|${series.account ?? ""}|${series.windowLabel}`}
                    series={series}
                    analytics={data}
                    compactLabels={layout === "narrow"}
                    perspective={perspective}
                  />
                ))}
              </div>
            ) : (
              <p className="usage-trend-empty">
                No observations for this selection in the {data.range} range.
              </p>
            )}
            {trends && trends.hiddenCount > 0 ? (
              <p className="usage-trend-more">
                …and {trends.hiddenCount} more windows — pick a provider to
                focus the trend.
              </p>
            ) : null}
            <p className="usage-chart-legend">
              dashed guide = 80% near-limit threshold · ◇ reset boundary ·
              hatched floor = not observed · line breaks at resets and gaps
            </p>
          </section>

          <UsageHeatmap analytics={data} perspective={perspective} />

          <p className="usage-coverage">
            Observed {data.summary.observedDays.count} of the last{" "}
            {data.heatmap.length} days
            {gapCount > 0
              ? ` · ${gapCount} ${gapCount === 1 ? "span" : "spans"} not observed`
              : ""}
            {" · "}gaps are missing observations, never zero usage
          </p>
        </>
      ) : null}
    </div>
  );
}
