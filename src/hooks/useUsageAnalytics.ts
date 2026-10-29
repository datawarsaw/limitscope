import { useCallback, useEffect, useRef, useState } from "react";
import {
  codexScopeIsCurrentOrNewer,
  loadUsageAnalytics,
  type CodexScope,
  type UsageAnalytics,
  type UsageAnalyticsLoader,
  type UsageAnalyticsQuery,
} from "../lib/usageAnalytics";

/**
 * Analytics state for the Usage view. The query is issued only while the
 * Usage view is enabled (mounted) — the provider quota runtime and the
 * Overview never wait on it — and a failure lands here, in this view, never
 * on the live provider cards.
 *
 * Requests are keyed on the normalized query (plus the runtime's history
 * revision, so a new observation cycle refreshes an open view once per
 * cycle). Renders never trigger fetches, and an in-flight result for an
 * outdated key is discarded; the previous payload stays visible (marked
 * refreshing) until the fresh one lands, so switching 24h/7d doesn't flash
 * the view empty.
 */
export type UsageAnalyticsState = {
  data: UsageAnalytics | null;
  /** No payload for the current query yet. */
  loading: boolean;
  /** Refetch in flight while a previous payload stays on screen. */
  refreshing: boolean;
  /** Display-safe failure of the last query for the current key. */
  error: string | null;
};

export type UsageAnalyticsView = UsageAnalyticsState & {
  /** Re-runs the current query (error banner action). */
  retry: () => void;
};

const IDLE: UsageAnalyticsState = {
  data: null,
  loading: false,
  refreshing: false,
  error: null,
};

function errorText(error: unknown): string {
  return error instanceof Error && error.message
    ? error.message
    : "Usage history could not be loaded.";
}

/**
 * The canonical form of a Codex identity scope: the slots, their observed
 * generations and their identities, in a stable order. It is the cache key
 * for one session's projection.
 */
function scopeKey(scope: CodexScope): string {
  return JSON.stringify(
    [...scope]
      .sort((a, b) => a.providerId.localeCompare(b.providerId))
      .map((state) => [state.providerId, state.generation, state.identity]),
  );
}

export function useUsageAnalytics(
  query: UsageAnalyticsQuery,
  enabled: boolean,
  historyRevision: number | null,
  loader: UsageAnalyticsLoader = loadUsageAnalytics,
  identityScope: CodexScope = [],
): UsageAnalyticsView {
  const [state, setState] = useState<UsageAnalyticsState>(IDLE);
  // The scope the currently displayed payload was computed under.
  const [dataScope, setDataScope] = useState<string>(scopeKey(identityScope));
  // Key of the request whose response is currently allowed to land.
  const keyRef = useRef<string>("");
  const requestRef = useRef(0);
  // The scope the consumer currently observes. A response computed under an
  // older scope must not land under the current one.
  const scopeRef = useRef<CodexScope>(identityScope);
  scopeRef.current = identityScope;

  const currentScopeKey = scopeKey(identityScope);
  const queryKey = JSON.stringify([query, currentScopeKey]);

  const runQuery = useCallback(
    (key: string, currentQuery: UsageAnalyticsQuery) => {
      const request = ++requestRef.current;
      keyRef.current = key;
      setState((previous) => ({
        data: dataScope === currentScopeKey ? previous.data : null,
        loading: dataScope !== currentScopeKey || previous.data === null,
        refreshing: dataScope === currentScopeKey && previous.data !== null,
        error: null,
      }));
      setDataScope(currentScopeKey);
      loader(currentQuery)
        .then((data) => {
          if (request !== requestRef.current || keyRef.current !== key) return;
          // The projection carries the Codex identity scope it was computed
          // under. A response that belongs to an older session must not be
          // shown under the current one.
          if (!codexScopeIsCurrentOrNewer(data.codexIdentityScope, scopeRef.current)) {
            // The response is obsolete: the consumer already observed a
            // newer session. The spinner must not stay up for a response
            // that will never be shown.
            setState((previous) => ({
              ...previous,
              loading: false,
              refreshing: false,
            }));
            return;
          }
          setState({ data, loading: false, refreshing: false, error: null });
        })
        .catch((error) => {
          if (request !== requestRef.current || keyRef.current !== key) return;
          console.error("Usage analytics query failed", error);
          setState((previous) => ({
            data: previous.data,
            loading: false,
            refreshing: false,
            error: errorText(error),
          }));
        });
    },
    [loader, dataScope, currentScopeKey],
  );

  useEffect(() => {
    if (!enabled) return;
    if (queryKey !== keyRef.current) runQuery(queryKey, query);
  }, [enabled, queryKey, query, runQuery]);

  // A runtime cycle that changed history refreshes the open view exactly
  // once per revision — never per render, never while the view is closed.
  const revisionRef = useRef<number | null>(null);
  useEffect(() => {
    if (!enabled) return;
    if (historyRevision === null) return;
    if (revisionRef.current === historyRevision) return;
    const firstSight = revisionRef.current === null;
    revisionRef.current = historyRevision;
    // Skip the initial attach when this query was just issued (or already
    // answered) for this mount; only *changed* revisions re-query.
    if (!firstSight || queryKey !== keyRef.current) runQuery(queryKey, query);
  }, [enabled, historyRevision, queryKey, query, runQuery]);

  const retry = useCallback(() => {
    if (enabled) runQuery(queryKey, query);
  }, [enabled, queryKey, query, runQuery]);

  return dataScope === currentScopeKey
    ? { ...state, retry }
    : { ...IDLE, loading: enabled, retry };
}
