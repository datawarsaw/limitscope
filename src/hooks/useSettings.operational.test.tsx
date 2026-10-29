// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useSettings } from "./useSettings";
import { DEFAULT_SETTINGS, SETTINGS_STORAGE_KEY } from "../lib/settings";
const mock = vi.hoisted(() => ({ invoke: vi.fn(), listeners: new Set<(event: {payload: unknown}) => void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mock.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async (_: string, callback: (event: {payload: unknown}) => void) => {
  mock.listeners.add(callback); return () => { mock.listeners.delete(callback); };
}}));
vi.mock("@tauri-apps/plugin-autostart", () => ({
  isEnabled: async () => false, enable: async () => {}, disable: async () => {},
}));
let persisted: { revision: number; quotaNotifications: boolean; monitored: Record<string, boolean> };
beforeEach(() => {
  Object.assign(window, {__TAURI_INTERNALS__: {}});
  persisted = {revision: 0, quotaNotifications: false, monitored: {openrouter: false, deepseek: false}};
  mock.invoke.mockReset();
  mock.invoke.mockImplementation(async (command, args) => {
    if (command === "get_codex_accounts") return DEFAULT_SETTINGS.codexAccounts.accounts;
    if (command === "get_usage_intelligence") return {enabled:false};
    if (command === "get_operational_settings") return structuredClone(persisted);
    if (command === "set_wallet_monitoring" || command === "set_quota_notifications_enabled") {
      persisted = {...persisted, revision:persisted.revision+1,
        monitored: {...persisted.monitored, ...args.monitored},
        quotaNotifications: args.enabled ?? persisted.quotaNotifications};
      for (const listener of mock.listeners) listener({payload:structuredClone(persisted)});
      return structuredClone(persisted);
    }
  });
});
afterEach(() => { cleanup(); localStorage.clear(); mock.listeners.clear(); delete (window as any).__TAURI_INTERNALS__; });
it("preserves disabled monitoring with missing storage and synchronizes both windows", async () => {
  const main = renderHook(useSettings);
  const floating = renderHook(useSettings);
  await waitFor(() => expect(main.result.current.settings.walletPreferences.monitored.openrouter).toBe(false));
  expect(mock.invoke.mock.calls.some(([name]) => name.startsWith("set_"))).toBe(false);
  act(() => main.result.current.setWalletMonitored("openrouter", true));
  await waitFor(() => expect(floating.result.current.settings.walletPreferences.monitored.openrouter).toBe(true));
  expect(persisted.monitored.deepseek).toBe(false);
  act(() => {
    for (const listener of mock.listeners) listener({payload:{revision:0, quotaNotifications:true, monitored:{openrouter:false}}});
    localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify({...DEFAULT_SETTINGS,quotaNotifications:true}));
    window.dispatchEvent(new StorageEvent("storage", {key:SETTINGS_STORAGE_KEY}));
  });
  expect(main.result.current.settings.walletPreferences.monitored.openrouter).toBe(true);
  expect(main.result.current.settings.quotaNotifications).toBe(false);
});
it("failed preference write keeps the durable choice and supports identical retry", async () => {
  const view = renderHook(useSettings);
  await waitFor(() => expect(view.result.current.settings.walletPreferences.monitored.openrouter).toBe(false));
  const original = mock.invoke.getMockImplementation()!;
  mock.invoke.mockImplementation(async (name, args) => {
    if (name === "set_quota_notifications_enabled") throw new Error("injected disk failure");
    return original(name,args);
  });
  act(() => view.result.current.setQuotaNotifications(true));
  await waitFor(() => expect(view.result.current.startupError).toContain("could not be saved"));
  expect(view.result.current.settings.quotaNotifications).toBe(false);
  mock.invoke.mockImplementation(original);
  act(() => view.result.current.setQuotaNotifications(true));
  await waitFor(() => expect(view.result.current.settings.quotaNotifications).toBe(true));
});
it("late initial reply cannot replace a newer event", async () => {
  let resolve!: (value: unknown) => void;
  const original = mock.invoke.getMockImplementation()!;
  mock.invoke.mockImplementation((name,args) => name === "get_operational_settings" ?
    new Promise(done => {resolve=done;}) : original(name,args));
  const view = renderHook(useSettings);
  await waitFor(() => expect(resolve).toBeTypeOf("function"));
  act(() => {
    for (const listener of mock.listeners) listener({payload:{revision:3,quotaNotifications:true,monitored:{openrouter:false}}});
    resolve({revision:1,quotaNotifications:false,monitored:{openrouter:true}});
  });
  await waitFor(() => expect(view.result.current.settings.quotaNotifications).toBe(true));
  expect(view.result.current.settings.walletPreferences.monitored.openrouter).toBe(false);
});
