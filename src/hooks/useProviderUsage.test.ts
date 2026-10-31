// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ProviderUsage, RuntimeSnapshot } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";

/**
 * Consumer-contract tests for the shared-runtime hook: the hook must only
 * ever pull the snapshot, subscribe to runtime events, forward the interval,
 * and request refreshes through the runtime — it must never invoke a
 * provider command (fetching is Rust-owned since the v0.5 shared runtime).
 */

const mocks = vi.hoisted(() => {
  const invocations: { command: string; args?: unknown }[] = [];
  const snapshotHandlerState = {
    handler: null as ((payload: RuntimeSnapshot) => void) | null,
  };
  const cycleStartedState = { handler: null as (() => void) | null };
  const snapshotResponses: Array<
    () => RuntimeSnapshot | Promise<RuntimeSnapshot>
  > = [];
  return {
    invocations,
    snapshotHandlerState,
    cycleStartedState,
    snapshotResponses,
    invoke: vi.fn(async (command: string, args?: unknown) => {
      invocations.push({ command, args });
      if (command === "get_runtime_snapshot") {
        const response = snapshotResponses.shift();
        return response ? await response() : currentSnapshot();
      }
      return undefined;
    }),
    listen: vi.fn(async (event: string, handler: (event: { payload: unknown }) => void) => {
      if (event === "runtime://snapshot") {
        snapshotHandlerState.handler = (payload: RuntimeSnapshot) =>
          handler({ payload });
      }
      if (event === "runtime://cycle-started") {
        cycleStartedState.handler = () => handler({ payload: null });
      }
      return async () => {};
    }),
  };
});

function currentSnapshot(): RuntimeSnapshot {
  return {
    seq: 1,
    providers: [
      {
        id: "zai",
        name: "Z.ai",
        status: "ok",
        health: "live",
        checkedAt: "2026-09-29T12:00:00.000Z",
        limits: [{ label: "5-hour", usedPercent: 42 }],
      },
    ],
    wallets: [],
    lastUpdatedAt: "2026-09-29T12:00:00.000Z",
    cycleSucceeded: true,
    cycleInFlight: false,
    refreshIntervalMinutes: 5,
    historyRevision: 0,
    usageIntelligenceRevision: 0,
  };
}

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

import { useProviderUsage } from "./useProviderUsage";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

function emitSnapshot(snapshot: RuntimeSnapshot) {
  act(() => mocks.snapshotHandlerState.handler?.(snapshot));
}

function snapshotWith(overrides: Partial<RuntimeSnapshot>): RuntimeSnapshot {
  return { ...currentSnapshot(), ...overrides };
}

function codexUsage(usedPercent: number, identity: string): ProviderUsage {
  return {
    id: "openai-codex",
    name: "OpenAI / Codex",
    status: "ok",
    health: "live",
    checkedAt: "2026-10-28T12:00:00.000Z",
    limits: [{ label: "Weekly", usedPercent }],
    account: { label: identity, identity },
  };
}

/**
 * The runtime's per-slot observation of one Codex slot. The generation
 * advances on every observed change of that slot's identity, so a consumer
 * can order two captures of the same slot without relying on the global
 * sequence.
 */
function codexState(
  providerId: string,
  generation: number,
  identity: string,
) {
  return { providerId, generation, identity };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  // The hook is a consumer of the Tauri runtime; simulate it (the hook's
  // environment probe checks this marker before touching the bridge).
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  mocks.invocations.length = 0;
  mocks.snapshotHandlerState.handler = null;
  mocks.cycleStartedState.handler = null;
  mocks.snapshotResponses.length = 0;
  mocks.invoke.mockClear();
  mocks.listen.mockClear();
});

afterEach(() => {
  // Vitest globals are off, so @testing-library/react's automatic cleanup
  // never registers — unmount explicitly, or mounted hooks from earlier
  // tests keep their window listeners (e.g. the `online` trigger) alive.
  cleanup();
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  document.body.innerHTML = "";
});

describe("useProviderUsage (shared runtime consumer)", () => {
  it("pulls the snapshot once on attach and never invokes provider commands", async () => {
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.usages).toHaveLength(1);
    const commands = mocks.invocations.map((call) => call.command);
    expect(commands).toContain("get_runtime_snapshot");
    // The cadence belongs to the settings lane: this hook never writes a
    // preference, so no window can push a stale copy into the runtime.
    expect(commands).not.toContain("set_refresh_interval");
    // Fetching is Rust-owned: no provider command may ever come from TS.
    expect(commands.filter((c) => c.startsWith("get_") && c !== "get_runtime_snapshot")).toEqual([]);
    expect(result.current.refreshOverdue).toBe(false);
    expect(result.current.lastUpdatedAt).toEqual(new Date("2026-09-29T12:00:00.000Z"));
  });

  it("periodically re-reads the runtime projection without requesting collection", async () => {
    vi.useFakeTimers();
    try {
      const { result } = renderHook(() => useProviderUsage());
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(result.current.loading).toBe(false);
      mocks.invocations.length = 0;

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });

      const commands = mocks.invocations.map((call) => call.command);
      expect(commands.filter((command) => command === "get_runtime_snapshot")).toHaveLength(1);
      expect(commands).not.toContain("request_refresh");
      expect(commands.filter((command) => command.startsWith("get_"))).toEqual([
        "get_runtime_snapshot",
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("accepts an equal-seq revalidation but lets only the newest read and a strictly newer event win", async () => {
    vi.useFakeTimers();
    try {
      const olderRead = deferred<RuntimeSnapshot>();
      const newestRead = deferred<RuntimeSnapshot>();
      const zai = currentSnapshot().providers[0];
      mocks.snapshotResponses.push(
        () => snapshotWith({
          providers: [codexUsage(14, "account-a"), zai],
          codexIdentityStates: [codexState("openai-codex", 5, "account-a")],
        }),
        () => olderRead.promise,
        () => newestRead.promise,
      );
      const { result } = renderHook(() => useProviderUsage());
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      // A stalled read is not a failure: the last confirmed reading stays
      // visible while the read is pending.
      expect(result.current.usages.some((usage) => usage.id === "openai-codex")).toBe(true);
      await act(async () => {
        olderRead.resolve(
          snapshotWith({
            providers: [codexUsage(14, "account-a"), zai],
            codexIdentityStates: [codexState("openai-codex", 5, "account-a")],
          }),
        );
        await olderRead.promise;
      });
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);
      await act(async () => {
        newestRead.resolve(
          snapshotWith({
            providers: [codexUsage(81, "account-b"), zai],
            codexIdentityStates: [codexState("openai-codex", 6, "account-b")],
          }),
        );
        await newestRead.promise;
      });
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(81);

      // A delayed event carrying the old equal-seq projection is also stale.
      emitSnapshot(snapshotWith({
        providers: [codexUsage(14, "account-a"), zai],
        codexIdentityStates: [codexState("openai-codex", 5, "account-a")],
      }));
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(81);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps an equal-seq event ahead of the initial pull that crossed it", async () => {
    const delayedInitialRead = deferred<RuntimeSnapshot>();
    const zai = currentSnapshot().providers[0];
    mocks.snapshotResponses.push(() => delayedInitialRead.promise);
    const { result } = renderHook(() => useProviderUsage());

    // Production order is listener first, pull second. The mock invoke is
    // already pending when the event arrives, so the response is genuinely
    // older than the event even though both identify it with seq 1.
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(mocks.snapshotHandlerState.handler).not.toBeNull();
    expect(
      mocks.invocations.filter((call) => call.command === "get_runtime_snapshot"),
    ).toHaveLength(1);

    emitSnapshot(
      snapshotWith({ providers: [codexUsage(81, "account-b"), zai] }),
    );
    expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(81);

    await act(async () => {
      delayedInitialRead.resolve(
        snapshotWith({ providers: [codexUsage(14, "account-a"), zai] }),
      );
      await delayedInitialRead.promise;
    });
    expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(81);
    expect(result.current.usages[1]?.id).toBe("zai");
  });

  it("rejects an equal-seq event after the initial snapshot", async () => {
    const zai = currentSnapshot().providers[0];
    mocks.snapshotResponses.push(() =>
      snapshotWith({ providers: [codexUsage(14, "account-a"), zai] }),
    );
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);

    emitSnapshot(
      snapshotWith({ providers: [codexUsage(81, "account-b"), zai] }),
    );
    expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);
  });

  it("fences an equal-seq read and older event after a newer identity event", async () => {
    vi.useFakeTimers();
    try {
      const delayedRead = deferred<RuntimeSnapshot>();
      const zai = currentSnapshot().providers[0];
      mocks.snapshotResponses.push(
        () => snapshotWith({ providers: [codexUsage(14, "account-a"), zai] }),
        () => delayedRead.promise,
      );
      const { result } = renderHook(() => useProviderUsage());
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });
      expect(
        mocks.invocations.filter((call) => call.command === "get_runtime_snapshot"),
      ).toHaveLength(2);

      emitSnapshot(snapshotWith({
        seq: 2,
        providers: [codexUsage(81, "account-b"), zai],
      }));
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(81);

      // This response has the event's seq, but its request began before the
      // event. Its payload still belongs to A and must not cross the fence.
      await act(async () => {
        delayedRead.resolve(snapshotWith({
          seq: 2,
          providers: [codexUsage(14, "account-a"), zai],
        }));
        await delayedRead.promise;
      });
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(81);

      emitSnapshot(snapshotWith({
        seq: 1,
        providers: [codexUsage(14, "account-a"), zai],
      }));
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(81);
      expect(result.current.usages[1]?.id).toBe("zai");
    } finally {
      vi.useRealTimers();
    }
  });

  it("applies runtime snapshot events and closes the loading span", async () => {
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    act(() => mocks.cycleStartedState.handler?.());
    expect(result.current.loading).toBe(true);
    emitSnapshot(snapshotWith({ seq: 2, providers: [] }));
    expect(result.current.loading).toBe(false);
    expect(result.current.usages).toHaveLength(0);
  });

  it("ignores snapshots older than the newest applied one (seq guard)", async () => {
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    emitSnapshot(snapshotWith({ seq: 3, providers: [], cycleSucceeded: false }));
    expect(result.current.usages).toHaveLength(0);
    // A late pull/event with an older seq must not roll state back.
    emitSnapshot(snapshotWith({ seq: 2, cycleSucceeded: true }));
    expect(result.current.refreshOverdue).toBe(true);
  });

  it.each(["error", "cooldown", "stale", "paused"] as const)(
    "keeps each Codex account's retained reading isolated through a %s snapshot",
    async (state) => {
      const { result } = renderHook(() => useProviderUsage());
      await waitFor(() => expect(result.current.loading).toBe(false));
      const ids = ["openai-codex", "openai-codex:secondary"];
      for (const affected of [0, 1]) {
        const providers: ProviderUsage[] = ids.map((id, index) => ({
          id,
          name: "OpenAI / Codex",
          status: "ok",
          health: "live",
          checkedAt: `2026-10-28T12:0${index}:00.000Z`,
          limits: [{ label: "Weekly", usedPercent: [14, 81][index] }],
          accountAlias: ["Personal", "Work"][index],
          account: { label: `Account ${index}`, identity: `chatgpt:fixture-${index}` },
        }));
        Object.assign(providers[affected], state === "paused"
          ? { monitoringPaused: true }
          : {
              health: state,
              status: state === "stale" ? "stale" : "error",
              ...(state === "stale"
                ? { dataFreshness: "stale" }
                : { error: "Refresh failed: fixture", errorHttpStatus: 429 }),
            });
        emitSnapshot(snapshotWith({ seq: affected + 2, providers }));
        expect(result.current.usages).toEqual(providers);
        const items = floatingQuotaItems(result.current.usages);
        expect(items.map((item) => item.providerId)).toEqual(ids);
        expect(items.map((item) => item.percent)).toEqual([14, 81]);
        expect(items.map((item) => item.accountAlias)).toEqual(["Personal", "Work"]);
        expect(items[affected].state).toBe(state === "cooldown" ? "error" : state);
        expect(items[1 - affected].state).toBe("healthy");
      }
    },
  );

  it("refresh requests go to the runtime, not to provider adapters", async () => {
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    act(() => result.current.refresh());
    expect(result.current.loading).toBe(true);
    expect(mocks.invocations.filter((c) => c.command === "request_refresh")).toHaveLength(1);
    // The closing snapshot of the chain clears the optimistic span.
    emitSnapshot(snapshotWith({ seq: 2 }));
    expect(result.current.loading).toBe(false);
  });

  it("invalidates Codex immediately on a revalidation error and preserves other providers", async () => {
    vi.useFakeTimers();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const zai = currentSnapshot().providers[0];
      const wallet = {
        id: "openrouter",
        name: "OpenRouter",
        health: "live" as const,
        checkedAt: "2026-10-28T12:00:00.000Z",
        balances: [{ currency: "USD", amount: 12.5 }],
      };
      mocks.snapshotResponses.push(
        () => snapshotWith({ providers: [codexUsage(14, "account-a"), zai], wallets: [wallet] }),
        () => Promise.reject(new Error("snapshot read failed")),
        () => snapshotWith({
          seq: 2,
          providers: [codexUsage(81, "account-b"), zai],
          wallets: [wallet],
        }),
      );
      const { result } = renderHook(() => useProviderUsage());
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(result.current.usages.map((usage) => usage.id)).toEqual([
        "openai-codex",
        "zai",
      ]);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });
      expect(result.current.usages.map((usage) => usage.id)).toEqual(["zai"]);
      expect(result.current.wallets).toEqual([wallet]);
      expect(result.current.loading).toBe(false);

      // Even a newer event cannot clear an invalidation caused by a failed
      // read; only a successful get_runtime_snapshot may do that.
      emitSnapshot(snapshotWith({
        seq: 2,
        providers: [codexUsage(14, "account-a"), zai],
        wallets: [wallet],
      }));
      expect(result.current.usages.map((usage) => usage.id)).toEqual(["zai"]);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });
      expect(result.current.usages.map((usage) => usage.id)).toEqual([
        "openai-codex",
        "zai",
      ]);
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(81);
      expect(
        mocks.invocations.filter((call) => call.command === "request_refresh"),
      ).toHaveLength(0);
    } finally {
      consoleError.mockRestore();
      vi.useRealTimers();
    }
  });

  it("keeps two Codex slots in different sessions independent", async () => {
    const zai = currentSnapshot().providers[0];
    const slot = (id: string, percent: number, identity: string): ProviderUsage => ({
      ...codexUsage(percent, identity),
      id,
    });
    mocks.snapshotResponses.push(
      () => snapshotWith({
        seq: 1,
        providers: [slot("openai-codex", 14, "user-a"), slot("openai-codex:secondary", 81, "user-b"), zai],
        codexIdentityStates: [
          codexState("openai-codex", 5, "user-a"),
          codexState("openai-codex:secondary", 3, "user-b"),
        ],
      }),
    );
    const { result } = renderHook(() => useProviderUsage());
 await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.usages.map((usage) => usage.id)).toEqual([
      "openai-codex",
      "openai-codex:secondary",
      "zai",
    ]);

    // Only the primary slot moves to another session. The secondary slot
    // keeps its own reading, and the unrelated provider is untouched.
    emitSnapshot(snapshotWith({
      seq: 2,
      providers: [slot("openai-codex", 38, "user-c"), slot("openai-codex:secondary", 81, "user-b"), zai],
      codexIdentityStates: [
        codexState("openai-codex", 6, "user-c"),
        codexState("openai-codex:secondary", 3, "user-b"),
      ],
    }));
    const byId = new Map(result.current.usages.map((usage) => [usage.id, usage]));
    expect(byId.get("openai-codex")?.limits[0]?.usedPercent).toBe(38);
    expect(byId.get("openai-codex:secondary")?.limits[0]?.usedPercent).toBe(81);
    expect(byId.get("zai")?.limits[0]?.usedPercent).toBe(42);
  });

  it("fails closed for one slot whose identity is missing, leaving the other slot visible", async () => {
    const zai = currentSnapshot().providers[0];
    const slot = (id: string, percent: number, identity: string): ProviderUsage => ({
      ...codexUsage(percent, identity),
      id,
    });
    mocks.snapshotResponses.push(
      () => snapshotWith({
        seq: 1,
        providers: [slot("openai-codex", 14, "user-a"), slot("openai-codex:secondary", 81, "user-b"), zai],
        codexIdentityStates: [
          codexState("openai-codex", 5, "user-a"),
          codexState("openai-codex:secondary", 3, "user-b"),
        ],
      }),
    );
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));

    // The runtime observed that the secondary slot has no usable login. It
    // blanks that slot on its own; the primary slot is unaffected.
    emitSnapshot(snapshotWith({
      seq: 2,
      providers: [slot("openai-codex", 14, "user-a"), zai],
      codexIdentityStates: [
        codexState("openai-codex", 5, "user-a"),
        { providerId: "openai-codex:secondary", generation: 4, identity: null },
      ],
    }));
    expect(result.current.usages.map((usage) => usage.id)).toEqual([
      "openai-codex",
      "zai",
    ]);
  });

  it("does not revive an earlier session after an A to B to A move", async () => {
    const zai = currentSnapshot().providers[0];
    mocks.snapshotResponses.push(
      () => snapshotWith({
        seq: 1,
        providers: [codexUsage(14, "user-a"), zai],
        codexIdentityStates: [codexState("openai-codex", 5, "user-a")],
      }),
    );
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);

    // A -> B.
    emitSnapshot(snapshotWith({
      seq: 2,
      providers: [codexUsage(38, "user-b"), zai],
      codexIdentityStates: [codexState("openai-codex", 6, "user-b")],
    }));
    expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(38);

    // B -> A. The generation advanced twice, so A's earlier reading is not
    // the current one.
    emitSnapshot(snapshotWith({
      seq: 3,
      providers: [codexUsage(14, "user-a"), zai],
      codexIdentityStates: [codexState("openai-codex", 7, "user-a")],
    }));
    expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);

    // A delayed response from the FIRST A session carries generation 5. It
    // must not replace the accepted generation 7.
    emitSnapshot(snapshotWith({
      seq: 4,
      providers: [codexUsage(99, "user-a"), zai],
      codexIdentityStates: [codexState("openai-codex", 5, "user-a")],
    }));
    expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);
  });

  it("accepts a Codex slot that moved even when an unrelated provider moved the sequence", async () => {
    vi.useFakeTimers();
    try {
      const zai = currentSnapshot().providers[0];
      const delayedRead = deferred<RuntimeSnapshot>();
      mocks.snapshotResponses.push(
        () => snapshotWith({
          seq: 1,
          providers: [codexUsage(14, "user-a"), zai],
          codexIdentityStates: [codexState("openai-codex", 5, "user-a")],
        }),
        () => delayedRead.promise,
      );
      const { result } = renderHook(() => useProviderUsage());
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);

      // An unrelated provider advances the global sequence.
      emitSnapshot(snapshotWith({
        seq: 2,
        providers: [{ ...codexUsage(14, "user-a"), health: "live" }, zai],
        codexIdentityStates: [codexState("openai-codex", 5, "user-a")],
      }));
      expect(
        result.current.usages.find((usage) => usage.id === "openai-codex")?.limits[0]?.usedPercent,
      ).toBe(14);

      // The pending read observed a NEW Codex session, but its global
      // sequence is older than the unrelated provider's. The Codex slot is
      // ordered by its own generation, so it is accepted.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
        delayedRead.resolve(snapshotWith({
          seq: 1,
          providers: [codexUsage(38, "user-b"), zai],
          codexIdentityStates: [codexState("openai-codex", 6, "user-b")],
        }));
        await delayedRead.promise;
      });
      expect(
        result.current.usages.find((usage) => usage.id === "openai-codex")?.limits[0]?.usedPercent,
      ).toBe(38);
    } finally {
      vi.useRealTimers();
    }
  });

  it("lands a read that takes longer than the revalidation interval", async () => {
    vi.useFakeTimers();
    try {
      const slowRead = deferred<RuntimeSnapshot>();
      const followUp = deferred<RuntimeSnapshot>();
      const zai = currentSnapshot().providers[0];
      mocks.snapshotResponses.push(
        () => snapshotWith({
          providers: [codexUsage(14, "user-a"), zai],
          codexIdentityStates: [codexState("openai-codex", 5, "user-a")],
        }),
        () => slowRead.promise,
        () => followUp.promise,
      );
      const { result } = renderHook(() => useProviderUsage());
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(14);

      // The revalidation tick fires while the read is still in flight. The
      // pending read is the newest issued read, so it must still be allowed
      // to land: otherwise every read that outlives the interval is
      // discarded and the Codex surfaces stay blank forever.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      await act(async () => {
        slowRead.resolve(snapshotWith({
          seq: 2,
          providers: [codexUsage(38, "user-b"), zai],
          codexIdentityStates: [codexState("openai-codex", 6, "user-b")],
        }));
        await slowRead.promise;
      });
      expect(result.current.usages[0]?.limits[0]?.usedPercent).toBe(38);
      expect(result.current.loading).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("never reinstates an earlier session after the accepted entry was retired", async () => {
    const zai = currentSnapshot().providers[0];
    mocks.snapshotResponses.push(
      () => snapshotWith({
        seq: 1,
        providers: [codexUsage(14, "user-a"), zai],
        codexIdentityStates: [codexState("openai-codex", 5, "user-a")],
      }),
    );
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(
      result.current.usages.find((usage) => usage.id === "openai-codex")?.limits[0]?.usedPercent,
    ).toBe(14);

    // The slot's session moved to another user and the runtime no longer
    // carries a reading for it, so the accepted entry is retired.
    emitSnapshot(snapshotWith({
      seq: 2,
      providers: [zai],
      codexIdentityStates: [codexState("openai-codex", 6, "user-b")],
    }));
    expect(
      result.current.usages.find((usage) => usage.id === "openai-codex"),
    ).toBeUndefined();

    // A late snapshot from the earlier session must not reinstate it: the
    // slot's highest observed generation fences the accept.
    emitSnapshot(snapshotWith({
      seq: 1,
      providers: [codexUsage(14, "user-a"), zai],
      codexIdentityStates: [codexState("openai-codex", 5, "user-a")],
    }));
    expect(
      result.current.usages.find((usage) => usage.id === "openai-codex"),
    ).toBeUndefined();
  });

  it("never resurrects a removed slot from a delayed same-generation event", async () => {
    const zai = currentSnapshot().providers[0];
    const primary = codexUsage(38, "primary-user");
    const secondary = { ...codexUsage(14, "user-a"), id: "openai-codex:secondary" };
    const primaryState = codexState(primary.id, 3, "primary-user");
    const original = snapshotWith({
      seq: 1,
      providers: [primary, secondary, zai],
      codexIdentityStates: [primaryState, codexState(secondary.id, 5, "user-a")],
    });
    mocks.snapshotResponses.push(() => original);
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.usages.find((usage) => usage.id === secondary.id)?.limits[0]?.usedPercent).toBe(14);

    emitSnapshot(snapshotWith({
      seq: 2,
      providers: [primary, zai],
      codexIdentityStates: [primaryState],
    }));
    expect(result.current.usages.map((usage) => usage.id)).toEqual([primary.id, zai.id]);
    emitSnapshot(original);
    expect(result.current.usages.map((usage) => usage.id)).toEqual([primary.id, zai.id]);
    expect(result.current.codexIdentityStates).toEqual([primaryState]);

    // Re-registering the same user has a new lifecycle generation.
    emitSnapshot(snapshotWith({
      seq: 3,
      providers: [primary, { ...secondary, limits: [{ label: "Weekly", usedPercent: 21 }] }, zai],
      codexIdentityStates: [primaryState, codexState(secondary.id, 6, "user-a")],
    }));
    expect(result.current.usages.find((usage) => usage.id === secondary.id)?.limits[0]?.usedPercent).toBe(21);
    emitSnapshot(original);
    expect(result.current.usages.find((usage) => usage.id === secondary.id)?.limits[0]?.usedPercent).toBe(21);

    // Ordinary refreshes keep their generation and remain eligible.
    emitSnapshot(snapshotWith({
      seq: 4,
      providers: [primary, { ...secondary, limits: [{ label: "Weekly", usedPercent: 27 }] }, { ...zai, limits: [{ label: "5-hour", usedPercent: 49 }] }],
      codexIdentityStates: [primaryState, codexState(secondary.id, 6, "user-a")],
    }));
    expect(result.current.usages.find((usage) => usage.id === secondary.id)?.limits[0]?.usedPercent).toBe(27);
    expect(result.current.usages.find((usage) => usage.id === primary.id)?.limits[0]?.usedPercent).toBe(38);
    expect(result.current.usages.find((usage) => usage.id === zai.id)?.limits[0]?.usedPercent).toBe(49);
  });

  it("ends initial loading when the first snapshot read fails", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      mocks.snapshotResponses.push(() =>
        Promise.reject(new Error("snapshot read failed")),
      );
      const { result } = renderHook(() => useProviderUsage());
      await waitFor(() => expect(result.current.loading).toBe(false));
      expect(result.current.usages).toEqual([]);
    } finally {
      consoleError.mockRestore();
    }
  });

  it("refreshes a wallet through its own busy state and applies its snapshot", async () => {
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.wallets).toEqual([]);

    act(() => result.current.refreshWallet());
    expect(result.current.walletRefreshing).toBe(true);
    // The provider loading span is untouched: a wallet action is its own.
    expect(result.current.loading).toBe(false);
    expect(
      mocks.invocations.filter((c) => c.command === "request_wallet_refresh"),
    ).toHaveLength(1);

    emitSnapshot(
      snapshotWith({
        seq: 2,
        wallets: [
          {
            id: "openrouter",
            name: "OpenRouter",
            health: "live",
            checkedAt: "2026-09-29T12:00:00.000Z",
            balances: [{ currency: "USD", amount: 12.5 }],
          },
        ],
      }),
    );
    expect(result.current.walletRefreshing).toBe(false);
    expect(result.current.wallets).toHaveLength(1);
    const [wallet] = result.current.wallets;
    expect(wallet.balances).toEqual([{ currency: "USD", amount: 12.5 }]);
  });

  it("the reconnect trigger only asks the shared runtime for one cycle", async () => {
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    mocks.invocations.length = 0;
    // Both webviews run this hook, so bursts are expected — each `online`
    // event simply triggers the command; the Rust runtime coalesces them.
    act(() => {
      window.dispatchEvent(new Event("online"));
      window.dispatchEvent(new Event("online"));
    });
    const commands = mocks.invocations.map((call) => call.command);
    expect(commands).toEqual([
      "request_refresh_on_reconnect",
      "request_refresh_on_reconnect",
    ]);
    // The trigger never fetches or touches provider state itself.
    expect(result.current.usages).toHaveLength(1);
  });

  it("runs the attach hand-off exactly once and never writes a preference", async () => {
    const onAttach = vi.fn();
    const { rerender } = renderHook(() => useProviderUsage({ onAttach }));
    await waitFor(() => expect(onAttach).toHaveBeenCalledTimes(1));
    rerender();
    await waitFor(() => expect(onAttach).toHaveBeenCalledTimes(1));
    // The hand-off is the settings lane's job: the hook writes nothing, so a
    // cadence can never land outside the queue that orders it against a
    // reset or a newer edit.
    expect(
      mocks.invocations.some((call) => call.command.startsWith("set_")),
    ).toBe(false);
  });

  it("stays read-only when no attach hand-off is given", async () => {
    const { result } = renderHook(() => useProviderUsage());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(
      mocks.invocations.some((call) => call.command.startsWith("set_")),
    ).toBe(false);
  });

  it("exposes the runtime's history revision, null before the first snapshot", async () => {
    const { result } = renderHook(() => useProviderUsage());
    expect(result.current.historyRevision).toBeNull();
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.historyRevision).toBe(0);
    emitSnapshot(snapshotWith({ seq: 2, historyRevision: 7 }));
    expect(result.current.historyRevision).toBe(7);
  });

  it("projects staleness from the runtime's last successful cycle", async () => {
    vi.useFakeTimers({ now: Date.parse("2026-09-29T13:00:00.000Z") });
    try {
      const { result } = renderHook(() => useProviderUsage());
      // Flush the subscribe+pull microtasks (fake timers keep waitFor's
      // intervals from advancing).
      await act(async () => {});
      expect(result.current.loading).toBe(false);
      // lastUpdatedAt is 1h ago; 5 min interval → stale threshold 10 min.
      expect(result.current.stale).toBe(true);
      expect(result.current.staleMinutes).toBe(60);
    } finally {
      vi.useRealTimers();
    }
  });
});
