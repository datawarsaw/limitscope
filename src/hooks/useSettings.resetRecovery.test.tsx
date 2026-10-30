// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { useSettings } from "./useSettings";
import { DEFAULT_SETTINGS, SETTINGS_STORAGE_KEY } from "../lib/settings";
import type { CodexAccountSettings } from "../lib/codexAccounts";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";

type OperationalSettings = {
  revision: number;
  quotaNotifications: boolean;
  monitored: Record<string, boolean>;
};

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
  {
    accountId: "openai-codex:secondary",
    alias: "Cowork",
    codexHome: "C:\\fixture\\cowork",
    monitored: false,
  },
];
const originalMonitored = { openrouter: true, deepseek: false };

let rustAccounts: CodexAccountSettings[];
let rustUsage: boolean;
let rustOperational: OperationalSettings;
let rustRefreshInterval: number;
let successfulWrites: Array<{ command: string; args: unknown }>;
const failures = new Map<string, number>();

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function failNext(command: string): void {
  failures.set(command, (failures.get(command) ?? 0) + 1);
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
    monitored: { ...originalMonitored },
  };
  rustRefreshInterval = 30;
  successfulWrites = [];
  failures.clear();

  mocks.invoke.mockReset();
  mocks.invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
    const remainingFailures = failures.get(command) ?? 0;
    if (remainingFailures > 0) {
      failures.set(command, remainingFailures - 1);
      throw new Error(`${command} failed`);
    }

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
          quotaNotifications: args.enabled as boolean,
        };
        successfulWrites.push({ command, args });
        return structuredClone(rustOperational);
      case "set_wallet_monitoring":
        rustOperational = {
          ...rustOperational,
          revision: rustOperational.revision + 1,
          monitored: { ...(args.monitored as Record<string, boolean>) },
        };
        successfulWrites.push({ command, args });
        return structuredClone(rustOperational);
      case "set_refresh_interval":
        rustRefreshInterval = args.minutes as number;
        successfulWrites.push({ command, args });
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
});

describe("preference reset recovery", () => {
  it("reconciles React and the local cache with Rust after a partial reset", async () => {
    failNext("set_refresh_interval");
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let outcome: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: false });
    expect(result.current.settings).toMatchObject({
      quotaNotifications: false,
      refreshIntervalMinutes: 30,
      usageIntelligence: true,
      walletPreferences: { monitored: originalMonitored },
      codexAccounts: { accounts: originalAccounts },
    });
    expect(stored()).toMatchObject({
      quotaNotifications: false,
      refreshIntervalMinutes: 30,
      usageIntelligence: true,
      walletPreferences: { monitored: originalMonitored },
      codexAccounts: { accounts: originalAccounts },
    });
    expect(rustRefreshInterval).toBe(30);
    expect(rustAccounts).toEqual(originalAccounts);
    expect(rustUsage).toBe(true);
    expect(rustOperational).toMatchObject({
      quotaNotifications: false,
      monitored: originalMonitored,
    });
    expect(successfulWrites.map(({ command }) => command)).toEqual([
      "set_quota_notifications_enabled",
    ]);
  });

  it("allows a clean retry after a failed reset and leaves no stale defaults", async () => {
    failNext("set_usage_intelligence_enabled");
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let first: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      first = await result.current.resetPreferences();
    });
    expect(first).toEqual({ ok: false });
    expect(result.current.settings.codexAccounts.accounts).toEqual(
      DEFAULT_SETTINGS.codexAccounts.accounts,
    );
    expect(result.current.settings.usageIntelligence).toBe(true);
    expect(rustUsage).toBe(true);

    let second: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      second = await result.current.resetPreferences();
    });
    expect(second).toEqual({ ok: true, removed: true });
    expect(result.current.settings).toEqual(DEFAULT_SETTINGS);
    expect(stored()).toMatchObject(DEFAULT_SETTINGS);
    expect(rustAccounts).toEqual(DEFAULT_SETTINGS.codexAccounts.accounts);
    expect(rustUsage).toBe(false);
    expect(rustOperational.quotaNotifications).toBe(false);
    expect(rustRefreshInterval).toBe(5);
  });

  it("waits for an account edit already pending when reset starts", async () => {
    const writeGate = deferred<void>();
    const writeStarted = deferred<void>();
    const base = mocks.invoke.getMockImplementation()!;
    let held = false;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "set_codex_accounts" && !held) {
        held = true;
        writeStarted.resolve(undefined);
        await writeGate.promise;
      }
      return base(command, args);
    });

    const { result } = renderHook(() => useSettings());
    await hydrated(result);
    act(() => result.current.setCodexAccount("openai-codex", { alias: "Before reset" }));
    await act(async () => {
      await writeStarted.promise;
    });

    let resetPromise!: Promise<Awaited<ReturnType<typeof result.current.resetPreferences>>>;
    act(() => {
      resetPromise = result.current.resetPreferences();
    });
    expect(result.current.settings.codexAccounts.accounts).toEqual(
      DEFAULT_SETTINGS.codexAccounts.accounts,
    );
    writeGate.resolve(undefined);

    let outcome: Awaited<typeof resetPromise> | undefined;
    await act(async () => {
      outcome = await resetPromise;
    });
    expect(outcome).toEqual({ ok: true, removed: true });
    expect(rustAccounts).toEqual(DEFAULT_SETTINGS.codexAccounts.accounts);
    expect(result.current.settings.codexAccounts.accounts).toEqual(
      DEFAULT_SETTINGS.codexAccounts.accounts,
    );
    expect(
      successfulWrites
        .filter(({ command }) => command === "set_codex_accounts")
        .map(({ args }) => (args as { accounts: CodexAccountSettings[] }).accounts),
    ).toEqual([
      [
        { ...originalAccounts[0], alias: "Before reset" },
        originalAccounts[1],
      ],
      DEFAULT_SETTINGS.codexAccounts.accounts,
    ]);
  });

  it("waits for a Usage Intelligence write already pending when reset starts", async () => {
    const writeGate = deferred<void>();
    const writeStarted = deferred<void>();
    const base = mocks.invoke.getMockImplementation()!;
    let usageWrites = 0;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "set_usage_intelligence_enabled") {
        usageWrites += 1;
        if (usageWrites === 1) {
          writeStarted.resolve(undefined);
          await writeGate.promise;
        }
      }
      return base(command, args);
    });

    const { result } = renderHook(() => useSettings());
    await hydrated(result);
    act(() => result.current.setUsageIntelligence(false));
    await act(async () => {
      await writeStarted.promise;
    });

    let resetPromise!: Promise<Awaited<ReturnType<typeof result.current.resetPreferences>>>;
    act(() => {
      resetPromise = result.current.resetPreferences();
    });
    expect(result.current.settings.usageIntelligence).toBe(
      DEFAULT_SETTINGS.usageIntelligence,
    );
    writeGate.resolve(undefined);

    let outcome: Awaited<typeof resetPromise> | undefined;
    await act(async () => {
      outcome = await resetPromise;
    });
    expect(outcome).toEqual({ ok: true, removed: true });
    expect(rustUsage).toBe(false);
    expect(result.current.settings.usageIntelligence).toBe(false);
    expect(stored().usageIntelligence).toBe(false);
    expect(
      successfulWrites
        .filter(({ command }) =>
          command === "set_usage_intelligence_enabled" ||
          command === "set_codex_accounts"
        )
        .map(({ command }) => command),
    ).toEqual([
      "set_usage_intelligence_enabled",
      "set_codex_accounts",
      "set_usage_intelligence_enabled",
    ]);
  });

  it("applies an explicit edit invoked after reset starts after the reset defaults", async () => {
    const resetGate = deferred<void>();
    const resetStarted = deferred<void>();
    const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "set_quota_notifications_enabled") {
        resetStarted.resolve(undefined);
        await resetGate.promise;
      }
      return base(command, args);
    });

    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let resetPromise!: Promise<Awaited<ReturnType<typeof result.current.resetPreferences>>>;
    act(() => {
      resetPromise = result.current.resetPreferences();
    });
    await act(async () => {
      await resetStarted.promise;
    });
    act(() => {
      result.current.setCodexAccount("openai-codex", { alias: "After reset" });
      result.current.setUsageIntelligence(true);
    });
    resetGate.resolve(undefined);

    await act(async () => {
      expect(await resetPromise).toEqual({ ok: true, removed: true });
    });
    await waitFor(() => {
      expect(rustAccounts[0].alias).toBe("After reset");
      expect(rustUsage).toBe(true);
      expect(result.current.settings.codexAccounts.accounts[0].alias).toBe("After reset");
      expect(result.current.settings.usageIntelligence).toBe(true);
    });
    expect(rustAccounts[1]).toEqual(DEFAULT_SETTINGS.codexAccounts.accounts[1]);
    expect(stored().codexAccounts.accounts[0].alias).toBe("After reset");
    expect(stored().usageIntelligence).toBe(true);
    expect(
      successfulWrites
        .filter(({ command }) => command === "set_codex_accounts")
        .map(({ args }) => (args as { accounts: CodexAccountSettings[] }).accounts[0].alias),
    ).toEqual(["", "After reset"]);
  });

  it("does not let reset recovery overwrite an edit started while recovery is pending", async () => {
    failNext("set_usage_intelligence_enabled");
    const recoveryGate = deferred<void>();
    const recoveryStarted = deferred<void>();
    const base = mocks.invoke.getMockImplementation()!;
    let accountReads = 0;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "get_codex_accounts" && ++accountReads === 2) {
        recoveryStarted.resolve(undefined);
        await recoveryGate.promise;
      }
      return base(command, args);
    });

    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let resetPromise!: Promise<Awaited<ReturnType<typeof result.current.resetPreferences>>>;
    act(() => {
      resetPromise = result.current.resetPreferences();
    });
    await act(async () => {
      await recoveryStarted.promise;
    });
    act(() => {
      result.current.setCodexAccount("openai-codex", { alias: "After failure" });
    });
    recoveryGate.resolve(undefined);

    await act(async () => {
      expect(await resetPromise).toEqual({ ok: false });
    });
    await waitFor(() => {
      expect(rustAccounts[0].alias).toBe("After failure");
      expect(result.current.settings.codexAccounts.accounts[0].alias).toBe("After failure");
      expect(result.current.settings.usageIntelligence).toBe(true);
    });
    expect(rustUsage).toBe(true);
    expect(stored().codexAccounts.accounts[0].alias).toBe("After failure");
    expect(stored().usageIntelligence).toBe(true);
  });

  it("reports an unverified state instead of last-seen values when recovery reads fail", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let seeded: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      seeded = await result.current.resetPreferences();
    });
    expect(seeded).toEqual({ ok: true, removed: true });
    act(() => result.current.setRefreshInterval(15));
    rustRefreshInterval = 15;
    successfulWrites = [];

    // Model the dangerous case from the review: the write reached storage and
    // only then rejected. The stale cache says 15, Rust now says 1, and the
    // recovery read will fail, so neither value may be presented as confirmed.
    const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "set_refresh_interval") {
        rustRefreshInterval = 1;
        throw new Error("set_refresh_interval persisted then rejected");
      }
      // Every recovery read fails, not just the first one: the failed write
      // and the failed reset each try to recover, and neither may present a
      // value as confirmed.
      if (command === "get_runtime_snapshot") {
        throw new Error("get_runtime_snapshot unavailable");
      }
      return base(command, args);
    });
    failNext("get_operational_settings");
    failNext("get_codex_accounts");
    failNext("get_usage_intelligence");

    let outcome: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: false });
    expect(result.current.settings).toEqual(DEFAULT_SETTINGS);
    expect(stored()).toMatchObject(DEFAULT_SETTINGS);
    expect(result.current.unverifiedPreferences).toEqual([
      "quotaNotifications",
      "walletMonitoring",
      "codexAccounts",
      "usageIntelligence",
      "refreshIntervalMinutes",
    ]);
    expect(result.current.startupError).toContain("could not be verified");
    expect(result.current.startupError).toContain("Codex accounts");
    expect(result.current.startupPending).toBe(false);
    expect(rustAccounts).toEqual(DEFAULT_SETTINGS.codexAccounts.accounts);
    expect(rustRefreshInterval).toBe(1);
    expect(successfulWrites.map(({ command }) => command)).toEqual([
      "set_quota_notifications_enabled",
    ]);
  });

  it("keeps reset pending and deterministic across overlapping reset calls", async () => {
    const firstGate = deferred<void>();
    const secondGate = deferred<void>();
    const firstStarted = deferred<void>();
    const secondStarted = deferred<void>();
    const base = mocks.invoke.getMockImplementation()!;
    let quotaResets = 0;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "set_quota_notifications_enabled") {
        quotaResets += 1;
        if (quotaResets === 1) {
          firstStarted.resolve(undefined);
          await firstGate.promise;
        }
        if (quotaResets === 2) {
          secondStarted.resolve(undefined);
          await secondGate.promise;
        }
      }
      return base(command, args);
    });

    const { result } = renderHook(() => useSettings());
    await hydrated(result);
    let firstReset!: Promise<Awaited<ReturnType<typeof result.current.resetPreferences>>>;
    let secondReset!: Promise<Awaited<ReturnType<typeof result.current.resetPreferences>>>;
    act(() => {
      firstReset = result.current.resetPreferences();
    });
    await act(async () => {
      await firstStarted.promise;
    });
    act(() => {
      secondReset = result.current.resetPreferences();
    });
    firstGate.resolve(undefined);
    await act(async () => {
      await secondStarted.promise;
    });

    expect(result.current.startupPending).toBe(true);
    secondGate.resolve(undefined);
    let outcomes: Array<Awaited<typeof firstReset>> | undefined;
    await act(async () => {
      outcomes = await Promise.all([firstReset, secondReset]);
    });

    expect(outcomes).toEqual([
      { ok: true, removed: true },
      { ok: true, removed: true },
    ]);
    expect(result.current.startupPending).toBe(false);
    expect(result.current.settings).toEqual(DEFAULT_SETTINGS);
    expect(
      successfulWrites
        .filter(({ command }) => command !== "set_usage_intelligence_enabled")
        .map(({ command }) => command),
    ).toEqual([
      "set_quota_notifications_enabled",
      "set_refresh_interval",
      "set_wallet_monitoring",
      "set_codex_accounts",
      "set_quota_notifications_enabled",
      "set_refresh_interval",
      "set_wallet_monitoring",
      "set_codex_accounts",
    ]);
  });

  it("resets every runtime preference even when the autostart entry cannot be updated", async () => {
    vi.mocked(disable).mockRejectedValueOnce(
      new Error("registry entry is locked"),
    );
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let outcome: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    // The OS entry is unresolved, but every other preference still had to be
    // reset and reconciled: a failed disable must not stop the reset.
    expect(outcome).toEqual({ ok: false });
    expect(result.current.startupError).toBe(
      "registry entry is locked. The launch-at-startup entry could not be turned off; it is currently off.",
    );
    expect(successfulWrites.map(({ command }) => command)).toEqual([
      "set_quota_notifications_enabled",
      "set_refresh_interval",
      "set_wallet_monitoring",
      "set_codex_accounts",
      "set_usage_intelligence_enabled",
    ]);
    expect(result.current.settings).toEqual(DEFAULT_SETTINGS);
    expect(stored()).toMatchObject(DEFAULT_SETTINGS);
    expect(rustOperational.quotaNotifications).toBe(
      DEFAULT_SETTINGS.quotaNotifications,
    );
    expect(rustOperational.monitored).toEqual(
      DEFAULT_SETTINGS.walletPreferences.monitored,
    );
    expect(rustRefreshInterval).toBe(DEFAULT_SETTINGS.refreshIntervalMinutes);
    expect(rustAccounts).toEqual(DEFAULT_SETTINGS.codexAccounts.accounts);
    expect(rustUsage).toBe(DEFAULT_SETTINGS.usageIntelligence);
    // The read of the OS state confirmed what is saved, so the scope is not
    // unknown: the entry is off, which is the state the reset asked for.
    expect(result.current.unverifiedPreferences).toEqual([]);
    expect(result.current.startupPending).toBe(false);
  });

  it("does not let the reset recovery push a stale operational snapshot over a concurrent edit", async () => {
    // The first runtime write of the reset fails, so Rust keeps the state it
    // had: notifications are still on. The recovery read therefore returns a
    // snapshot that contradicts the edit the user makes meanwhile.
    failNext("set_quota_notifications_enabled");
    const recoveryGate = deferred<void>();
    const recoveryStarted = deferred<void>();
    const editGate = deferred<void>();
    const editStarted = deferred<void>();
    const base = mocks.invoke.getMockImplementation()!;
    let operationalReads = 0;
    let quotaWrites = 0;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "get_operational_settings" && ++operationalReads === 2) {
        recoveryStarted.resolve(undefined);
        await recoveryGate.promise;
      }
      if (command === "set_quota_notifications_enabled") {
        quotaWrites += 1;
        if (quotaWrites === 2) {
          // The concurrent edit is in flight and the reset task is done, so
          // the state can be inspected before the edit reaches Rust.
          editStarted.resolve(undefined);
          await editGate.promise;
        }
      }
      return base(command, args);
    });

    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let resetPromise!: Promise<Awaited<ReturnType<typeof result.current.resetPreferences>>>;
    act(() => {
      resetPromise = result.current.resetPreferences();
    });
    await act(async () => {
      await recoveryStarted.promise;
    });
    // A newer operational edit starts while the recovery read is pending: the
    // user turns the notifications off, Rust still has them on.
    act(() => {
      result.current.setQuotaNotifications(false);
    });
    recoveryGate.resolve(undefined);

    await act(async () => {
      await editStarted.promise;
    });

    // The recovery read returned the stale snapshot (notifications on). The
    // concurrent edit is newer, so the recovery must not push that value back
    // over the pending choice - not in React and not in the local cache.
    expect(result.current.settings.quotaNotifications).toBe(false);
    expect(stored().quotaNotifications).toBe(false);

    editGate.resolve(undefined);
    await act(async () => {
      expect(await resetPromise).toEqual({ ok: false });
    });
    await waitFor(() => {
      expect(result.current.settings.quotaNotifications).toBe(false);
      expect(stored().quotaNotifications).toBe(false);
    });
    // The newer edit owns the operational group: its reply carries the whole
    // runtime snapshot, so React and the cache end up in step with Rust.
    expect(rustOperational.quotaNotifications).toBe(false);
    expect(result.current.settings.walletPreferences.monitored).toEqual(
      originalMonitored,
    );
    expect(stored().walletPreferences.monitored).toEqual(originalMonitored);
    expect(result.current.unverifiedPreferences).toEqual([]);
    expect(result.current.startupError).toContain("could not be reset");
  });

  it("keeps the local cache in step with the operational values the recovery applies", async () => {
    // Rust keeps everything at its default except the operational group, so
    // the recovery only changes that group and the local cache can only be
    // updated by the recovery itself.
    rustAccounts = structuredClone(DEFAULT_SETTINGS.codexAccounts.accounts);
    rustUsage = DEFAULT_SETTINGS.usageIntelligence;
    rustRefreshInterval = DEFAULT_SETTINGS.refreshIntervalMinutes;
    rustOperational = {
      revision: 0,
      quotaNotifications: true,
      monitored: { ...originalMonitored },
    };
    failNext("set_quota_notifications_enabled");
    // The account read of the recovery fails, so the recovery cannot rely on
    // a later account reconciliation to write the cache.
    const base = mocks.invoke.getMockImplementation()!;
    let accountReads = 0;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "get_codex_accounts" && ++accountReads === 2) {
        throw new Error("get_codex_accounts failed");
      }
      return base(command, args);
    });

    const { result } = renderHook(() => useSettings());
    await waitFor(() => {
      expect(result.current.settings.quotaNotifications).toBe(true);
    });

    let outcome: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: false });
    expect(result.current.settings.quotaNotifications).toBe(true);
    expect(result.current.settings.walletPreferences.monitored).toEqual(
      originalMonitored,
    );
    // The recovered operational values are a read-only projection of Rust,
    // so the WebView cache has to follow them.
    expect(stored().quotaNotifications).toBe(true);
    expect(stored().walletPreferences.monitored).toEqual(originalMonitored);
    expect(result.current.unverifiedPreferences).toEqual(["codexAccounts"]);
  });

  it("recovers the canonical autostart state instead of claiming the reset default", async () => {
    // The OS entry stays on: the reset wrote its default into the WebView
    // stores, but the canonical state is the one the OS reports.
    vi.mocked(disable).mockRejectedValueOnce(
      new Error("registry entry is locked"),
    );
    vi.mocked(isEnabled).mockResolvedValue(true);
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let outcome: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: false });
    // The default must not be presented as the saved state.
    expect(result.current.settings.launchAtStartup).toBe(true);
    expect(stored().launchAtStartup).toBe(true);
    expect(result.current.unverifiedPreferences).not.toContain("launchAtStartup");
    expect(result.current.startupError).toContain("it is currently on");
    // Every runtime preference was still reset.
    expect(successfulWrites.map(({ command }) => command)).toEqual([
      "set_quota_notifications_enabled",
      "set_refresh_interval",
      "set_wallet_monitoring",
      "set_codex_accounts",
      "set_usage_intelligence_enabled",
    ]);
  });

  it("keeps the launch-at-startup scope unknown when neither write nor read produces evidence", async () => {
    vi.mocked(disable).mockRejectedValueOnce(
      new Error("registry entry is locked"),
    );
    vi.mocked(isEnabled)
      // The startup probe answers.
      .mockResolvedValueOnce(false)
      // The recovery read after the failed write does not.
      .mockRejectedValueOnce(new Error("autostart probe failed"));
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    let outcome: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: false });
    // Neither the write nor a read produced evidence, so the scope stays
    // explicitly unknown instead of claiming the default was applied.
    expect(result.current.unverifiedPreferences).toEqual(["launchAtStartup"]);
    expect(result.current.startupError).toContain("could not be verified");
    expect(result.current.startupError).toContain("registry entry is locked");
    // The runtime preferences were reset regardless of the OS entry.
    expect(successfulWrites.map(({ command }) => command)).toEqual([
      "set_quota_notifications_enabled",
      "set_refresh_interval",
      "set_wallet_monitoring",
      "set_codex_accounts",
      "set_usage_intelligence_enabled",
    ]);
  });

  it("recovers the canonical autostart state after a failed toggle", async () => {
    vi.mocked(enable).mockRejectedValueOnce(new Error("access denied"));
    vi.mocked(isEnabled).mockResolvedValue(false);
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    await act(async () => {
      await result.current.setLaunchAtStartup(true);
    });

    // The read says the OS never enabled the entry, so the optimistic value
    // is rolled back to the canonical one.
    expect(result.current.settings.launchAtStartup).toBe(false);
    expect(stored().launchAtStartup).toBe(false);
    expect(result.current.unverifiedPreferences).not.toContain("launchAtStartup");
    expect(result.current.startupError).toContain("it is currently off");
    expect(result.current.startupError).toContain("access denied");
  });

  it("keeps the launch-at-startup scope unknown after a failed toggle with a failed read", async () => {
    vi.mocked(enable).mockRejectedValueOnce(new Error("access denied"));
    vi.mocked(isEnabled)
      // The startup probe answers.
      .mockResolvedValueOnce(false)
      // The recovery read after the failed toggle does not.
      .mockRejectedValueOnce(new Error("autostart probe failed"));
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    await act(async () => {
      await result.current.setLaunchAtStartup(true);
    });

    expect(result.current.unverifiedPreferences).toEqual(["launchAtStartup"]);
    expect(result.current.startupError).toContain("could not be verified");
    expect(result.current.startupError).toContain("access denied");
    expect(result.current.startupPending).toBe(false);
  });

  it("does not let a concurrent action clear the unknown launch-at-startup state", async () => {
    vi.mocked(enable).mockRejectedValueOnce(new Error("access denied"));
    vi.mocked(isEnabled)
      .mockResolvedValueOnce(false)
      .mockRejectedValueOnce(new Error("autostart probe failed"));
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    await act(async () => {
      await result.current.setLaunchAtStartup(true);
    });
    expect(result.current.unverifiedPreferences).toEqual(["launchAtStartup"]);

    // Other actions run while the scope is unresolved. They neither confirm
    // nor deny the OS state, so they must not drop the unknown state or the
    // message that explains it.
    act(() => {
      result.current.setRefreshInterval(15);
      result.current.setQuotaNotifications(false);
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(result.current.unverifiedPreferences).toEqual(["launchAtStartup"]);
    expect(result.current.startupError).toContain("could not be verified");
  });

  it("clears the unknown launch-at-startup state once a retry is confirmed", async () => {
    vi.mocked(enable).mockRejectedValueOnce(new Error("access denied"));
    vi.mocked(isEnabled)
      .mockResolvedValueOnce(false)
      .mockRejectedValueOnce(new Error("autostart probe failed"));
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    await act(async () => {
      await result.current.setLaunchAtStartup(true);
    });
    expect(result.current.unverifiedPreferences).toEqual(["launchAtStartup"]);

    // The retry succeeds, so the OS state is known again.
    vi.mocked(enable).mockResolvedValueOnce(undefined);
    await act(async () => {
      await result.current.setLaunchAtStartup(true);
    });

    expect(result.current.unverifiedPreferences).toEqual([]);
    expect(result.current.startupError).toBeNull();
    expect(result.current.settings.launchAtStartup).toBe(true);
  });

  it("clears an older refresh interval error after a confirmed reset", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    failNext("set_refresh_interval");
    failNext("get_runtime_snapshot");
    act(() => {
      result.current.setRefreshInterval(15);
    });
    await waitFor(() => {
      expect(result.current.unverifiedPreferences).toContain(
        "refreshIntervalMinutes",
      );
      expect(result.current.startupError).toContain("could not be saved");
    });

    let outcome: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: true, removed: true });
    expect(result.current.startupError).toBeNull();
    expect(result.current.unverifiedPreferences).toEqual([]);
  });

  it("clears an older operational error after a confirmed reset", async () => {
    const { result } = renderHook(() => useSettings());
    await hydrated(result);

    failNext("set_quota_notifications_enabled");
    failNext("get_operational_settings");
    act(() => {
      result.current.setQuotaNotifications(false);
    });
    await waitFor(() => {
      expect(result.current.unverifiedPreferences).toContain(
        "quotaNotifications",
      );
      expect(result.current.startupError).toContain("could not be saved");
    });

    let outcome: Awaited<ReturnType<typeof result.current.resetPreferences>> | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: true, removed: true });
    expect(result.current.startupError).toBeNull();
    expect(result.current.unverifiedPreferences).toEqual([]);
  });

  it("keeps a newer concurrent operational failure after the reset succeeds", async () => {
    const resetStarted = deferred<void>();
    const resetWrite = deferred<void>();
    const base = mocks.invoke.getMockImplementation()!;
    let quotaWrites = 0;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "set_quota_notifications_enabled") {
        quotaWrites += 1;
        if (quotaWrites === 1) {
          resetStarted.resolve(undefined);
          await resetWrite.promise;
        } else if (quotaWrites === 2) {
          throw new Error("newer operational edit failed");
        }
      }
      return base(command, args);
    });

    const { result } = renderHook(() => useSettings());
    await hydrated(result);
    failNext("get_operational_settings");

    let resetPromise!: Promise<Awaited<ReturnType<typeof result.current.resetPreferences>>>;
    act(() => {
      resetPromise = result.current.resetPreferences();
    });
    await act(async () => {
      await resetStarted.promise;
    });

    // This newer action starts while reset is in flight. Its failure must
    // survive the reset success that precedes its queued write.
    act(() => {
      result.current.setQuotaNotifications(false);
    });
    resetWrite.resolve(undefined);
    await act(async () => {
      expect(await resetPromise).toEqual({ ok: true, removed: true });
    });

    await waitFor(() => {
      expect(result.current.unverifiedPreferences).toContain(
        "quotaNotifications",
      );
      expect(result.current.startupError).toContain("could not be saved");
    });
  });
});
