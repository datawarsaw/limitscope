import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  importLegacyHistory,
  loadRuntimeHistory,
  readLegacyHistory,
  removeLegacyHistory,
} from "../lib/historyClient";
import {
  clearUsageHistoryStore,
  type LocalDataClearResult,
} from "../lib/localData";
import type { QuotaObservation } from "../lib/quotaHistory";
import { predictWindows } from "../lib/prediction/engine";
import {
  historyForCurrentAccounts,
  predictionKey,
  predictionsByIdentity,
} from "../lib/v03Integration";
import type { QuotaPrediction } from "../lib/prediction/types";
import type { ProviderUsage } from "../types";

/**
 * History ownership rule (v0.5 phase 2): Rust is the single history owner.
 * The runtime records one observation batch per cycle from the snapshot it
 * broadcasts, and this hook only consumes read results — it never writes
 * localStorage and never records from any window, so main open + floating
 * open is one history stream, not two. Outside Tauri (plain-browser dev,
 * tests) there is no runtime and the hook degrades to an empty history.
 *
 * The one-time legacy migration runs in the main window only, mirroring the
 * retired single-recorder rule: the main window always exists (created
 * hidden at startup), so the legacy localStorage blob is imported exactly
 * once regardless of which windows the user opens.
 */
function isMainWindowView(): boolean {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return true;
  }
  return getCurrentWindow().label === "main";
}

/**
 * Best-effort v0.3 history and prediction lifecycle over Rust-owned
 * history. Reads and the clear mutation can never interrupt refresh, tray
 * actions, or last-good rendering: a failed read keeps the last known
 * history and surfaces `historyUnavailable`.
 *
 * Predictions only ever see the current account's slice of history
 * (`historyForCurrentAccounts`): when a provider proves an account, its
 * observations are partitioned by that identity, so swapping a stored
 * credential starts fresh predictions instead of inheriting the previous
 * account's samples — including for imported legacy data, whose
 * unattributed entries never feed a proven account.
 */
export function useQuotaPredictions(
  usages: readonly ProviderUsage[],
  nowMs: number,
  historyRevision: number | null = null,
) {
  const [history, setHistory] = useState<QuotaObservation[]>([]);
  const [historyUnavailable, setHistoryUnavailable] = useState(false);
  // The history revision behind the `history` state, so a runtime snapshot
  // only triggers a re-pull when the runtime actually changed the history.
  const pulledRevisionRef = useRef<number | null>(null);

  const pullHistory = useCallback(async () => {
    try {
      const observations = await loadRuntimeHistory();
      setHistory(observations);
      setHistoryUnavailable(false);
    } catch (error) {
      // The history source itself failed; keep serving the last known
      // history and let the settings note explain the state.
      console.error("Failed to read the quota history", error);
      setHistoryUnavailable(true);
    }
  }, []);

  // Initial read on attach — predictions are ready as soon as the runtime
  // answers, even before the first snapshot of this session arrives.
  useEffect(() => {
    void pullHistory();
  }, [pullHistory]);

  // Re-read when the runtime reports changed history (new observations,
  // prune, import, or a clear from either window).
  useEffect(() => {
    if (historyRevision === null) return;
    if (historyRevision === pulledRevisionRef.current) return;
    pulledRevisionRef.current = historyRevision;
    void pullHistory();
  }, [historyRevision, pullHistory]);

  // One-time legacy migration, main window only. After a successful import
  // the legacy key is removed, so the migration can never run again (and a
  // cleared history can never be resurrected from it); a failed or skipped
  // import leaves the key in place for the next launch. The promise is kept
  // so a user clear can deterministically wait out an in-flight import.
  const migrationRef = useRef<Promise<void> | null>(null);
  useEffect(() => {
    if (!isMainWindowView()) return;
    let cancelled = false;
    const migration = (async () => {
      const legacy = readLegacyHistory();
      if (!legacy.present) return;
      try {
        await importLegacyHistory(legacy.observations);
      } catch (error) {
        console.error("Failed to import the legacy quota history", error);
        return;
      }
      if (!cancelled) removeLegacyHistory();
    })();
    migrationRef.current = migration;
    return () => {
      cancelled = true;
    };
  }, []);

  const predictions = useMemo(() => {
    const now = new Date(nowMs).toISOString();
    const current = historyForCurrentAccounts(history, usages);
    return predictionsByIdentity(predictWindows({ observations: current, now }));
  }, [history, usages, nowMs]);

  const predictionFor = useCallback(
    (providerId: string, windowLabel: string): QuotaPrediction | undefined =>
      predictions.get(predictionKey(providerId, windowLabel)),
    [predictions],
  );

  /**
   * Clears the user's stored quota history. Deterministic ordering against
   * the one-time migration: an in-flight legacy import is waited out first,
   * so a late import can never resurrect history behind the user's clear.
   * The owned local data service then clears the Rust store through its
   * canonical command and drops the retired legacy blob; the UI drops the
   * derived in-memory history (and with it every prediction) immediately.
   *
   * The outcome is returned so the caller can report a scoped failure
   * instead of claiming the data was removed.
   */
  const clearLocalHistory = useCallback(async (): Promise<LocalDataClearResult> => {
    await (migrationRef.current ?? Promise.resolve()).catch(() => {});
    const result = await clearUsageHistoryStore();
    if (!result.ok) return result;
    setHistory([]);
    setHistoryUnavailable(false);
    return result;
  }, []);

  return { predictionFor, historyUnavailable, clearLocalHistory };
}
