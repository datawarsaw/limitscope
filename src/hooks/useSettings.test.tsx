// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { useSettings } from "./useSettings";
import { DEFAULT_SETTINGS, SETTINGS_STORAGE_KEY } from "../lib/settings";
import {
  DEFAULT_FLOATING_PREFS,
  FLOATING_PREFS_STORAGE_KEY,
} from "../lib/floatingWindowPrefs";
import type { LocalDataClearResult } from "../lib/localData";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

vi.mock("@tauri-apps/plugin-autostart", () => ({
  isEnabled: vi.fn().mockResolvedValue(false),
  enable: vi.fn().mockResolvedValue(undefined),
  disable: vi.fn().mockResolvedValue(undefined),
}));

function simulateTauri(): void {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  window.localStorage.clear();
  vi.clearAllMocks();
});

describe("useSettings quota notification forwarding", () => {
  it("forwards the persisted toggle to the runtime lane on attach", async () => {
    simulateTauri();
    window.localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({ ...DEFAULT_SETTINGS, quotaNotifications: true }),
    );
    renderHook(() => useSettings());
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith(
        "set_quota_notifications_enabled",
        { enabled: true },
      );
    });
  });

  it("forwards every toggle change and persists it", async () => {
    simulateTauri();
    const { result } = renderHook(() => useSettings());
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith(
        "set_quota_notifications_enabled",
        { enabled: false },
      );
    });
    act(() => result.current.setQuotaNotifications(true));
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith(
        "set_quota_notifications_enabled",
        { enabled: true },
      );
    });
    expect(
      JSON.parse(window.localStorage.getItem(SETTINGS_STORAGE_KEY)!)
        .quotaNotifications,
    ).toBe(true);
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
  });

  it("never invokes in the plain-browser dev path", () => {
    // No __TAURI_INTERNALS__: the hook must stay inert outside Tauri.
    renderHook(() => useSettings());
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});

describe("useSettings cross-feature persistence", () => {
  it("keeps v0.6 and foreign fields when a core setting changes", () => {
    window.localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({
        launchAtStartup: false,
        refreshIntervalMinutes: 5,
        theme: "graphite",
        quotaNotifications: false,
        providerPreferences: { order: ["zai"], hidden: ["grok"] },
        quotaPerspective: "remaining",
        futurePluginSettings: { telemetryOptIn: false },
      }),
    );
    const { result } = renderHook(() => useSettings());

    act(() => result.current.setTheme("oled"));

    const stored = JSON.parse(window.localStorage.getItem(SETTINGS_STORAGE_KEY)!);
    expect(stored.theme).toBe("oled");
    expect(stored.providerPreferences).toEqual({ order: ["zai"], hidden: ["grok"] });
    expect(stored.quotaPerspective).toBe("remaining");
    expect(stored.futurePluginSettings).toEqual({ telemetryOptIn: false });
  });

  it("persists the quota perspective and provider preferences", () => {
    const { result } = renderHook(() => useSettings());

    act(() => result.current.setQuotaPerspective("remaining"));
    act(() =>
      result.current.setProviderPreferences({ order: ["grok"], hidden: [] }),
    );

    const stored = JSON.parse(window.localStorage.getItem(SETTINGS_STORAGE_KEY)!);
    expect(stored.quotaPerspective).toBe("remaining");
    expect(stored.providerPreferences).toEqual({ order: ["grok"], hidden: [] });
    expect(result.current.settings.quotaPerspective).toBe("remaining");
  });
});

describe('useSettings provider presentation preferences', () => {
  const ids = ['openai-codex', 'zai', 'grok'];

  function storedPrefs() {
    return JSON.parse(window.localStorage.getItem(SETTINGS_STORAGE_KEY)!).providerPreferences;
  }

  it('persists visibility and order changes', () => {
    const { result } = renderHook(() => useSettings());
    let hidden = false;
    act(() => {
      hidden = result.current.setProviderHidden('zai', true, ids);
    });
    expect(hidden).toBe(true);
    expect(storedPrefs().hidden).toEqual(['zai']);
    let moved = false;
    act(() => {
      moved = result.current.moveProvider('grok', -1, ids);
    });
    expect(moved).toBe(true);
    expect(storedPrefs().order).toEqual(['openai-codex', 'grok', 'zai']);
  });

  it('refuses to hide the final visible provider', () => {
    const { result } = renderHook(() => useSettings());
    const two = ['a', 'b'];
    let first = false;
    let second = false;
    act(() => {
      first = result.current.setProviderHidden('a', true, two);
    });
    act(() => {
      second = result.current.setProviderHidden('b', true, two);
    });
    expect(first).toBe(true);
    expect(second).toBe(false);
    expect(storedPrefs().hidden).toEqual(['a']);
  });
});

describe("useSettings reset preferences", () => {
  it("resets both preference stores, disables autostart, and mirrors the runtime", async () => {
    simulateTauri();
    window.localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({
        launchAtStartup: true,
        refreshIntervalMinutes: 30,
        theme: "oled",
        quotaNotifications: true,
        providerPreferences: { order: ["grok"], hidden: ["zai"] },
        quotaPerspective: "remaining",
        futurePluginSettings: { telemetryOptIn: false },
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
    const autostart = await import("@tauri-apps/plugin-autostart");

    const { result } = renderHook(() => useSettings());
    mocks.invoke.mockClear();

    let outcome: LocalDataClearResult | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: true, removed: true });
    expect(result.current.settings).toEqual(DEFAULT_SETTINGS);
    expect(autostart.disable).toHaveBeenCalledTimes(1);
    expect(autostart.enable).not.toHaveBeenCalled();
    expect(mocks.invoke).toHaveBeenCalledWith(
      "set_quota_notifications_enabled",
      { enabled: false },
    );
    expect(mocks.invoke).toHaveBeenCalledWith("set_refresh_interval", {
      minutes: 5,
    });
    // A preference reset never clears history or the provider cache.
    expect(mocks.invoke).not.toHaveBeenCalledWith("clear_history");
    expect(mocks.invoke).not.toHaveBeenCalledWith("clear_provider_cache");

    const storedSettings = JSON.parse(
      window.localStorage.getItem(SETTINGS_STORAGE_KEY)!,
    );
    expect(storedSettings.launchAtStartup).toBe(false);
    expect(storedSettings.theme).toBe("graphite");
    expect(storedSettings.quotaPerspective).toBe("used");
    expect(storedSettings.providerPreferences).toEqual({ order: [], hidden: [] });
    // Unknown fields another lane wrote are not LimitScope's to delete.
    expect(storedSettings.futurePluginSettings).toEqual({
      telemetryOptIn: false,
    });
    expect(
      JSON.parse(window.localStorage.getItem(FLOATING_PREFS_STORAGE_KEY)!),
    ).toEqual(DEFAULT_FLOATING_PREFS);
  });

  it("reports a scoped failure when the autostart entry cannot be updated", async () => {
    simulateTauri();
    const autostart = await import("@tauri-apps/plugin-autostart");
    vi.mocked(autostart.disable).mockRejectedValueOnce(
      new Error("registry entry is locked"),
    );
    const { result } = renderHook(() => useSettings());

    let outcome: LocalDataClearResult | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: false });
    expect(result.current.startupError).toBe("registry entry is locked");
  });

  it("resets the stores without OS interaction outside Tauri", async () => {
    window.localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({ ...DEFAULT_SETTINGS, theme: "oled" }),
    );
    const { result } = renderHook(() => useSettings());

    let outcome: LocalDataClearResult | undefined;
    await act(async () => {
      outcome = await result.current.resetPreferences();
    });

    expect(outcome).toEqual({ ok: true, removed: true });
    expect(
      JSON.parse(window.localStorage.getItem(SETTINGS_STORAGE_KEY)!).theme,
    ).toBe("graphite");
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});
