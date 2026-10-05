import { invoke } from "@tauri-apps/api/core";
import { clearRuntimeHistory, removeLegacyHistory } from "./historyClient";
import { isRunningInTauri } from "./runtime";
import {
  clearExecutionRunsState,
  loadExecutionRunsState,
  type ClearExecutionRunsOptions,
} from "./executionRuns";
import {
  resetFloatingQuotaPrefs,
  type FloatingQuotaPrefs,
} from "./floatingWindowPrefs";
import { resetSettings, type Settings } from "./settings";

/**
 * The owned local data service (v0.7).
 *
 * This module is the only place the product surface talks to when the user
 * deliberately clears LimitScope-owned data. It exists so the settings
 * component never learns a filesystem path, a storage key, or a wire
 * argument: each operation targets one fixed, application-owned store.
 *
 * Ownership split:
 *
 * - Rust owns `<app-data>/quota-history-v1.json` and
 *   `<app-data>/provider-last-good-v1.json`; this module reaches them only
 *   through fixed commands (see `LOCAL_DATA_COMMANDS`).
 * - The WebView owns `rate-limits.settings.v1`,
 *   `rate-limits.floating-quota.v1`, and `limitscope.execution-runs.v1`; the
 *   resets and execution clear go through their store APIs, which preserve
 *   unknown fields and require deliberate confirmation for an active run.
 *
 * Not part of this surface, by construction: provider credentials and any
 * other application's files (`~/.codex/auth.json`, OpenCode/OpenCodex/ZCode/
 * Grok/Google credential stores, browser cookies, Windows credential stores),
 * user-saved diagnostic exports, the CLI-managed
 * `.limitscope-provenance-run.json` context file, and the autostart registry
 * entry that belongs to the preferences reset rather than to local data.
 */

export type LocalDataCategoryId =
  | "usageHistory"
  | "providerCache"
  | "executionRuns"
  | "preferences";

/**
 * Result of a clear operation. `ok: false` means the store could not be
 * cleared: the caller must not claim the data was removed. `removed: false`
 * is a success — the category was already empty, which is never an error.
 */
export type LocalDataClearResult =
  | { ok: true; removed: boolean }
  | { ok: false };

/**
 * Fixed wire names of the Rust-managed categories. Nothing here takes an
 * argument, so no path can be named from the WebView.
 */
export const LOCAL_DATA_COMMANDS = {
  /** Clears `<app-data>/provider-last-good-v1.json` (no arguments). */
  providerCache: "clear_provider_cache",
} as const;

export type LocalDataControl = {
  id: LocalDataCategoryId;
  /** Row label. */
  label: string;
  /** Secondary line: what this category actually is on disk. */
  note: string;
  /** Trigger button text. */
  action: string;
  /** Confirmation prompt that states exactly what is removed. */
  confirmTitle: string;
  confirmBody: string;
  /**
   * Confirming button text. It names the action in words, so the destructive
   * choice is identified by text and not by color alone.
   */
  confirmAction: string;
  /** Restrained inline success confirmation (never a modal). */
  success: string;
  /** Scoped failure text; never claims the data was removed. */
  failure: string;
};

/**
 * The Local data section's copy and order, in one place so the component and
 * its tests read the same contract.
 *
 * The execution-run row's confirmation names the active run when one is in
 * flight, so discarding it is always a deliberate, worded choice.
 */
export function localDataControls(
  activeExecutionRun: boolean,
): readonly LocalDataControl[] {
  return [
  {
    id: "usageHistory",
    label: "Usage history",
    note: "Quota observations kept for 24 hours on this device.",
    action: "Clear",
    confirmTitle: "Clear usage history?",
    confirmBody:
      "This removes locally stored quota observations. Your settings and provider credentials are not affected.",
    confirmAction: "Clear usage history",
    success: "Usage history cleared.",
    failure: "Could not clear usage history.",
  },
  {
    id: "providerCache",
    label: "Provider cache",
    note: "Last known quota retained for the next start.",
    action: "Clear",
    confirmTitle: "Clear provider cache?",
    confirmBody:
      "This removes retained last-known quota data. Your settings and provider credentials are not affected; LimitScope rebuilds the cache on the next successful refresh.",
    confirmAction: "Clear provider cache",
    success: "Provider cache cleared.",
    failure: "Could not clear provider cache.",
  },
    activeExecutionRun
      ? {
          id: "executionRuns",
          label: "Execution runs",
          note: "Local execution run records; one run is currently active.",
          action: "Clear",
          confirmTitle: "Discard the active execution run and clear?",
          confirmBody:
            "This discards the active execution run and removes all stored run records. Your quota history, settings, and provider credentials are not affected.",
          confirmAction: "Discard active run and clear",
          success: "Execution runs cleared.",
          failure: "Could not clear execution runs.",
        }
      : {
          id: "executionRuns",
          label: "Execution runs",
          note: "Local execution run records on this device.",
          action: "Clear",
          confirmTitle: "Clear execution runs?",
          confirmBody:
            "This removes locally stored execution run records. Your quota history, settings, and provider credentials are not affected.",
          confirmAction: "Clear execution runs",
          success: "Execution runs cleared.",
          failure: "Could not clear execution runs.",
        },
  {
    id: "preferences",
    label: "Preferences",
    note: "Theme, refresh interval, notifications, providers, floating bar.",
    action: "Reset",
    confirmTitle: "Reset preferences?",
    confirmBody:
      "This restores LimitScope preferences to their defaults. Your quota history and provider credentials are not affected.",
    confirmAction: "Reset preferences",
    success: "Preferences reset.",
    failure: "Could not reset preferences.",
  },
  ];
}

export function hasActiveExecutionRun(): boolean {
  return loadExecutionRunsState().activeRun !== null;
}

/**
 * Clears the Rust-owned provider last-good cache through its fixed command.
 * The clear targets persisted recovery data only: the current in-session
 * runtime snapshot remains visible until the next normal refresh replaces it.
 *
 * Outside Tauri (plain-browser dev, tests without a backend) there is no
 * store to clear and the call is a successful no-op.
 */
export async function clearProviderCache(): Promise<LocalDataClearResult> {
  if (!isRunningInTauri()) return { ok: true, removed: false };
  try {
    const outcome = await invoke<{ removed?: boolean } | null>(
      LOCAL_DATA_COMMANDS.providerCache,
    );
    return { ok: true, removed: outcome?.removed === true };
  } catch (error) {
    console.error("Failed to clear the provider cache", error);
    return { ok: false };
  }
}

/**
 * Clears the Rust-owned quota history plus the retired legacy localStorage
 * blob. The Rust store's canonical `clear_history` command is the production
 * clear path, so its invariants (memory + file, revision bump) are used
 * rather than a file deletion here.
 */
export async function clearUsageHistoryStore(): Promise<LocalDataClearResult> {
  try {
    await clearRuntimeHistory();
    removeLegacyHistory();
    return { ok: true, removed: true };
  } catch (error) {
    console.error("Failed to clear the quota history", error);
    return { ok: false };
  }
}

/**
 * Clears the WebView-owned execution-runs store through its own API. The
 * caller must pass deliberate confirmation to remove an active run; without
 * it, completed runs clear and the active run is preserved. Exported receipts
 * are user artifacts and are deliberately out of scope.
 */
export async function clearExecutionRuns(
  options: ClearExecutionRunsOptions,
): Promise<LocalDataClearResult> {
  try {
    const before = loadExecutionRunsState();
    const result = clearExecutionRunsState(options);
    const removed =
      before.recentRuns.length > 0 ||
      (before.activeRun !== null && !result.activeRunPreserved);
    return { ok: true, removed };
  } catch (error) {
    console.error("Failed to clear the execution runs", error);
    return { ok: false };
  }
}

export type PreferenceResetResult = {
  settings: Settings;
  floating: FloatingQuotaPrefs;
};

/**
 * Resets the WebView-owned preference stores to their canonical defaults.
 * Both writes go through the store APIs, so unknown/future fields written by
 * another lane survive: an explicit reset clears what LimitScope knows it
 * owns and nothing else.
 */
export function resetPreferenceStores(): PreferenceResetResult {
  return {
    settings: resetSettings(),
    floating: resetFloatingQuotaPrefs(),
  };
}
