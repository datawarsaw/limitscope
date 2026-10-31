import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CodexIdentityState, ProviderUsage, RuntimeSnapshot } from "../types";
import { isCodexProviderId } from "../lib/providerIds";

const SNAPSHOT_EVENT = "runtime://snapshot";
const CYCLE_STARTED_EVENT = "runtime://cycle-started";
// Fallback cadence for the moment before the first snapshot arrives. It
// never decides the outcome: without a snapshot there is no successful cycle
// to age, so the readout is measured from the attach instant.
const DEFAULT_REFRESH_INTERVAL_MINUTES = 5;
// How often the "stale" relative-time readout is re-rendered.
const CLOCK_TICK_MS = 30 * 1000;
// Auth files can change without starting a provider cycle or emitting a
// snapshot event. Re-read the Rust-owned projection often enough to bound
// how long a prior Codex session can remain visible while idle or paused.
const SNAPSHOT_REVALIDATION_MS = 1000;

function isRunningInTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// The accepted reading of one Codex slot, fenced by the session generation
// the runtime observed for that slot. The generation is per slot: one
// account changing never moves another account's generation.
type CodexSlotEntry = {
  providerId: string;
  generation: number;
  identity: string | null;
  usage: ProviderUsage;
};

/**
 * Consumer side of the shared Rust provider runtime. Rust is the single
 * refresh owner (cycles, coalescing, retry, last-good, interval timer);
 * this hook only projects runtime state into view state:
 *
 * - subscribes to runtime events and pulls the current snapshot once on
 *   attach (the `seq` guard makes pull/event ordering irrelevant);
 * - runs the optional attach hand-off exactly once, so the settings lane
 *   can push the persisted refresh interval to the runtime through its own
 *   write queue — no provider timer lives in TS anymore;
 * - `refresh` requests a runtime cycle (one owner, all windows update from
 *   the same snapshot fan-out).
 *
 * The public API is unchanged from the pre-runtime hook, so both windows
 * (main dashboard, floating quota bar) keep their exact view semantics —
 * while duplicate per-window provider cycles are gone.
 */
export type ProviderUsageOptions = {
  /**
   * Runs once when the hook attaches. The settings lane uses it to hand the
   * persisted refresh interval over to the runtime. The hook itself never
   * writes a preference: a write from here would land outside the settings
   * write queue and could survive a reset or overwrite a newer edit.
   */
  onAttach?: () => void;
};

export function useProviderUsage(options?: ProviderUsageOptions) {
  const [snapshot, setSnapshot] = useState<RuntimeSnapshot | null>(null);
  // Optimistic loading for a just-requested cycle; the runtime's
  // cycle-started/snapshot events take over from there.
  const [requested, setRequested] = useState(false);
  // Wallet refreshes share the runtime cycle, but their own busy state is
  // kept separate: a wallet request must not make the provider surfaces
  // look like they are fetching.
  const [walletRequested, setWalletRequested] = useState(false);
  // Reference point for the stale readout before any successful cycle.
  const [attachAt] = useState(() => Date.now());
  const [now, setNow] = useState(() => Date.now());
  const [initialReadFailed, setInitialReadFailed] = useState(false);
  // The accepted reading of every Codex slot, each fenced by the slot's own
  // session generation. Codex slots are ordered independently of the global
  // snapshot sequence: an unrelated provider moving that sequence must
  // never suppress a Codex slot, and one slot must never suppress another.
  const [codexSlots, setCodexSlots] = useState<CodexSlotEntry[]>([]);
  // Slots whose current login could not be confirmed by a completed read. A
  // failed read proves nothing about any slot, so every known slot becomes
  // unconfirmed until a later read confirms it again. Events only carry a
  // snapshot taken when the runtime emitted it; they are not proof that the
  // current auth file was checked after the failure.
  const [unconfirmedCodexSlots, setUnconfirmedCodexSlots] = useState<
    ReadonlySet<string>
  >(new Set());
  const codexSlotsRef = useRef<CodexSlotEntry[]>([]);
  // The runtime's newest observation for every Codex slot. It is the
  // session scope a consumer compares its responses against, so it must
  // not lag behind the accepted readings.
  const [codexStates, setCodexStates] = useState<CodexIdentityState[]>([]);
  const codexStatesRef = useRef<Map<string, CodexIdentityState>>(new Map());
  // The highest generation ever seen for every Codex slot. It is never
  // pruned: a slot that leaves the registry must not let an older snapshot
  // bring it back.
  const codexHighWaterRef = useRef<Map<string, number>>(new Map());
  // Removal retires the registration itself, including its last generation.
  // Keep this boundary after the visible entry and identity state are gone.
  const codexRetiredRef = useRef<Map<string, number>>(new Map());
  const lastSeqRef = useRef<number | null>(null);
  const latestSnapshotReadRef = useRef(0);
  // Every accepted event advances the observation generation. A read can
  // only replace an equal-seq projection when that generation did not move
  // while the read was in flight. `seq` alone cannot distinguish an old
  // snapshot from a newer identity projection when Rust has not advanced it
  // yet, so arrival order supplies the missing fence at the IPC boundary.
  const eventGenerationRef = useRef(0);

  const applySnapshot = useCallback(
    (
      next: RuntimeSnapshot,
      source: "event" | "revalidation",
      eventGenerationAtReadStart?: number,
    ) => {
    const previousSeq = lastSeqRef.current;
    // The whole-snapshot order applies to everything that is not a Codex
    // slot. A pull may legitimately refresh identity while Rust keeps the
    // same sequence, so it may do so only when no event crossed the read in
    // flight. Events must always advance the sequence, so an equal-sequence
    // event is never an update.
    const sequenceAdvanced =
      previousSeq === null ||
      (source === "event"
        ? next.seq > previousSeq
        : next.seq > previousSeq ||
          (next.seq === previousSeq &&
            eventGenerationAtReadStart === eventGenerationRef.current));

    // Codex slots are fenced by their own session generation, never by the
    // global sequence. A slot whose generation moved backwards describes an
    // older session and is dropped in favour of the accepted reading; the
    // other slots and every unrelated provider are unaffected.
    const states = next.codexIdentityStates;
    const observed = new Map(
      (states ?? []).map((state) => [state.providerId, state]),
    );
    const accepted = new Map(
      codexSlotsRef.current.map((entry) => [entry.providerId, entry]),
    );
    const highWater = codexHighWaterRef.current;
    const retired = codexRetiredRef.current;
    const observedStates = codexStatesRef.current;
    // A slot the runtime no longer observes cannot keep a reading. Only a
    // snapshot that is not older than the accepted one may retire a slot.
    if (states !== undefined && sequenceAdvanced) {
      for (const providerId of new Set([...accepted.keys(), ...observedStates.keys()])) {
        if (observed.has(providerId)) continue;
        retired.set(providerId, highWater.get(providerId) ?? 0);
        accepted.delete(providerId);
        observedStates.delete(providerId);
      }
    }
    // A slot whose generation advanced belongs to an older session: its
    // accepted reading survives only when this snapshot carries the new
    // one.
    for (const [providerId, state] of observed) {
      const current = accepted.get(providerId);
      if (!current) continue;
      if (state.generation <= current.generation) continue;
      if (!next.providers.some((provider) => provider.id === providerId)) {
        accepted.delete(providerId);
      }
    }
    // The slots this snapshot confirms: every observed slot that is at
    // least as new as the highest generation already seen for it. A
    // delayed snapshot never rewinds the scope, and only the slots the
    // fence actually accepted are confirmed.
    const confirmed: CodexIdentityState[] = [];
    for (const [providerId, state] of observed) {
      if (state.generation <= (retired.get(providerId) ?? -1)) continue;
      const seen = highWater.get(providerId);
      if (seen !== undefined && state.generation < seen) continue;
      highWater.set(providerId, Math.max(seen ?? 0, state.generation));
      observedStates.set(providerId, { ...state });
      confirmed.push(state);
    }
    for (const provider of next.providers) {
      if (!isCodexProviderId(provider.id)) continue;
      if (states === undefined) {
        // A legacy snapshot cannot prove a new registration epoch.
        if (retired.has(provider.id)) continue;
        // A snapshot without the per-slot generations predates that
        // contract, so the whole-snapshot order is the only fence it can
        // offer. It may not replace a newer accepted projection.
        if (!sequenceAdvanced) continue;
        // The slot's known observation is kept: a legacy snapshot may not
        // lower a session the runtime has already reported.
        const known = observedStates.get(provider.id);
        const state: CodexIdentityState = known ?? {
          providerId: provider.id,
          generation: 0,
          identity: provider.account?.identity ?? null,
        };
        if (known === undefined) observedStates.set(provider.id, state);
        // A slot that was ever observed with a real generation keeps it:
        // a legacy snapshot may not replace a newer accepted projection.
        if ((highWater.get(provider.id) ?? 0) <= state.generation) {
          accepted.set(provider.id, {
            providerId: provider.id,
            generation: state.generation,
            identity: state.identity,
            usage: provider,
          });
        }
        // The slot is confirmed by this snapshot: the read that carried it
        // observed the slot's login.
        confirmed.push(state);
        continue;
      }
      const state = observed.get(provider.id);
      if (!state) {
        continue;
      }
      if (state.generation <= (retired.get(provider.id) ?? -1)) continue;
      // The slot's highest observed generation fences the accept. A
      // snapshot that carries an older generation can never reinstate an
      // earlier session - not even after the accepted entry was retired,
      // because the high-water mark is never lowered.
      if (state.generation < (highWater.get(provider.id) ?? 0)) continue;
      const current = accepted.get(provider.id);
      if (current && state.generation < current.generation) continue;
      accepted.set(provider.id, {
        providerId: provider.id,
        generation: state.generation,
        identity: state.identity,
        usage: provider,
      });
    }
    const nextCodexSlots = [...accepted.values()];
    codexSlotsRef.current = nextCodexSlots;
    setCodexSlots(nextCodexSlots);
    setCodexStates([...observedStates.values()]);

    if (!sequenceAdvanced) {
      // The snapshot as a whole is older than the accepted one. Its Codex
      // slots may still be newer - they are ordered by their own
      // generation - but nothing else in it may replace the accepted state.
      return confirmed;
    }
    lastSeqRef.current = next.seq;
    if (source === "event") eventGenerationRef.current += 1;
    setSnapshot(next);
    if (source === "revalidation") {
      // This read is the fence that may confirm Codex again after a failed
      // read; stale events cannot clear it.
      setInitialReadFailed(false);
    }
    if (!next.cycleInFlight) {
      setRequested(false);
      // Wallet jobs run inside the cycle and publish with it. A background
      // identity read can observe an in-flight cycle before that close, and
      // must not hide the wallet action's busy state prematurely.
      setWalletRequested(false);
    }
    return confirmed;
  },
    [],
  );

  const applyEventSnapshot = useCallback(
    (next: RuntimeSnapshot) => applySnapshot(next, "event"),
    [applySnapshot],
  );

  const applyRevalidatedSnapshot = useCallback(
    (next: RuntimeSnapshot, eventGenerationAtReadStart: number) =>
      applySnapshot(next, "revalidation", eventGenerationAtReadStart),
    [applySnapshot],
  );

  const invalidateCodexUsage = useCallback(() => {
    // A failed read proves nothing about any slot, so every slot this hook
    // knows about becomes unconfirmed. Each one is confirmed again by its
    // own later read.
    setUnconfirmedCodexSlots(
      new Set(codexSlotsRef.current.map((entry) => entry.providerId)),
    );
  }, []);

  const confirmCodexSlots = useCallback((confirmed: CodexIdentityState[]) => {
    if (confirmed.length === 0) return;
    const observed = new Set(confirmed.map((state) => state.providerId));
    setUnconfirmedCodexSlots((previous) => {
      if (previous.size === 0) return previous;
      const remaining = new Set(
        [...previous].filter((providerId) => !observed.has(providerId)),
      );
      return remaining.size === previous.size ? previous : remaining;
    });
  }, []);

  useEffect(() => {
    if (!isRunningInTauri()) return;
    let cancelled = false;
    let detach = () => {};
    let revalidationTimer: number | undefined;
    let readInFlight = false;
    let readExpired = false;
    const revalidateSnapshot = async () => {
      if (readInFlight) {
        // A read is already pending. A pending read is not a failure: it
        // confirms the login when it lands, and the last confirmed reading
        // stays visible until then. Only a FAILED read invalidates the
        // Codex surfaces.
        //
        // The in-flight read is still the newest issued read, so it must
        // stay allowed to land. Invalidating it here would discard every
        // read that takes longer than the revalidation interval and leave
        // the Codex surfaces permanently hidden.
        readExpired = true;
        return;
      }
      readInFlight = true;
      readExpired = false;
      const readId = ++latestSnapshotReadRef.current;
      const eventGenerationAtReadStart = eventGenerationRef.current;
      try {
        const next = await invoke<RuntimeSnapshot>("get_runtime_snapshot");
        // Overlapping reads are possible when a local IPC call is delayed.
        // Only the most recently issued read may commit or clear the failure
        // fence; a late response from an older request is untrusted.
        if (cancelled || readId !== latestSnapshotReadRef.current) return;
        // The read completed, so it observed the current login of every
        // slot it carries. Those slots are confirmed again - after the
        // snapshot has been applied, so only the slots the fence actually
        // accepted are confirmed.
        const confirmed = applyRevalidatedSnapshot(next, eventGenerationAtReadStart);
        confirmCodexSlots(confirmed);
      } catch (error) {
        if (cancelled || readId !== latestSnapshotReadRef.current) return;
        setInitialReadFailed(true);
        invalidateCodexUsage();
        console.error("Failed to revalidate the runtime snapshot", error);
      } finally {
        readInFlight = false;
        if (!cancelled && readExpired) void revalidateSnapshot();
      }
    };
    void (async () => {
      // Subscribe first, then pull: a snapshot arriving in between is
      // applied and the pull result is ignored by the seq guard (or
      // vice versa — both orders end on the newest state).
      const unlistenSnapshot = await listen<RuntimeSnapshot>(
        SNAPSHOT_EVENT,
        (event) => applyEventSnapshot(event.payload),
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
      void revalidateSnapshot();
      revalidationTimer = window.setInterval(() => {
        void revalidateSnapshot();
      }, SNAPSHOT_REVALIDATION_MS);
    })();
    return () => {
      cancelled = true;
      latestSnapshotReadRef.current += 1;
      if (revalidationTimer !== undefined) {
        window.clearInterval(revalidationTimer);
      }
      detach();
    };
  }, [
    applyEventSnapshot,
    applyRevalidatedSnapshot,
    confirmCodexSlots,
    invalidateCodexUsage,
  ]);

  // The runtime owns the timer and the settings lane owns the queue that
  // writes it. This hook only runs the hand-off once, so the interval is
  // written exactly like every other preference: ordered against a reset,
  // fenced against a newer edit, and never sent by a second window.
  const onAttach = options?.onAttach;
  const attached = useRef(false);
  useEffect(() => {
    if (attached.current) return;
    attached.current = true;
    if (!isRunningInTauri()) return;
    onAttach?.();
  }, [onAttach]);

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

  // Manual wallet refresh. The Rust runtime coalesces it with the one
  // provider cycle; this hook only marks the wallet action busy.
  const refreshWallet = useCallback(() => {
    if (!isRunningInTauri()) return;
    setWalletRequested(true);
    void invoke("request_wallet_refresh").catch((error) => {
      console.error("Failed to request a wallet refresh", error);
      setWalletRequested(false);
    });
  }, []);

  // Wall clock for the relative "stale" age in the footer.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), CLOCK_TICK_MS);
    return () => window.clearInterval(timer);
  }, []);

  // The visible provider list: unrelated providers come from the accepted
  // snapshot, and every Codex slot from its own accepted reading. A slot
  // that moved is never replaced by an older session, and a slot whose
  // login could not be confirmed is hidden on its own.
  const usages: ProviderUsage[] = [];
  const mergedCodexSlots = new Set<string>();
  // The index the Codex entries occupy, so a slot the accepted snapshot
  // does not carry keeps its registry neighbours instead of moving to the
  // end of the list.
  let lastCodexIndex = -1;
  for (const usage of snapshot?.providers ?? []) {
    if (!isCodexProviderId(usage.id)) {
      usages.push(usage);
      continue;
    }
    const entry = codexSlots.find((slot) => slot.providerId === usage.id);
    if (!entry) continue;
    mergedCodexSlots.add(entry.providerId);
    usages.push(entry.usage);
    lastCodexIndex = usages.length - 1;
  }
  for (const entry of codexSlots) {
    if (mergedCodexSlots.has(entry.providerId)) continue;
    const insertAt = lastCodexIndex < 0 ? usages.length : lastCodexIndex + 1;
    usages.splice(insertAt, 0, entry.usage);
    lastCodexIndex = insertAt;
  }
  const visibleUsages = usages.filter(
    (usage) => !unconfirmedCodexSlots.has(usage.id),
  );
  const wallets = snapshot?.wallets ?? [];
  // Loading spans a whole chain: from the request (or the runtime's
  // cycle-started event) until the closing snapshot of the chain.
  // A failed first pull must end the initial spinner. Periodic revalidation
  // failures leave an already-loaded dashboard in its current loading state.
  const loading =
    snapshot === null
      ? isRunningInTauri() && !initialReadFailed
      : snapshot.cycleInFlight || requested;
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
  // runtime cycle, or from attach if none has happened yet. The runtime owns
  // the cadence, so its own snapshot decides the threshold - a window's copy
  // of the preference could still be an unapplied edit.
  const runtimeIntervalMinutes = snapshot?.refreshIntervalMinutes;
  const effectiveIntervalMinutes =
    typeof runtimeIntervalMinutes === "number" && runtimeIntervalMinutes > 0
      ? runtimeIntervalMinutes
      : DEFAULT_REFRESH_INTERVAL_MINUTES;
  const staleAfterMs = effectiveIntervalMinutes * 2 * 60 * 1000;
  const referenceAt = lastUpdatedAt?.getTime() ?? attachAt;
  const staleMinutes = Math.max(0, Math.floor((now - referenceAt) / 60000));
  const stale = now - referenceAt > staleAfterMs;

  return {
    usages: visibleUsages,
    // The runtime's current per-slot observation. A consumer that reads
    // user-bound history uses it to tell a current projection from one
    // that belongs to an older session.
    codexIdentityStates: codexStates,
    wallets,
    loading,
    lastUpdatedAt,
    refresh,
    refreshWallet,
    walletRefreshing: walletRequested,
    refreshOverdue,
    stale,
    staleMinutes,
    historyRevision,
    usageIntelligenceRevision,
  };
}
