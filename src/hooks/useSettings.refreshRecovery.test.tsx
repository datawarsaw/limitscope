// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { useSettings } from "./useSettings";
import { SETTINGS_STORAGE_KEY } from "../lib/settings";
import type { CodexAccountSettings } from "../lib/codexAccounts";
import type { LocalDataClearResult } from "../lib/localData";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";

/**
 * The refresh interval is the one preference the runtime owns as a timer, so
 * its write path has to survive a reset, a concurrent edit, and a failed
 * write. These tests pin the three invariants:
 *
 * - an explicit edit is applied after a reset that was still pending, never
 *   before it;
 * - a failed write recovers the canonical value, and stays explicitly
 *   unknown only when the recovery read fails too;
 * - an operational recovery never overwrites a newer operational edit.
 */

type OperationalSettings = {
  revision: number;
  quotaNotifications: boolean;
  monitored: Record<string, boolean>;
};

type Gate = { promise: Promise<void>; release: () => void };

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/plugin-autostart", () => ({
  isEnabled: vi.fn().mockResolvedValue(false),
  enable: vi.fn().mockResolvedValue(undefined),
  disable: vi.fn().mockResolvedValue(undefined),
}));

const originalAccounts: CodexAccountSettings[] = [
  {
    accountId: "openai-codex",
    alias: "Main",
    codexHome: "C:\\fixture\\main",
    monitored: true,
  },
];

let rustAccounts: CodexAccountSettings[];
let rustUsage: boolean;
let rustOperational: OperationalSettings;
let rustRefreshInterval: number;
let successfulWrites: Array<{ command: string; args: unknown }>;
let reached: string[];
const failures = new Map<string, number>();
const gates = new Map<string, Gate>();
// A write that reaches storage and only then rejects: the value is committed
// even though the caller sees a failure.
const rejectAfterPersist = new Set<string>();
// A write that stores a value other than the requested one.
const persistOverride = new Map<string, unknown>();

function failNext(command: string): void {
  failures.set(command, (failures.get(command) ?? 0) + 1);
}

function gate(command: string): Gate {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  const value: Gate = { promise, release };
  gates.set(command, value);
  return value;
}

function stored() {
  return JSON.parse(localStorage.getItem(SETTINGS_STORAGE_KEY)!);
}

async function hydrated(result: {
  current: ReturnType<typeof useSettings>;
}): Promise<void> {
  await waitFor(() => {
    expect(result.current.settings.codexAccounts.accounts).toEqual(originalAccounts);
    expect(result.current.settings.usageIntelligence).toBe(true);
    expect(result.current.settings.quotaNotifications).toBe(true);
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(console, "error").mockImplementation(() => {});
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  localStorage.clear();
  vi.mocked(isEnabled).mockResolvedValue(false);
  vi.mocked(enable).mockResolvedValue(undefined);
  vi.mocked(disable).mockResolvedValue(undefined);
  rustAccounts = structuredClone(originalAccounts);
  rustUsage = true;
  rustOperational = {
    revision: 0,
    quotaNotifications: true,
    monitored: { openrouter: false, deepseek: false },
  };
  rustRefreshInterval = 5;
  successfulWrites = [];
  reached = [];
  failures.clear();
  gates.clear();
  rejectAfterPersist.clear();
  persistOverride.clear();

  mocks.invoke.mockReset();
  mocks.invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
    reached.push(command);
    const remainingFailures = failures.get(command) ?? 0;
    if (remainingFailures > 0) {
      failures.set(command, remainingFailures - 1);
      throw new Error(`${command} failed`);
    }
    const pending = gates.get(command);
    if (pending) {
      gates.delete(command);
      await pending.promise;
    }
    const override = persistOverride.get(command);
    persistOverride.delete(command);
    switch (command) {
      case "get_codex_accounts":
        return structuredClone(rustAccounts);
      case "get_usage_intelligence":
        return { enabled: rustUsage };
      case "get_operational_settings":
        return structuredClone(rustOperational);
      case "get_runtime_snapshot":
        return { refreshIntervalMinutes: rustRefreshInterval };
      case "set_quota_notifications_enabled":
        rustOperational = {
          ...rustOperational,
          revision: rustOperational.revision + 1,
          quotaNotifications: (override ?? args.enabled) as boolean,
        };
        successfulWrites.push({ command, args });
        if (rejectAfterPersist.delete(command)) {
          throw new Error(`${command} persisted then rejected`);
        }
        return structuredClone(rustOperational);
      case "set_wallet_monitoring":
        rustOperational = {
          ...rustOperational,
          revision: rustOperational.revision + 1,
          monitored: {
            ...rustOperational.monitored,
            ...((override ?? args.monitored) as Record<string, boolean>),
          },
        };
        successfulWrites.push({ command, args });
        if (rejectAfterPersist.delete(command)) {
          throw new Error(`${command} persisted then rejected`);
        }
        return structuredClone(rustOperational);
      case "set_refresh_interval":
        rustRefreshInterval = (override ?? args.minutes) as number;
        successfulWrites.push({ command, args });
        if (rejectAfterPersist.delete(command)) {
          throw new Error(`${command} persisted then rejected`);
        }
        return;
      case "set_codex_accounts":
        rustAccounts = structuredClone(args.accounts as CodexAccountSettings[]);
        successfulWrites.push({ command, args });
        return true;
      case "set_usage_intelligence_enabled":
        rustUsage = args.enabled as boolean;
        successfulWrites.push({ command, args });
        return;
    }
  });
});

afterEach(() => {
  cleanup();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  localStorage.clear();
  failures.clear();
  gates.clear();
});

describe("refresh interval recovery and concurrency", () => {
  it("applies an interval change made during a pending reset after the reset defaults", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    // The reset blocks on its own interval write, so the user's change is
    // made while the reset is still in flight.
    const resetGate = gate("set_refresh_interval");
    let resetOutcome: LocalDataClearResult | undefined;
    act(() => {
      void result.current.resetPreferences().then((outcome) => {
        resetOutcome = outcome;
      });
    });
    await waitFor(() => expect(reached).toContain("set_refresh_interval"));

    act(() => {
      result.current.setRefreshInterval(30);
    });
    await waitFor(() =>
      expect(result.current.settings.refreshIntervalMinutes).toBe(30),
    );

    resetGate.release();
    await act(async () => {
      await waitFor(() => expect(resetOutcome).toEqual({ ok: true, removed: true }));
    });

    // The runtime, the local cache and React all end on the user's value: the
    // reset default landed first and the explicit edit after it.
    const intervalWrites = successfulWrites.filter(
      (write) => write.command === "set_refresh_interval",
    );
    expect(intervalWrites.map((write) => write.args)).toEqual([
      { minutes: 5 },
      { minutes: 30 },
    ]);
    expect(rustRefreshInterval).toBe(30);
    expect(stored().refreshIntervalMinutes).toBe(30);
    expect(result.current.settings.refreshIntervalMinutes).toBe(30);
    expect(result.current.unverifiedPreferences).not.toContain(
      "refreshIntervalMinutes",
    );
  });

  it("recovers the canonical interval when a rejected write had already persisted", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    // The runtime keeps a different cadence than the one that was asked for,
    // so only a fresh read can say what is really in effect.
    persistOverride.set("set_refresh_interval", 15);
    rejectAfterPersist.add("set_refresh_interval");
    act(() => {
      result.current.setRefreshInterval(30);
    });

    await waitFor(() =>
      expect(result.current.startupError).toContain(
        "The refresh interval could not be saved; it is currently 15 minutes.",
      ),
    );
    expect(rustRefreshInterval).toBe(15);
    expect(result.current.settings.refreshIntervalMinutes).toBe(15);
    expect(stored().refreshIntervalMinutes).toBe(15);
    // The recovered value is confirmed, so the scope is not left unknown.
    expect(result.current.unverifiedPreferences).not.toContain(
      "refreshIntervalMinutes",
    );
  });

  it("keeps the interval explicitly unknown when neither the write nor the read produces evidence", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    failNext("set_refresh_interval");
    failNext("get_runtime_snapshot");
    act(() => {
      result.current.setRefreshInterval(30);
    });

    await waitFor(() =>
      expect(result.current.unverifiedPreferences).toContain(
        "refreshIntervalMinutes",
      ),
    );
    expect(result.current.startupError).toContain(
      "its saved state could not be verified",
    );
    expect(rustRefreshInterval).toBe(5);
  });

  it("preserves the persisted startup interval when the hand-off fails, then retries it", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    // Establish the saved preference the runtime should receive.
    act(() => {
      result.current.setRefreshInterval(30);
    });
    await waitFor(() => {
      expect(rustRefreshInterval).toBe(30);
      expect(stored().refreshIntervalMinutes).toBe(30);
    });

    // Rust has a transient cadence when the startup hand-off is rejected. A
    // snapshot of that runtime value must not replace the user's saved choice.
    rustRefreshInterval = 5;
    const snapshotReads = reached.filter(
      (command) => command === "get_runtime_snapshot",
    ).length;
    failNext("set_refresh_interval");
    act(() => {
      result.current.syncRefreshInterval();
    });

    await waitFor(() =>
      expect(result.current.startupError).toContain(
        "The refresh interval could not be handed over to the runtime and its saved runtime state could not be verified.",
      ),
    );
    expect(result.current.settings.refreshIntervalMinutes).toBe(30);
    expect(stored().refreshIntervalMinutes).toBe(30);
    expect(result.current.unverifiedPreferences).toContain(
      "refreshIntervalMinutes",
    );
    expect(rustRefreshInterval).toBe(5);
    expect(
      reached.filter((command) => command === "get_runtime_snapshot").length,
    ).toBe(snapshotReads);

    // The visible retry re-sends the value that stayed persisted.
    act(() => {
      result.current.syncRefreshInterval();
    });
    await waitFor(() => {
      expect(rustRefreshInterval).toBe(30);
      expect(result.current.unverifiedPreferences).not.toContain(
        "refreshIntervalMinutes",
      );
      expect(result.current.startupError).toBeNull();
    });
    expect(result.current.settings.refreshIntervalMinutes).toBe(30);
    expect(stored().refreshIntervalMinutes).toBe(30);
  });
});

describe("operational scope verdicts", () => {
  it("clears the unknown scope when a later write succeeds", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    // Neither the write nor the recovery read produces evidence, so the scope
    // stays explicitly unknown.
    failNext("set_quota_notifications_enabled");
    failNext("get_operational_settings");
    act(() => {
      result.current.setQuotaNotifications(false);
    });
    await waitFor(() =>
      expect(result.current.unverifiedPreferences).toContain(
        "quotaNotifications",
      ),
    );
    expect(result.current.startupError).toContain("could not be saved");

    // A confirmed write settles the scope again.
    act(() => {
      result.current.setQuotaNotifications(false);
    });
    await waitFor(() =>
      expect(result.current.unverifiedPreferences).not.toContain(
        "quotaNotifications",
      ),
    );
    expect(result.current.startupError).toBeNull();
  });

  it("confirms the scope when the recovery read succeeds after a failed write", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    failNext("set_quota_notifications_enabled");
    act(() => {
      result.current.setQuotaNotifications(false);
    });

    await waitFor(() =>
      expect(result.current.startupError).toContain("could not be saved"),
    );
    // The recovery read produced the canonical state, so the scope is
    // confirmed even though the write itself failed.
    expect(result.current.unverifiedPreferences).not.toContain(
      "quotaNotifications",
    );
    expect(result.current.settings.quotaNotifications).toBe(true);
  });

  it("does not let an older recovery overwrite a newer operational edit", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    // The first write fails and its recovery read hangs.
    failNext("set_wallet_monitoring");
    const readGate = gate("get_operational_settings");
    act(() => {
      result.current.setWalletMonitored("openrouter", true);
    });
    await waitFor(() => expect(reached).toContain("get_operational_settings"));

    // A newer edit takes the state over while the older recovery is pending.
    const writeGate = gate("set_wallet_monitoring");
    act(() => {
      result.current.setWalletMonitored("deepseek", true);
    });

    readGate.release();
    await waitFor(() => expect(reached.filter((c) => c === "set_wallet_monitoring").length).toBe(2));

    // The stale recovery must not push the canonical snapshot over the newer
    // edit: the newer edit still owns the live state.
    expect(
      result.current.settings.walletPreferences.monitored.openrouter,
    ).toBe(false);

    writeGate.release();
    await waitFor(() =>
      expect(result.current.settings.walletPreferences.monitored.deepseek).toBe(
        true,
      ),
    );
    expect(
      result.current.settings.walletPreferences.monitored.openrouter,
    ).toBe(false);
  });
});
