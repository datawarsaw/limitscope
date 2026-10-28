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
  return {
    invocations,
    snapshotHandlerState,
    cycleStartedState,
    invoke: vi.fn(async (command: string, args?: unknown) => {
      invocations.push({ command, args });
      if (command === "get_runtime_snapshot") return currentSnapshot();
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

beforeEach(() => {
  // The hook is a consumer of the Tauri runtime; simulate it (the hook's
  // environment probe checks this marker before touching the bridge).
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  mocks.invocations.length = 0;
  mocks.snapshotHandlerState.handler = null;
  mocks.cycleStartedState.handler = null;
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
    const { result } = renderHook(() => useProviderUsage(5));
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.usages).toHaveLength(1);
    const commands = mocks.invocations.map((call) => call.command);
    expect(commands).toContain("get_runtime_snapshot");
    expect(commands).toContain("set_refresh_interval");
    // Fetching is Rust-owned: no provider command may ever come from TS.
    expect(commands.filter((c) => c.startsWith("get_") && c !== "get_runtime_snapshot")).toEqual([]);
    expect(result.current.refreshOverdue).toBe(false);
    expect(result.current.lastUpdatedAt).toEqual(new Date("2026-09-29T12:00:00.000Z"));
  });

  it("applies runtime snapshot events and closes the loading span", async () => {
    const { result } = renderHook(() => useProviderUsage(5));
    await waitFor(() => expect(result.current.loading).toBe(false));
    act(() => mocks.cycleStartedState.handler?.());
    expect(result.current.loading).toBe(true);
    emitSnapshot(snapshotWith({ seq: 2, providers: [] }));
    expect(result.current.loading).toBe(false);
    expect(result.current.usages).toHaveLength(0);
  });

  it("ignores snapshots older than the newest applied one (seq guard)", async () => {
    const { result } = renderHook(() => useProviderUsage(5));
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
      const { result } = renderHook(() => useProviderUsage(5));
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
    const { result } = renderHook(() => useProviderUsage(5));
    await waitFor(() => expect(result.current.loading).toBe(false));
    act(() => result.current.refresh());
    expect(result.current.loading).toBe(true);
    expect(mocks.invocations.filter((c) => c.command === "request_refresh")).toHaveLength(1);
    // The closing snapshot of the chain clears the optimistic span.
    emitSnapshot(snapshotWith({ seq: 2 }));
    expect(result.current.loading).toBe(false);
  });

  it("refreshes a wallet through its own busy state and applies its snapshot", async () => {
    const { result } = renderHook(() => useProviderUsage(5));
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
    const { result } = renderHook(() => useProviderUsage(5));
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

  it("forwards the persisted refresh interval on attach and on change", async () => {
    const { rerender } = renderHook(({ minutes }) => useProviderUsage(minutes), {
      initialProps: { minutes: 5 },
    });
    await waitFor(() =>
      expect(mocks.invocations.some((c) => c.command === "set_refresh_interval")).toBe(true),
    );
    rerender({ minutes: 15 });
    await waitFor(() =>
      expect(
        mocks.invocations.some((c) => c.command === "set_refresh_interval" && c.args !== undefined),
      ).toBe(true),
    );
    const intervalCalls = mocks.invocations.filter((c) => c.command === "set_refresh_interval");
    expect(intervalCalls.at(-1)?.args).toEqual({ minutes: 15 });
  });

  it("exposes the runtime's history revision, null before the first snapshot", async () => {
    const { result } = renderHook(() => useProviderUsage(5));
    expect(result.current.historyRevision).toBeNull();
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.historyRevision).toBe(0);
    emitSnapshot(snapshotWith({ seq: 2, historyRevision: 7 }));
    expect(result.current.historyRevision).toBe(7);
  });

  it("projects staleness from the runtime's last successful cycle", async () => {
    vi.useFakeTimers({ now: Date.parse("2026-09-29T13:00:00.000Z") });
    try {
      const { result } = renderHook(() => useProviderUsage(5));
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
