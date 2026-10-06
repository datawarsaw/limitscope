import { useCallback, useEffect, useRef, useState } from "react";
import {
  loadUsageIntelligence,
  type UsageIntelligence,
  type UsageIntelligenceLoader,
  type UsageIntelligenceQuery,
} from "../lib/usageIntelligence";

/**
 * Usage Intelligence state for the token-usage section. Issued only while
 * the section is enabled (the opt-in toggle is on) and mounted — a
 * disabled section never queries at all, mirroring the backend's
 * no-probe-while-disabled contract.
 *
 * Requests are keyed on the normalized query plus the store's revision
 * (broadcast through the runtime snapshot), so an open view refreshes
 * once per collection pass. An in-flight result for an outdated key is
 * discarded; the previous payload stays visible until the fresh one
 * lands. A failure lands here, never on quota cards or analytics.
 */
export type UsageIntelligenceState = {
  data: UsageIntelligence | null;
  loading: boolean;
  error: string | null;
};

export type UsageIntelligenceView = UsageIntelligenceState & {
  retry: () => void;
};

const IDLE: UsageIntelligenceState = { data: null, loading: false, error: null };

function errorText(error: unknown): string {
  return error instanceof Error && error.message
    ? error.message
    : "Token usage could not be loaded.";
}

export function useUsageIntelligence(
  query: UsageIntelligenceQuery,
  enabled: boolean,
  revision: number | null,
  loader: UsageIntelligenceLoader = loadUsageIntelligence,
): UsageIntelligenceView {
  const [state, setState] = useState<UsageIntelligenceState>(IDLE);
  const keyRef = useRef<string>("");
  const requestRef = useRef(0);

  const queryKey = JSON.stringify(query);

  const runQuery = useCallback(
    (key: string, currentQuery: UsageIntelligenceQuery) => {
      const request = ++requestRef.current;
      keyRef.current = key;
      setState((previous) => ({
        data: previous.data,
        loading: previous.data === null,
        error: null,
      }));
      loader(currentQuery)
        .then((data) => {
          if (request !== requestRef.current || keyRef.current !== key) return;
          setState({ data, loading: false, error: null });
        })
        .catch((error) => {
          if (request !== requestRef.current || keyRef.current !== key) return;
          console.error("Usage Intelligence query failed", error);
          setState((previous) => ({
            data: previous.data,
            loading: false,
            error: errorText(error),
          }));
        });
    },
    [loader],
  );

  useEffect(() => {
    if (!enabled) return;
    if (queryKey !== keyRef.current) runQuery(queryKey, query);
  }, [enabled, queryKey, query, runQuery]);

  // A collection pass that changed the store refreshes the open view
  // exactly once per revision.
  const revisionRef = useRef<number | null>(null);
  useEffect(() => {
    if (!enabled) return;
    if (revision === null) return;
    if (revisionRef.current === revision) return;
    const firstSight = revisionRef.current === null;
    revisionRef.current = revision;
    if (!firstSight || queryKey !== keyRef.current) runQuery(queryKey, query);
  }, [enabled, revision, queryKey, query, runQuery]);

  const retry = useCallback(() => {
    if (enabled) runQuery(queryKey, query);
  }, [enabled, queryKey, query, runQuery]);

  return { ...state, retry };
}
