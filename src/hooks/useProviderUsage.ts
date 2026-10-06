import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { RuntimeSnapshot } from "../types";

const SNAPSHOT_EVENT = "runtime://snapshot";
const CYCLE_STARTED_EVENT = "runtime://cycle-started";
// How often the "stale" relative-time readout is re-rendered.
const CLOCK_TICK_MS = 30 * 1000;

function isRunningInTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Consumer side of the shared Rust provider runtime. Rust is the single
 * refresh owner (cycles, coalescing, retry, last-good, interval timer);
 * this hook only projects runtime state into view state:
 *
 * - subscribes to runtime events and pulls the current snapshot once on
 *   attach (the `seq` guard makes pull/event ordering irrelevant);
 * - pushes the persisted refresh interval to the runtime on attach and on
 *   every settings change — no provider timer lives in TS anymore;
 * - `refresh` requests a runtime cycle (one owner, all windows update from
 *   the same snapshot fan-out).
 *
 * The public API is unchanged from the pre-runtime hook, so both windows
 * (main dashboard, floating quota bar) keep their exact view semantics —
 * while duplicate per-window provider cycles are gone.
 */
export function useProviderUsage(refreshIntervalMinutes: number) {
  const [snapshot, setSnapshot] = useState<RuntimeSnapshot | null>(null);
  // Optimistic loading for a just-requested cycle; the runtime's
  // cycle-started/snapshot events take over from there.
  const [requested, setRequested] = useState(false);
  // Reference point for the stale readout before any successful cycle.
  const [attachAt] = useState(() => Date.now());
  const [now, setNow] = useState(() => Date.now());
  const lastSeqRef = useRef(0);

  const applySnapshot = useCallback((next: RuntimeSnapshot) => {
    if (next.seq < lastSeqRef.current) return;
    lastSeqRef.current = next.seq;
    setSnapshot(next);
    if (!next.cycleInFlight) setRequested(false);
  }, []);

  useEffect(() => {
    if (!isRunningInTauri()) return;
    let cancelled = false;
    let detach = () => {};
    void (async () => {
      // Subscribe first, then pull: a snapshot arriving in between is
      // applied and the pull result is ignored by the seq guard (or
      // vice versa — both orders end on the newest state).
      const unlistenSnapshot = await listen<RuntimeSnapshot>(
        SNAPSHOT_EVENT,
        (event) => applySnapshot(event.payload),
      );
      const unlistenStarted = await listen(CYCLE_STARTED_EVENT, () => {
        setRequested(true);
      });
      if (cancelled) {
        unlistenSnapshot();
        unlistenStarted();
        return;
      }
      detach = () => {
        unlistenSnapshot();
        unlistenStarted();
      };
      try {
        applySnapshot(await invoke<RuntimeSnapshot>("get_runtime_snapshot"));
      } catch (error) {
        console.error("Failed to pull the runtime snapshot", error);
      }
    })();
    return () => {
      cancelled = true;
      detach();
    };
  }, [applySnapshot]);

  // The runtime owns the timer; TS only forwards the persisted interval
  // (kept in localStorage by settings.ts) at attach and on every change.
  useEffect(() => {
    if (!isRunningInTauri()) return;
    void invoke("set_refresh_interval", { minutes: refreshIntervalMinutes });
  }, [refreshIntervalMinutes]);

  // Resilience trigger (v0.5): after connectivity returns, ask the shared
  // runtime for one cycle. JS is only a trigger — the Rust runtime is the
  // only fetch owner, and it coalesces bursts (both windows fire `online`,
  // and interfaces flap). No provider state is touched here.
  useEffect(() => {
    if (!isRunningInTauri()) return;
    const onOnline = () => {
      void invoke("request_refresh_on_reconnect").catch((error) => {
        console.error("Failed to request a reconnect refresh", error);
      });
    };
    window.addEventListener("online", onOnline);
    return () => window.removeEventListener("online", onOnline);
  }, []);

  const refresh = useCallback(() => {
    if (!isRunningInTauri()) return;
    setRequested(true);
    void invoke("request_refresh").catch((error) => {
      // The request never reached the runtime; no snapshot will close the
      // optimistic loading span, so clear it here.
      console.error("Failed to request a runtime refresh", error);
      setRequested(false);
    });
  }, []);

  // Wall clock for the relative "stale" age in the footer.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), CLOCK_TICK_MS);
    return () => window.clearInterval(timer);
  }, []);

  const usages = snapshot?.providers ?? [];
  // Loading spans a whole chain: from the request (or the runtime's
  // cycle-started event) until the closing snapshot of the chain.
  const loading = snapshot === null ? isRunningInTauri() : snapshot.cycleInFlight || requested;
  const lastUpdatedAt = snapshot?.lastUpdatedAt
    ? new Date(snapshot.lastUpdatedAt)
    : null;
  const refreshOverdue = snapshot !== null && !snapshot.cycleSucceeded;
  // Monotonic revision of the Rust-owned quota history; consumers re-pull
  // `get_history` only when it moves. `null` until the first snapshot.
  const historyRevision = snapshot?.historyRevision ?? null;
  // Monotonic revision of the Rust-owned Usage Intelligence store (v0.8.9);
  // consumers re-pull only when it moves.
  const usageIntelligenceRevision = snapshot?.usageIntelligenceRevision ?? null;

  // Data counts as stale after 2 × configured refresh interval without a
  // successful refresh; "min ago" is measured from the last successful
  // runtime cycle, or from attach if none has happened yet.
  const staleAfterMs = refreshIntervalMinutes * 2 * 60 * 1000;
  const referenceAt = lastUpdatedAt?.getTime() ?? attachAt;
  const staleMinutes = Math.max(0, Math.floor((now - referenceAt) / 60000));
  const stale = now - referenceAt > staleAfterMs;

  return {
    usages,
    loading,
    lastUpdatedAt,
    refresh,
    refreshOverdue,
    stale,
    staleMinutes,
    historyRevision,
    usageIntelligenceRevision,
  };
}
