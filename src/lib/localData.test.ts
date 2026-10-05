// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_FLOATING_PREFS,
  FLOATING_PREFS_STORAGE_KEY,
} from "./floatingWindowPrefs";
import { DEFAULT_SETTINGS, SETTINGS_STORAGE_KEY } from "./settings";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(async () => ({ category: "providerCache", removed: true })),
  clearRuntimeHistory: vi.fn(async () => {}),
  removeLegacyHistory: vi.fn(() => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

vi.mock("./historyClient", () => ({
  clearRuntimeHistory: mocks.clearRuntimeHistory,
  removeLegacyHistory: mocks.removeLegacyHistory,
}));

import {
  clearProviderCache,
  clearExecutionRuns,
  clearUsageHistoryStore,
  LOCAL_DATA_COMMANDS,
  localDataControls,
  resetPreferenceStores,
} from "./localData";
import {
  beginExecutionRun,
  completeExecutionRun,
  EMPTY_EXECUTION_RUNS_STATE,
  loadExecutionRunsState,
  saveExecutionRunsState,
} from "./executionRuns";
import type { ProviderUsage } from "../types";

function simulateTauri(): void {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}

beforeEach(() => {
  window.localStorage.clear();
  mocks.invoke.mockClear();
  mocks.invoke.mockResolvedValue({ category: "providerCache", removed: true });
  mocks.clearRuntimeHistory.mockClear();
  mocks.removeLegacyHistory.mockClear();
});

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe("clearProviderCache", () => {
  it("calls the fixed command with no argument at all", async () => {
    simulateTauri();
    const result = await clearProviderCache();

    expect(mocks.invoke).toHaveBeenCalledTimes(1);
    expect(mocks.invoke.mock.calls[0]).toEqual([LOCAL_DATA_COMMANDS.providerCache]);
    expect(mocks.invoke.mock.calls[0]).toHaveLength(1);
    expect(result).toEqual({ ok: true, removed: true });
  });

  it("reports an already-empty cache as success, not as a failure", async () => {
    simulateTauri();
    mocks.invoke.mockResolvedValueOnce({ category: "providerCache", removed: false });

    await expect(clearProviderCache()).resolves.toEqual({ ok: true, removed: false });
  });

  it("surfaces a scoped failure instead of throwing or claiming success", async () => {
    simulateTauri();
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    mocks.invoke.mockRejectedValueOnce(new Error("ipc unavailable"));

    await expect(clearProviderCache()).resolves.toEqual({ ok: false });
    spy.mockRestore();
  });

  it("is a successful no-op outside the Tauri runtime", async () => {
    await expect(clearProviderCache()).resolves.toEqual({ ok: true, removed: false });
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});
describe("clearUsageHistoryStore", () => {
  it("clears the Rust history store through its canonical command", async () => {
    simulateTauri();
    await expect(clearUsageHistoryStore()).resolves.toEqual({ ok: true, removed: true });
    expect(mocks.clearRuntimeHistory).toHaveBeenCalledTimes(1);
    expect(mocks.removeLegacyHistory).toHaveBeenCalledTimes(1);
  });

  it("never reports success when the history store fails", async () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    mocks.clearRuntimeHistory.mockRejectedValueOnce(new Error("history store down"));

    await expect(clearUsageHistoryStore()).resolves.toEqual({ ok: false });
    spy.mockRestore();
  });

  it("does not touch the provider cache", async () => {
    simulateTauri();
    await clearUsageHistoryStore();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});

describe("clearExecutionRuns", () => {
  it("clears completed runs without touching other owned stores", async () => {
    const settingsBefore = JSON.stringify({
      ...DEFAULT_SETTINGS,
      theme: "glass",
      futureLaneField: { keep: true },
    });
    const floatingBefore = JSON.stringify({
      ...DEFAULT_FLOATING_PREFS,
      x: 320,
      futureFloatingField: { keep: true },
    });
    window.localStorage.setItem(SETTINGS_STORAGE_KEY, settingsBefore);
    window.localStorage.setItem(FLOATING_PREFS_STORAGE_KEY, floatingBefore);
    const nowMs = Date.parse("2026-09-30T12:00:00Z");
    const usage: ProviderUsage = {
      id: "openai-codex",
      name: "OpenAI / Codex",
      status: "ok",
      health: "live",
      checkedAt: new Date(nowMs - 120000).toISOString(),
      limits: [{ label: "5-hour", usedPercent: 23, resetAt: "2026-09-30T18:00:00Z" }],
    };
    const started = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage,
      harness: "Codex",
      nowMs,
    });
    if (started.status !== "started") throw new Error("fixture start failed");
    const finished = completeExecutionRun(
      started.state,
      [{ ...usage, checkedAt: new Date(nowMs + 60000).toISOString() }],
      { nowMs: nowMs + 60000 },
    );
    if (finished.status !== "finished") throw new Error("fixture finish failed");
    saveExecutionRunsState(finished.state);

    await expect(
      clearExecutionRuns({ deliberateActiveConfirmation: true }),
    ).resolves.toEqual({ ok: true, removed: true });
    expect(loadExecutionRunsState()).toEqual(EMPTY_EXECUTION_RUNS_STATE);
    expect(mocks.clearRuntimeHistory).not.toHaveBeenCalled();
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(window.localStorage.getItem(SETTINGS_STORAGE_KEY)).toBe(settingsBefore);
    expect(window.localStorage.getItem(FLOATING_PREFS_STORAGE_KEY)).toBe(
      floatingBefore,
    );
  });
});

describe("resetPreferenceStores", () => {
  it("restores canonical defaults in both preference stores", () => {
    window.localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({
        launchAtStartup: true,
        refreshIntervalMinutes: 30,
        theme: "oled",
        quotaNotifications: true,
        providerPreferences: { order: ["grok"], hidden: ["zai"] },
        quotaPerspective: "remaining",
      }),
    );
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: false,
        floatingBarEnabled: false,
        alwaysOnTop: false,
        x: 320,
        y: 180,
      }),
    );

    const reset = resetPreferenceStores();

    expect(reset.settings).toEqual(DEFAULT_SETTINGS);
    expect(reset.floating).toEqual(DEFAULT_FLOATING_PREFS);
    expect(
      JSON.parse(window.localStorage.getItem(SETTINGS_STORAGE_KEY)!),
    ).toEqual(DEFAULT_SETTINGS);
    expect(
      JSON.parse(window.localStorage.getItem(FLOATING_PREFS_STORAGE_KEY)!),
    ).toEqual(DEFAULT_FLOATING_PREFS);
  });

  it("preserves unknown fields another lane wrote", () => {
    window.localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({ theme: "oled", futurePluginSettings: { telemetryOptIn: true } }),
    );
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ x: 12, y: 34, futureFloatingField: { autoPin: true } }),
    );

    resetPreferenceStores();

    const storedSettings = JSON.parse(
      window.localStorage.getItem(SETTINGS_STORAGE_KEY)!,
    );
    expect(storedSettings.theme).toBe("graphite");
    expect(storedSettings.futurePluginSettings).toEqual({ telemetryOptIn: true });
    const storedFloating = JSON.parse(
      window.localStorage.getItem(FLOATING_PREFS_STORAGE_KEY)!,
    );
    expect(storedFloating.x).toBeNull();
    expect(storedFloating.futureFloatingField).toEqual({ autoPin: true });
  });

  it("clears a corrupt preference store instead of preserving garbage", () => {
    window.localStorage.setItem(SETTINGS_STORAGE_KEY, "{not json");
    window.localStorage.setItem(FLOATING_PREFS_STORAGE_KEY, "[1,2,3]");

    resetPreferenceStores();

    expect(JSON.parse(window.localStorage.getItem(SETTINGS_STORAGE_KEY)!)).toEqual(
      DEFAULT_SETTINGS,
    );
    expect(
      JSON.parse(window.localStorage.getItem(FLOATING_PREFS_STORAGE_KEY)!),
    ).toEqual(DEFAULT_FLOATING_PREFS);
  });
});

describe("local data control catalog", () => {
  it("offers exactly the categories that exist on this lineage", () => {
    // "executionRuns" is the rehearsal reconciliation: the run store exists
    // on the integrated v0.7 product, so it gets a confirmed clear like
    // every other owned category. No other rows may appear.
    expect(localDataControls(false).map((control) => control.id)).toEqual([
      "usageHistory",
      "providerCache",
      "executionRuns",
      "preferences",
    ]);
  });

  it("names every destructive action in words and states what is preserved", () => {
    for (const control of localDataControls(false)) {
      expect(control.confirmAction.toLowerCase()).toContain(
        control.action.toLowerCase(),
      );
      expect(control.confirmTitle.endsWith("?")).toBe(true);
      expect(control.confirmBody).toMatch(/not affected/);
    }
  });

  it("never offers to clear credentials or names a credential location", () => {
    const copy = localDataControls(false).map(
      (control) =>
        control.label + " " + control.note + " " + control.confirmTitle + " " +
        control.confirmBody + " " + control.confirmAction + " " + control.success + " " +
        control.failure,
    ).join(" | ");
    expect(copy).not.toMatch(/clear\s+(your\s+)?(provider\s+)?credentials/i);
    expect(copy).not.toMatch(/auth\.json|credentials\.json|\.codex|credential manager/i);
    expect(copy).not.toMatch(/[a-z]:\\|[\\/](users|appdata)[\\/]/i);
  });

  it("exposes fixed command names only", () => {
    for (const command of Object.values(LOCAL_DATA_COMMANDS)) {
      expect(command).toMatch(/^[a-z_]+$/);
    }
  });

  it("varies only the execution row while an execution run is active", () => {
    const inactive = localDataControls(false);
    const active = localDataControls(true);
    expect(active.map((control) => control.id)).toEqual(
      inactive.map((control) => control.id),
    );
    expect(active.find((control) => control.id === "executionRuns")).toMatchObject({
      confirmTitle: "Discard the active execution run and clear?",
      confirmAction: "Discard active run and clear",
    });
  });
});
