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
  type UsageSourceDiagnostics,
  type UsageSourceCoverage,
} from "../../lib/usageIntelligence";
import { useUsageIntelligence } from "../../hooks/useUsageIntelligence";
import { UsageDailyActivity } from "./UsageDailyActivity";

/**
 * The Usage-first main-window overview (v0.9): one calm surface over the
 * backend `get_usage_intelligence` projection — reported tokens collected
 * locally, grouped by provider and model.
 *
 * Honesty rules, enforced everywhere below:
 * - these are reported tokens collected locally — never account quota,
 *   billing usage, exact spend, or complete history;
 * - usage events are recorded completions, not the same thing as "every API
 *   request", and observed usage is not the user's total actual usage;
 * - while the opt-in toggle is off, the view explains and never queries
 *   (mirroring the backend's no-probe-while-disabled contract);
 * - an empty view states when local collection started rather than implying
 *   the range was measured and empty;
 * - a range reaching further back than collection itself discloses that
 *   ("less history than the range"); per-source gaps (a late baseline, a
 *   disabled period, a paused or failed source) name the source and its
 *   coverage boundary instead of claiming the whole range is complete;
 * - only sources with collected data get provider groups; source failures
 *   and completions-without-reported-usage stay visible;
 * - the Daily activity heatmap renders the backend's per-local-day
 *   projection (`daily`): relative reported-token intensity, hatched
 *   missing days, and before-collection days — never invented history.
 */

/** Ranking rows stay compact; the full model list opens on demand. */
const MAX_VISIBLE_MODELS = 6;

type ModelRow = UsageModelBreakdown & { provider: string };

/**
 * Dev-only initial range from `?range=7d|30d` (visual-fixture URLs mirror
 * the `?view=` seam in App.tsx). Production always lands on Today.
 */
function devInitialRange(): UsageIntelligenceRange {
  if (import.meta.env.DEV && typeof window !== "undefined") {
    const param = new URLSearchParams(window.location.search).get("range");
    if (param === "7d" || param === "30d") return param;
  }
  return "today";
}

export function UsageOverview({
  enabled,
  revision,
  loader = loadUsageIntelligence,
  onOpenSettings,
  nowMs,
}: {
  /** The persisted Usage Intelligence opt-in (settings). */
  enabled: boolean;
  /** Store revision from the runtime snapshot; drives per-pass refresh. */
  revision: number | null;
  /** Injected source (tests, dev fixtures); production default. */
  loader?: UsageIntelligenceLoader;
  /** Navigates to Settings (the "off" explainer's action). */
  onOpenSettings?: () => void;
  /**
   * Wall clock for the Today anchor — the app's existing tick (useNow),
   * so a view that stays open across local midnight re-anchors without a
   * dedicated timer. The derived anchor changes value only at midnight.
   */
  nowMs?: number;
}) {
  const [range, setRange] = useState<UsageIntelligenceRange>(
    () => devInitialRange(),
  );
  const [expandedModel, setExpandedModel] = useState<string | null>(null);
  const [showAllModels, setShowAllModels] = useState(false);

  // The Today anchor is derived from the wall clock, not captured once per
  // range switch: the memo recomputes only when the local day actually
  // changes, which alone changes the query key and re-queries the backend.
  const todayStartMs =
    range === "today"
      ? localMidnightMs(new Date(nowMs ?? Date.now()))
      : undefined;

  const query = useMemo(
    () => ({
      range,
      ...(todayStartMs !== undefined ? { todayStartMs } : {}),
    }),
    [range, todayStartMs],
  );

  const intelligence = useUsageIntelligence(query, enabled, revision, loader);
  const data = intelligence.data;

  // Model ranking across every provider group, heaviest first. The row key
  // is provider-qualified: the same model name can appear under two
  // providers, and both observations are real.
  const modelRows = useMemo<ModelRow[]>(() => {
    if (!data) return [];
    const rows = data.groups.flatMap((group) =>
      group.models.map((model) => ({ ...model, provider: group.provider })),
    );
    rows.sort((a, b) => b.totalTokens - a.totalTokens);
    return rows;
  }, [data]);

  const providerRows = useMemo(() => {
    if (!data) return [];
    return [...data.groups].sort((a, b) => b.totalTokens - a.totalTokens);
  }, [data]);

  const affectedCoverage = useMemo(
    () =>
      data?.sourceCoverage?.filter(
        (coverage) => coverage.coverage !== "complete",
      ) ?? [],
    [data],
  );

  // The naming contract makes `model` the normalized model identity and
  // keeps the provider qualifier on the separate provider axis (Codex splits
  // `xai/grok-4.6`; ZCode and OpenCode report the provider directly). The
  // same identity can therefore surface under several providers as distinct
  // rows — both observations stay in the ranking above, but the KPI counts
  // identities, not provider-qualified rows.
  const modelsObserved = new Set(modelRows.map((row) => row.model)).size;
  const maxModelTokens = modelRows[0]?.totalTokens ?? 0;
  const maxProviderTokens = providerRows[0]?.totalTokens ?? 0;
  const hiddenModelCount = Math.max(
    0,
    modelRows.length - (showAllModels ? modelRows.length : MAX_VISIBLE_MODELS),
  );
  const visibleModelRows = showAllModels ? modelRows : modelRows.slice(0, MAX_VISIBLE_MODELS);

  const rangeLabel =
    range === "today" ? "Today" : range === "7d" ? "7 days" : "30 days";

  // Both instants are canonical RFC-3339 UTC strings of the same shape, so
  // the comparison is exact: the range reaches before collection only when
  // the first global baseline is later than the selected range start.
  const rangePredatesCollection =
    data?.collectionStartedAt !== undefined &&
    data.collectionStartedAt > data.rangeStart;

  if (!enabled) {
    return (
      <div className="usage-view usage-overview" aria-label="Usage overview">
        <section className="usage-panel usage-off" aria-label="Usage Intelligence">
          <h3 className="panel-heading">Token usage</h3>
          <p className="usage-off-note">Usage Intelligence is off.</p>
          <p className="usage-off-copy">
            When enabled, LimitScope reads local AI tool usage metadata —
            models and token counts — from supported tools like ZCode, Codex,
            and OpenCode, on this device only. It never reads prompt or
            response content, and nothing is uploaded.
          </p>
          {onOpenSettings ? (
            <button
              type="button"
              className="usage-off-btn"
              onClick={onOpenSettings}
            >
              Open Settings
            </button>
          ) : null}
        </section>
      </div>
    );
  }

  return (
    <div className="usage-view usage-overview" aria-label="Usage overview">
      <div className="usage-toolbar">
        <div className="usage-range" role="group" aria-label="Range">
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

      {intelligence.error && data === null ? (
        <div className="usage-panel usage-error" role="alert">
          <p>Token usage could not be loaded. {intelligence.error}</p>
          <p className="usage-error-note">
            Live provider quotas are unaffected — only this view failed.
          </p>
          <button
            type="button"
            className="usage-retry"
            onClick={() => intelligence.retry()}
          >
            Retry
          </button>
        </div>
      ) : null}

      {intelligence.loading && data === null ? (
        <div className="usage-loading" role="status">
          Loading token usage…
        </div>
      ) : null}

      {affectedCoverage.length > 0 ? (
        <p
          className="usage-subtitle"
          role="note"
          aria-label="History coverage"
        >
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

      {data !== null && providerRows.length === 0 ? (
        <div className="usage-panel usage-empty">
          <p>No locally collected token usage yet.</p>
          <p className="usage-empty-note">
            {data.collectionStartedAt
              ? `Collection started ${formatSinceDate(data.collectionStartedAt)}; new requests appear here as they complete.`
              : "Collection starts with the next completed request; history from before the feature was enabled is never imported."}
          </p>
        </div>
      ) : null}

      {data !== null && providerRows.length > 0 ? (
        <>
          {intelligence.error ? (
            <p className="usage-refresh-error" role="alert">
              Could not refresh token usage: {intelligence.error}
            </p>
          ) : null}

          <p className="usage-subtitle">
            Reported token usage collected locally
            {data.collectionStartedAt
              ? ` since ${formatSinceDate(data.collectionStartedAt)}`
              : ""}
            {data.incompleteHistory
              ? rangePredatesCollection
                ? ` — less history than the ${rangeLabel} range`
                : " — some source history is incomplete for this range"
              : ""}
          </p>

          <div className="usage-kpis" role="group" aria-label="Usage summary">
            <div className="usage-kpi">
              <span className="usage-kpi-label">Reported tokens</span>
              <span className="usage-kpi-value">
                {formatTokens(data.totals.totalTokens)}
              </span>
              <span className="usage-kpi-sub">
                observed as reported — not your total usage
              </span>
            </div>
            <div className="usage-kpi">
              <span className="usage-kpi-label">Usage events</span>
              <span className="usage-kpi-value">{data.totals.events}</span>
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
                  : `distinct models across providers`}
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
                        onClick={() =>
                          setExpandedModel(expanded ? null : key)
                        }
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
                              <span className="model-detail-label">
                                Reasoning
                              </span>
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
              {data.sources.length > 0 ? (
                <ul className="usage-sources-list" aria-label="Collection sources">
                  {data.sources.map((source) => (
                    <li key={source.source} className="usage-source-row">
                      <span
                        className={`status-dot ${sourceTone(source.state)}`}
                        aria-hidden="true"
                      />
                      <span className="usage-source-copy">
                        {sourceLabel(source.source)}:{" "}
                        {usageSourceStateText(source.state)}
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
              ) : null}
            </section>
          </div>

          <UsageDailyActivity days={data.daily} />
        </>
      ) : null}

      {data !== null && providerRows.length === 0 && data.sources.length > 0 ? (
        <ul className="usage-sources-list usage-sources-standalone" aria-label="Collection sources">
          {data.sources.map((source) => (
            <li key={source.source} className="usage-source-row">
              <span
                className={`status-dot ${sourceTone(source.state)}`}
                aria-hidden="true"
              />
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
      ) : null}
    </div>
  );
}

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

function sourceLabel(source: string): string {
  if (source === "zcode") return "ZCode";
  if (source === "codex") return "Codex";
  if (source === "opencode") return "OpenCode";
  return source;
}

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
