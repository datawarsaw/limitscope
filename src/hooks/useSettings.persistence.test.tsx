// @vitest-environment jsdom
import { StrictMode, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { useSettings } from "./useSettings";
import { DEFAULT_SETTINGS, SETTINGS_STORAGE_KEY } from "../lib/settings";
import type { CodexAccountSettings } from "../lib/codexAccounts";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { SettingsView } from "../components/settings/SettingsView";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/plugin-autostart", () => ({
  isEnabled: vi.fn().mockResolvedValue(false),
  enable: vi.fn().mockResolvedValue(undefined),
  disable: vi.fn().mockResolvedValue(undefined),
}));

const originalAccounts: CodexAccountSettings[] = [
  { accountId: "openai-codex", alias: "Main", codexHome: "C:\\fixture\\main", monitored: true },
  { accountId: "openai-codex:secondary", alias: "Cowork", codexHome: "C:\\fixture\\cowork", monitored: false },
];
let rustAccounts: CodexAccountSettings[];
let rustEnabled: boolean;

function protectedWrites() {
  return mocks.invoke.mock.calls.filter(([command]) =>
    command === "set_codex_accounts" || command === "set_usage_intelligence_enabled",
  );
}

function stored() {
  return JSON.parse(localStorage.getItem(SETTINGS_STORAGE_KEY)!);
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

// Same ownership as App: the hook stays mounted while Settings comes and goes.
function SettingsNavigation() {
  const preferences = useSettings();
  const [open, setOpen] = useState(false);
  const noop = () => {};
  const cleared = () => ({ ok: true as const, removed: false });
  return <>
    <button onClick={() => setOpen(!open)}>Toggle Settings</button>
    {open && <SettingsView {...preferences}
      usages={[]} wallets={[]} visibleUsages={[]} registryIds={[]}
      onClearUsageHistory={cleared} onClearExecutionRuns={cleared}
      activeExecutionRun={false} historyUnavailable={false}
      updatePhase="idle" currentVersion={null} availableUpdate={null} updateError={null}
      onCheckForUpdates={noop} onInstallUpdate={noop} onDismissUpdate={noop}
    />}
  </>;
}

beforeEach(() => {
  vi.mocked(isEnabled).mockResolvedValue(false);
  vi.mocked(enable).mockResolvedValue(undefined);
  vi.mocked(disable).mockResolvedValue(undefined);
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  localStorage.clear();
  rustAccounts = structuredClone(originalAccounts);
  rustEnabled = true;
  mocks.invoke.mockReset();
  mocks.invoke.mockImplementation(async (command, args) => {
    switch (command) {
      case "get_codex_accounts": return structuredClone(rustAccounts);
      case "get_usage_intelligence": return { enabled: rustEnabled };
      case "set_codex_accounts": rustAccounts = structuredClone(args.accounts); return true;
      case "set_usage_intelligence_enabled": rustEnabled = args.enabled; return;
    }
  });
});

afterEach(() => {
  cleanup();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("Rust-owned settings persistence", () => {
  it.each(["missing", "stale", "corrupt"])("preserves Main / Cowork and opt-in with %s WebView state, including StrictMode", async (cache) => {
    if (cache === "stale") {
      localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify({
        ...DEFAULT_SETTINGS,
        codexAccounts: { accounts: originalAccounts.map((a, i) => ({
          ...a, alias: i === 0 ? "Personal" : "Work", codexHome: "", monitored: true,
        })) },
        usageIntelligence: false,
      }));
    } else if (cache === "corrupt") {
      localStorage.setItem(SETTINGS_STORAGE_KEY, "{broken");
    }
    const { result } = renderHook(() => useSettings(), { wrapper: StrictMode });
    await waitFor(() => {
      expect(result.current.settings.codexAccounts.accounts).toEqual(originalAccounts);
      expect(result.current.settings.usageIntelligence).toBe(true);
    });
    expect(protectedWrites()).toEqual([]);
    expect(rustAccounts).toEqual(originalAccounts);
    expect(rustEnabled).toBe(true);
  });

  it("ignores stale storage events and unrelated preference changes", async () => {
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.settings.usageIntelligence).toBe(true));
    act(() => {
      localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify({ ...DEFAULT_SETTINGS, theme: "oled" }));
      window.dispatchEvent(new StorageEvent("storage", { key: SETTINGS_STORAGE_KEY }));
    });
    expect(result.current.settings.theme).toBe("oled");
    expect(result.current.settings.codexAccounts.accounts).toEqual(originalAccounts);
    expect(result.current.settings.usageIntelligence).toBe(true);
    act(() => result.current.setQuotaPerspective("remaining"));
    expect(stored().usageIntelligence).toBe(true);
    expect(protectedWrites()).toEqual([]);
  });

  it("persists explicit changes and preserves the other account's home and monitoring flag", async () => {
    const { result } = renderHook(() => useSettings(), { wrapper: StrictMode });
    await waitFor(() => expect(result.current.settings.usageIntelligence).toBe(true));
    act(() => {
      result.current.setCodexAccount("openai-codex", { alias: "Primary" });
      result.current.setCodexAccount("openai-codex:secondary", { alias: "Team" });
      result.current.setUsageIntelligence(false);
    });
    await waitFor(() => {
      expect(rustAccounts.map((a) => a.alias)).toEqual(["Primary", "Team"]);
      expect(rustEnabled).toBe(false);
      expect(stored().codexAccounts.accounts).toEqual(rustAccounts);
    });
    expect(rustAccounts[0]).toEqual({ ...originalAccounts[0], alias: "Primary" });
    expect(rustAccounts[1]).toEqual({ ...originalAccounts[1], alias: "Team" });
    expect(protectedWrites()).toHaveLength(3);
    expect(stored().usageIntelligence).toBe(false);
  });

  it("applies an edit during initialization to Rust values and rejects a late initialization reply", async () => {
    const accounts = deferred<CodexAccountSettings[]>();
    const usage = deferred<{ enabled: boolean }>();
    mocks.invoke.mockImplementationOnce(() => accounts.promise);
    // Hold the two initial reads while an explicit edit reaches Rust.
    mocks.invoke.mockImplementationOnce(() => usage.promise);
    const { result } = renderHook(() => useSettings());
    act(() => {
      result.current.setCodexAccount("openai-codex", { alias: "Edited" });
      result.current.setUsageIntelligence(false);
    });
    await waitFor(() => expect(rustAccounts[0].alias).toBe("Edited"));
    await act(async () => {
      accounts.resolve(structuredClone(originalAccounts));
      usage.resolve({ enabled: true });
    });
    expect(result.current.settings.codexAccounts.accounts).toEqual(rustAccounts);
    expect(rustAccounts[1]).toEqual(originalAccounts[1]);
    expect(result.current.settings.usageIntelligence).toBe(false);
    expect(rustEnabled).toBe(false);
  });

  it("does not write defaults if initial reads fail", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    mocks.invoke.mockImplementation(async (command) => {
      if (command.startsWith("get_")) throw new Error("read failed");
    });
    renderHook(() => useSettings());
    await waitFor(() => expect(console.error).toHaveBeenCalled());
    expect(protectedWrites()).toEqual([]);
    expect(rustAccounts).toEqual(originalAccounts);
    expect(rustEnabled).toBe(true);
  });

  it("reopens the real Settings view and restarts the hook without unintended writes", async () => {
    const { unmount } = render(<SettingsNavigation />);
    fireEvent.click(screen.getByRole("button", { name: "Toggle Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Providers & Wallets" }));
    await waitFor(() => expect(screen.getByDisplayValue("Cowork")).toBeTruthy());
    const callsBeforeNavigation = mocks.invoke.mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: "Toggle Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Toggle Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Providers & Wallets" }));
    expect(screen.getByDisplayValue("Main")).toBeTruthy();
    expect(screen.getByDisplayValue("Cowork")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    expect((screen.getByRole("switch", { name: "Usage Intelligence" }) as HTMLInputElement).checked).toBe(true);
    expect(mocks.invoke.mock.calls).toHaveLength(callsBeforeNavigation);
    unmount();
    const restarted = renderHook(() => useSettings());
    await waitFor(() => expect(restarted.result.current.settings.codexAccounts.accounts).toEqual(originalAccounts));
    expect(restarted.result.current.settings.usageIntelligence).toBe(true);
    expect(protectedWrites()).toEqual([]);
  });

  it("does not enable collection from a stale true cache when Rust has opted out", async () => {
    rustEnabled = false;
    localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify({ ...DEFAULT_SETTINGS, usageIntelligence: true }));
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.settings.usageIntelligence).toBe(false));
    expect(protectedWrites()).toEqual([]);
    act(() => result.current.setUsageIntelligence(true));
    await waitFor(() => expect(rustEnabled).toBe(true));
    expect(protectedWrites()).toEqual([["set_usage_intelligence_enabled", { enabled: true }]]);
  });

  it("persists real Settings input events once and keeps them on reopening", async () => {
    render(<SettingsNavigation />);
    fireEvent.click(screen.getByRole("button", { name: "Toggle Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Providers & Wallets" }));
    await waitFor(() => expect(screen.getByDisplayValue("Main")).toBeTruthy());
    fireEvent.change(screen.getByDisplayValue("Main"), { target: { value: "Main edited" } });
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    fireEvent.click(screen.getByRole("switch", { name: "Usage Intelligence" }));
    await waitFor(() => {
      expect(rustAccounts[0].alias).toBe("Main edited");
      expect(rustEnabled).toBe(false);
    });
    expect(rustAccounts[1]).toEqual(originalAccounts[1]);
    const writes = structuredClone(protectedWrites());
    expect(writes).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Toggle Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Toggle Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Providers & Wallets" }));
    expect(screen.getByDisplayValue("Main edited")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Data & Privacy" }));
    expect((screen.getByRole("switch", { name: "Usage Intelligence" }) as HTMLInputElement).checked).toBe(false);
    expect(protectedWrites()).toEqual(writes);
  });

  it("never sends a stale account set when the read before an explicit edit fails", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.settings.usageIntelligence).toBe(true));
    mocks.invoke.mockRejectedValueOnce(new Error("account read failed"));
    act(() => result.current.setCodexAccount("openai-codex", { alias: "New" }));
    await waitFor(() => expect(console.error).toHaveBeenCalled());
    expect(rustAccounts).toEqual(originalAccounts);
    expect(protectedWrites()).toEqual([]);
    act(() => result.current.setCodexAccount("openai-codex", { alias: "Retried" }));
    await waitFor(() => expect(rustAccounts[0].alias).toBe("Retried"));
    expect(rustAccounts[1]).toEqual(originalAccounts[1]);
  });

  it("patches the latest Rust configuration rather than an older attached window's copy", async () => {
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.settings.codexAccounts.accounts).toEqual(originalAccounts));
    rustAccounts[1] = { ...rustAccounts[1], alias: "Updated elsewhere", codexHome: "C:\\fixture\\new-home" };
    act(() => result.current.setCodexAccount("openai-codex", { alias: "Explicit" }));
    await waitFor(() => expect(rustAccounts[0].alias).toBe("Explicit"));
    expect(rustAccounts[1]).toEqual({ ...originalAccounts[1], alias: "Updated elsewhere", codexHome: "C:\\fixture\\new-home" });
  });

  it("keeps the existing explicit preference reset effective despite a late initialization reply", async () => {
    const accounts = deferred<CodexAccountSettings[]>();
    const usage = deferred<{ enabled: boolean }>();
    mocks.invoke.mockImplementationOnce(() => accounts.promise);
    mocks.invoke.mockImplementationOnce(() => usage.promise);
    const { result } = renderHook(() => useSettings());
    await act(async () => { expect(await result.current.resetPreferences()).toEqual({ ok: true, removed: true }); });
    await act(async () => {
      accounts.resolve(structuredClone(originalAccounts));
      usage.resolve({ enabled: true });
    });
    expect(rustAccounts).toEqual(DEFAULT_SETTINGS.codexAccounts.accounts);
    expect(rustEnabled).toBe(false);
    expect(result.current.settings).toEqual(DEFAULT_SETTINGS);
  });

  it("keeps browser-only editing local", () => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    const { result } = renderHook(() => useSettings());
    act(() => {
      result.current.setCodexAccount("openai-codex", { alias: "Browser" });
      result.current.setUsageIntelligence(true);
    });
    expect(stored().codexAccounts.accounts[0].alias).toBe("Browser");
    expect(stored().usageIntelligence).toBe(true);
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});
