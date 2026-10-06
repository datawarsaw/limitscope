import { useMemo, useState } from "react";
import {
  cachedTokens,
  formatTokens,
  loadUsageIntelligence,
  localMidnightMs,
  usageProviderLabel,
  usageSourceStateText,
  type UsageIntelligenceLoader,
  type UsageIntelligenceRange,
} from "../../lib/usageIntelligence";
import { useUsageIntelligence } from "../../hooks/useUsageIntelligence";

/**
 * Usage Intelligence (v0.8.9): the minimal Today / 7d / 30d view over
 * reported token usage collected locally, grouped by provider and model.
 *
 * Honesty rules, enforced everywhere below:
 * - these are reported tokens collected locally — never account quota,
 *   billing usage, exact spend, or complete history;
 * - while the opt-in toggle is off, the section explains and never
 *   queries (mirroring the backend's no-probe-while-disabled contract);
 * - an empty view states when local collection started rather than
 *   implying the range was measured and empty;
 * - a range reaching further back than collection itself discloses that
 *   ("Collected locally since …", plus the incomplete qualifier);
 * - only sources with collected data get provider groups; there are no
 *   empty Codex / OpenCode placeholders.
 *
 * Optimized for fast comparison: one dense table, compact K/M/B numbers,
 * no decorative charts.
 */
export function TokenUsageSection({
  enabled,
  revision,
  scopeProviderId = null,
  loader = loadUsageIntelligence,
  onOpenSettings,
}: {
  /** The persisted Usage Intelligence opt-in (settings). */
  enabled: boolean;
  /** Store revision from the runtime snapshot; drives per-pass refresh. */
  revision: number | null;
  /** Provider the Usage view is scoped to; null means all providers. */
  scopeProviderId?: string | null;
  /** Injected source (tests, dev fixtures); production default. */
  loader?: UsageIntelligenceLoader;
  /** Opens the settings drawer (the "off" explainer's action). */
  onOpenSettings?: () => void;
}) {
  const [range, setRange] = useState<UsageIntelligenceRange>("today");

  const query = useMemo(
    () => ({
      range,
      ...(range === "today" ? { todayStartMs: localMidnightMs() } : {}),
    }),
    [range],
  );

  const intelligence = useUsageIntelligence(query, enabled, revision, loader);
  const data = intelligence.data;

  const groups = useMemo(() => {
    if (!data) return [];
    if (scopeProviderId === null) return data.groups;
    return data.groups.filter((group) => group.provider === scopeProviderId);
  }, [data, scopeProviderId]);

  const rangeLabel = range === "today" ? "Today" : range === "7d" ? "7 days" : "30 days";

  if (!enabled) {
    return (
      <section className="usage-panel token-usage" aria-label="Usage Intelligence">
        <h3 className="panel-heading">Token usage</h3>
        <p className="token-usage-off-note">
          Reported token usage collected locally is off.
        </p>
        <p className="token-usage-off-copy">
          When enabled, LimitScope reads local AI tool usage metadata —
          models and token counts — from supported tools like ZCode, Codex,
          and OpenCode, on this device only. It never reads prompt or response
          content, and nothing is uploaded.
        </p>
        {onOpenSettings ? (
          <button
            type="button"
            className="token-usage-enable-btn"
            onClick={onOpenSettings}
          >
            Enable in Settings
          </button>
        ) : null}
      </section>
    );
  }

  return (
    <section className="usage-panel token-usage" aria-label="Usage Intelligence">
      <div className="token-usage-head">
        <h3 className="panel-heading">Token usage</h3>
        <div className="usage-range" role="group" aria-label="Token usage range">
          <button
            type="button"
            aria-pressed={range === "today"}
            onClick={() => setRange("today")}
          >
            Today
          </button>
          <button
            type="button"
            aria-pressed={range === "7d"}
            onClick={() => setRange("7d")}
          >
            7d
          </button>
          <button
            type="button"
            aria-pressed={range === "30d"}
            onClick={() => setRange("30d")}
          >
            30d
          </button>
        </div>
      </div>
      <p className="token-usage-subtitle">
        Reported token usage collected locally{data?.collectionStartedAt
          ? ` since ${formatSinceDate(data.collectionStartedAt)}`
          : ""}
        {data?.incompleteHistory
          ? ` — less history than the ${rangeLabel} range`
          : ""}
      </p>

      {intelligence.error && data === null ? (
        <div className="token-usage-error" role="alert">
          <p>Token usage could not be loaded. {intelligence.error}</p>
          <button type="button" className="usage-retry" onClick={() => intelligence.retry()}>
            Retry
          </button>
        </div>
      ) : null}

      {intelligence.loading && data === null ? (
        <div className="token-usage-loading" role="status">
          Loading token usage…
        </div>
      ) : null}

      {data !== null && groups.length === 0 ? (
        <div className="token-usage-empty">
          <p>
            {scopeProviderId !== null
              ? "No locally collected token usage for this provider yet."
              : "No locally collected token usage yet."}
          </p>
          <p className="token-usage-empty-note">
            {data.collectionStartedAt
              ? `Collection started ${formatSinceDate(data.collectionStartedAt)}; new requests appear here as they complete.`
              : "Collection starts with the next completed request; history from before the feature was enabled is never imported."}
          </p>
        </div>
      ) : null}

      {groups.length > 0 && data !== null ? (
        <>
          {intelligence.error ? (
            <p className="usage-refresh-error" role="alert">
              Could not refresh token usage: {intelligence.error}
            </p>
          ) : null}
          <div className="token-usage-table" role="table" aria-label={`Token usage by provider and model (${rangeLabel})`}>
            <div className="token-usage-row token-usage-header" role="row">
              <span className="token-usage-model" role="columnheader">Model</span>
              <span className="token-usage-num" role="columnheader">Total</span>
              <span className="token-usage-num" role="columnheader">Input</span>
              <span className="token-usage-num" role="columnheader">Cached</span>
              <span className="token-usage-num" role="columnheader">Output</span>
              <span className="token-usage-num" role="columnheader">Reasoning</span>
            </div>
            {groups.map((group) => (
              <div key={group.provider} className="token-usage-provider" role="rowgroup">
                <div className="token-usage-provider-row" role="row">
                  <span className="token-usage-provider-name" role="rowheader">
                    {usageProviderLabel(group.provider)}
                  </span>
                  <span className="token-usage-num token-usage-provider-total" role="cell">
                    {formatTokens(group.totalTokens)}
                  </span>
                  <span className="token-usage-num" role="cell" />
                  <span className="token-usage-num" role="cell" />
                  <span className="token-usage-num" role="cell" />
                  <span className="token-usage-num" role="cell" />
                </div>
                {group.models.map((model) => (
                  <div className="token-usage-row" role="row" key={model.model}>
                    <span className="token-usage-model" role="rowheader">{model.model}</span>
                    <span className="token-usage-num" role="cell">{formatTokens(model.totalTokens)}</span>
                    <span className="token-usage-num" role="cell">{formatTokens(model.inputTokens)}</span>
                    <span className="token-usage-num" role="cell">{formatTokens(cachedTokens(model))}</span>
                    <span className="token-usage-num" role="cell">{formatTokens(model.outputTokens)}</span>
                    <span className="token-usage-num" role="cell">{formatTokens(model.reasoningTokens)}</span>
                  </div>
                ))}
              </div>
            ))}
          </div>
          {data.sources.length > 0 ? (
            <p className="token-usage-sources">
              {data.sources
                .map(
                  (source) =>
                    `${sourceLabel(source.source)}: ${usageSourceStateText(source.state)}${
                      source.lastObservedAt ? ` · last observed ${formatSinceDate(source.lastObservedAt)}` : ""
                    }`,
                )
                .join(" · ")}
            </p>
          ) : null}
        </>
      ) : null}
    </section>
  );
}

function sourceLabel(source: string): string {
  if (source === "zcode") return "ZCode";
  if (source === "codex") return "Codex";
  if (source === "opencode") return "OpenCode";
  return source;
}

function formatSinceDate(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  }).format(date);
}
