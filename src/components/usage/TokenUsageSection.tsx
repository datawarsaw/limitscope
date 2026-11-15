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
  type UsageModelBreakdown,
  type UsageSourceCoverage,
  type UsageSourceDiagnostics,
} from "../../lib/usageIntelligence";
import { useUsageIntelligence } from "../../hooks/useUsageIntelligence";
import { UsageDailyActivity } from "./UsageDailyActivity";

/**
 * Usage Intelligence (v0.8.9): the Today / 7d / 30d view over reported
 * token usage collected locally, grouped by provider and model. It carries
 * the restored v0.9 Usage-first pieces — the KPI strip, the model ranking
 * with per-model expand, and the collection-source diagnostics — scoped
 * like the table: a narrowed provider scope ranks that provider's models
 * only, while collection diagnostics stay global.
 *
 * Honesty rules, enforced everywhere below:
 * - these are reported tokens collected locally — never account quota,
 *   billing usage, exact spend, or complete history; the KPIs say so;
 * - while the opt-in toggle is off, the section explains and never
 *   queries (mirroring the backend's no-probe-while-disabled contract);
 * - an empty view states when local collection started rather than
 *   implying the range was measured and empty;
 * - a range reaching further back than collection itself discloses that
 *   ("Collected locally since …", plus the incomplete qualifier);
 * - only sources with collected data get provider groups; there are no
 *   empty Codex / OpenCode placeholders; source failures and
 *   completions-without-reported-usage stay visible;
 * - a source whose history is incomplete for the range says so in the
 *   per-source coverage banner (v0.9), and the local-days token-activity
 *   heatmap shows observed activity without zero-filling unobserved days.
 *
 * Optimized for fast comparison: one dense table, compact K/M/B numbers.
 */

/** Ranking rows stay compact; the full model list opens on demand. */
const MAX_VISIBLE_MODELS = 6;

type ModelRow = UsageModelBreakdown & { provider: string };

/** Dot tone for a source's diagnostic state; failures stay visible. */
function sourceTone(state: UsageSourceDiagnostics["state"]): string {
  switch (state) {
    case "ok":
    case "collecting":
      return "status-ok";
    case "disabled":
    case "sourceAbsent":
      return "status-unknown";
    default:
      return "status-error";
  }
}

/** Per-source collection diagnostics; also the standalone empty-state list. */
function SourceDiagnosticsList({
  sources,
  standalone = false,
}: {
  sources: UsageSourceDiagnostics[];
  standalone?: boolean;
}) {
  if (sources.length === 0) return null;
  return (
    <ul
      className={`usage-sources-list${standalone ? " usage-sources-standalone" : ""}`}
      aria-label="Collection sources"
    >
      {sources.map((source) => (
        <li key={source.source} className="usage-source-row">
          <span className={`status-dot ${sourceTone(source.state)}`} aria-hidden="true" />
          <span className="usage-source-copy">
            {sourceLabel(source.source)}: {usageSourceStateText(source.state)}
            {source.lastObservedAt
              ? ` · last observed ${formatSinceDate(source.lastObservedAt)}`
              : ""}
            {source.lastScan?.usageNotReported
              ? ` · ${source.lastScan.usageNotReported} completions reported no usage`
              : ""}
          </span>
        </li>
      ))}
    </ul>
  );
}

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
  const [expandedModel, setExpandedModel] = useState<string | null>(null);
  const [showAllModels, setShowAllModels] = useState(false);

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

  // Restored v0.9 ranking: heaviest models first, provider-qualified so the
  // same model identity can appear under two providers as distinct rows.
  const modelRows = useMemo<ModelRow[]>(() => {
    const rows = groups.flatMap((group) =>
      group.models.map((model) => ({ ...model, provider: group.provider })),
    );
    rows.sort((a, b) => b.totalTokens - a.totalTokens);
    return rows;
  }, [groups]);

  const providerRows = useMemo(
    () => [...groups].sort((a, b) => b.totalTokens - a.totalTokens),
    [groups],
  );

  // Sums over the scoped groups: the KPIs describe what the table shows.
  // All-providers scope reconciles with the payload totals.
  const scopedTotals = useMemo(
    () =>
      groups.reduce(
        (acc, group) => ({
          events: acc.events + group.events,
          totalTokens: acc.totalTokens + group.totalTokens,
        }),
        { events: 0, totalTokens: 0 },
      ),
    [groups],
  );

  // Distinct model identities, not provider-qualified rows (the same model
  // can surface under several providers — each row stays in the ranking).
  const modelsObserved = new Set(modelRows.map((row) => row.model)).size;
  const maxModelTokens = modelRows[0]?.totalTokens ?? 0;
  const maxProviderTokens = providerRows[0]?.totalTokens ?? 0;
  const hiddenModelCount = Math.max(
    0,
    modelRows.length - (showAllModels ? modelRows.length : MAX_VISIBLE_MODELS),
  );
  const visibleModelRows = showAllModels
    ? modelRows
    : modelRows.slice(0, MAX_VISIBLE_MODELS);

  // Both instants are canonical RFC-3339 UTC strings of the same shape, so
  // the comparison is exact: the range reaches before collection only when
  // the first global baseline is later than the selected range start.
  const rangePredatesCollection =
    data?.collectionStartedAt !== undefined &&
    data.collectionStartedAt > data.rangeStart;

  // Per-source coverage honesty (v0.9): a source whose history is not
  // complete for the range says so, instead of the totals silently
  // implying the whole period was observed.
  const affectedCoverage = useMemo(
    () =>
      data?.sourceCoverage?.filter(
        (coverage) => coverage.coverage !== "complete",
      ) ?? [],
    [data],
  );

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

      {affectedCoverage.length > 0 ? (
        <p className="usage-subtitle" role="note" aria-label="History coverage">
          <strong>History coverage:</strong>{" "}
          {affectedCoverage
            .map(
              (coverage) =>
                `${sourceLabel(coverage.source)} — ${coverageDetail(coverage)}`,
            )
            .join("; ")}
          . Reported totals show what was collected, not the full period.
        </p>
      ) : null}

      {data !== null &&
      (data.persistenceAvailable === false || data.lastWriteSucceeded === false) ? (
        <p className="usage-refresh-error" role="alert">
          Local usage could not be saved. Collection progress is retained for
          retry; totals may be incomplete.
        </p>
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

      {/* Restored v0.9 diagnostics: a source that collects but has no data
          yet stays visible instead of looking absent. */}
      {data !== null && groups.length === 0 ? (
        <SourceDiagnosticsList sources={data.sources} standalone />
      ) : null}

      {groups.length > 0 && data !== null ? (
        <>
          {intelligence.error ? (
            <p className="usage-refresh-error" role="alert">
              Could not refresh token usage: {intelligence.error}
            </p>
          ) : null}

          <div className="usage-kpis" role="group" aria-label="Token usage summary">
            <div className="usage-kpi">
              <span className="usage-kpi-label">Reported tokens</span>
              <span className="usage-kpi-value">
                {formatTokens(scopedTotals.totalTokens)}
              </span>
              <span className="usage-kpi-sub">
                observed as reported — not your total usage
              </span>
            </div>
            <div className="usage-kpi">
              <span className="usage-kpi-label">Usage events</span>
              <span className="usage-kpi-value">{scopedTotals.events}</span>
              <span className="usage-kpi-sub">
                recorded completions — not every API request
              </span>
            </div>
            <div className="usage-kpi">
              <span className="usage-kpi-label">Models observed</span>
              <span className="usage-kpi-value">{modelsObserved}</span>
              <span className="usage-kpi-sub">
                {modelsObserved === 1
                  ? "1 distinct model"
                  : "distinct models across providers"}
              </span>
            </div>
            <div className="usage-kpi">
              <span className="usage-kpi-label">Collection</span>
              <span className="usage-kpi-value usage-kpi-date">
                {data.collectionStartedAt
                  ? `since ${formatSinceDate(data.collectionStartedAt)}`
                  : "not started"}
              </span>
              <span className="usage-kpi-sub">
                {data.incompleteHistory
                  ? rangePredatesCollection
                    ? "range reaches before collection started"
                    : "history is incomplete for this range"
                  : `covers the ${rangeLabel} range`}
              </span>
            </div>
          </div>

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

          <div className="usage-rankings">
            <section className="usage-panel usage-ranking" aria-label="Models">
              <h3 className="panel-heading">Models</h3>
              <ul className="ranking-list">
                {visibleModelRows.map((row) => {
                  const key = `${row.provider}|${row.model}`;
                  const expanded = expandedModel === key;
                  return (
                    <li key={key} className="ranking-item">
                      <button
                        type="button"
                        className="ranking-row"
                        aria-expanded={expanded}
                        onClick={() => setExpandedModel(expanded ? null : key)}
                      >
                        <span className="ranking-line">
                          <span className="ranking-name" title={row.model}>
                            {row.model}
                          </span>
                          <span className="ranking-value">
                            {formatTokens(row.totalTokens)}
                          </span>
                        </span>
                        <span className="ranking-bar-track" aria-hidden="true">
                          <span
                            className="ranking-bar-fill"
                            style={{
                              width:
                                maxModelTokens > 0
                                  ? `${Math.max(2, (row.totalTokens / maxModelTokens) * 100)}%`
                                  : "0%",
                            }}
                          />
                        </span>
                      </button>
                      {expanded ? (
                        <div className="model-detail">
                          <div className="model-detail-grid">
                            <span className="model-detail-cell">
                              <span className="model-detail-label">Input</span>
                              <span className="model-detail-value">
                                {formatTokens(row.inputTokens)}
                              </span>
                            </span>
                            <span className="model-detail-cell">
                              <span className="model-detail-label">Cached</span>
                              <span className="model-detail-value">
                                {formatTokens(cachedTokens(row))}
                              </span>
                            </span>
                            <span className="model-detail-cell">
                              <span className="model-detail-label">Output</span>
                              <span className="model-detail-value">
                                {formatTokens(row.outputTokens)}
                              </span>
                            </span>
                            <span className="model-detail-cell">
                              <span className="model-detail-label">Reasoning</span>
                              <span className="model-detail-value">
                                {formatTokens(row.reasoningTokens)}
                              </span>
                            </span>
                          </div>
                          <p className="model-detail-note">
                            {row.events} recorded{" "}
                            {row.events === 1 ? "event" : "events"} ·{" "}
                            {usageProviderLabel(row.provider)}
                          </p>
                        </div>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
              {hiddenModelCount > 0 ? (
                <button
                  type="button"
                  className="ranking-more"
                  onClick={() => setShowAllModels(true)}
                >
                  Show {hiddenModelCount} more{" "}
                  {hiddenModelCount === 1 ? "model" : "models"}
                </button>
              ) : null}
            </section>

            <section
              className="usage-panel usage-ranking"
              aria-label="Usage sources"
            >
              <h3 className="panel-heading">Sources</h3>
              <ul className="ranking-list">
                {providerRows.map((group) => (
                  <li
                    key={group.provider}
                    className="ranking-item ranking-item-static"
                  >
                    <span className="ranking-line">
                      <span className="ranking-name">
                        {usageProviderLabel(group.provider)}
                      </span>
                      <span className="ranking-value">
                        {formatTokens(group.totalTokens)}
                      </span>
                    </span>
                    <span className="ranking-bar-track" aria-hidden="true">
                      <span
                        className="ranking-bar-fill"
                        style={{
                          width:
                            maxProviderTokens > 0
                              ? `${Math.max(2, (group.totalTokens / maxProviderTokens) * 100)}%`
                              : "0%",
                        }}
                      />
                    </span>
                    <span className="ranking-sub">
                      {group.models.length}{" "}
                      {group.models.length === 1 ? "model" : "models"} ·{" "}
                      {group.events} {group.events === 1 ? "event" : "events"}
                    </span>
                  </li>
                ))}
              </ul>
              <SourceDiagnosticsList sources={data.sources} />
            </section>
          </div>

          <UsageDailyActivity days={data.daily} />
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

/** Why a source's history is incomplete; phrased for the coverage banner. */
function coverageDetail(coverage: UsageSourceCoverage): string {
  switch (coverage.reason) {
    case "lateBaseline":
      return coverage.coveredFrom
        ? `history is only covered since ${formatSinceDate(coverage.coveredFrom)}`
        : "history starts partway through this range";
    case "monitoringDisabled":
      return "collection is off; history while off is missing";
    case "pendingRebaseline":
      return "history while collection was off is missing";
    case "noBaseline":
      return "no history collected yet";
    case "sourceAbsent":
    case "schemaUnsupported":
    case "readFailure":
      return coverage.lastObservedAt
        ? `may be missing events after ${formatSinceDate(coverage.lastObservedAt)}`
        : "recent events may be missing";
    case "scanCeiling":
      return "collection has a recoverable backlog";
    case "collectionPaused":
      return coverage.lastObservedAt
        ? `collection is paused — events after ${formatSinceDate(coverage.lastObservedAt)} may be missing`
        : "collection is paused — recent events may be missing";
    default:
      return "history coverage is incomplete";
  }
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
