import { invoke } from "@tauri-apps/api/core";
import { LEGACY_QUOTA_HISTORY_KEY, type QuotaObservation } from "./quotaHistory";
import { isTauriRuntime } from "./floatingWindowChrome";

/**
 * TS side of the Rust-owned quota history (v0.5 phase 2). The Rust runtime
 * records, retains, dedupes, heals, and clears history; this module is the
 * read/mutation boundary — there is no TS write path and no localStorage
 * history storage anymore. Outside Tauri (plain-browser dev, tests without
 * a backend) every call degrades to "no history" instead of throwing.
 */

/** Outcome of the one-time legacy localStorage import. */
export type ImportOutcome = {
  accepted: number;
  rejected: number;
};

export type HistoryRange = "24h" | "7d" | "30d";

export type HistoryRangeParams = {
  providerId?: string;
  account?: string | null;
  windowLabel?: string;
  range?: HistoryRange;
  exactAccount?: boolean;
};

/** Reads the full history from the Rust store. */
export async function loadRuntimeHistory(): Promise<QuotaObservation[]> {
  if (!isTauriRuntime()) return [];
  return invoke<QuotaObservation[]>("get_history");
}

/**
 * Reads a range-bounded quota history from the Rust store, suitable for UI
 * trend visualization. When parameters are omitted, returns all retained
 * windows across all providers for the requested range.
 */
export async function loadRuntimeHistoryRange(
  params: HistoryRangeParams = {},
): Promise<QuotaObservation[]> {
  if (!isTauriRuntime()) return [];
  return invoke<QuotaObservation[]>("get_history_range", {
    providerId: params.providerId ?? null,
    account: params.account === undefined ? null : params.account,
    windowLabel: params.windowLabel ?? null,
    range: params.range ?? "24h",
    exactAccount: params.exactAccount ?? params.account !== undefined,
  });
}

/** Clears the Rust-owned history (memory + persisted file). */
export async function clearRuntimeHistory(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("clear_history");
}

/**
 * Sends the legacy localStorage blob to the Rust store once. The Rust side
 * validates every entry and deduplicates, so the import is idempotent — a
 * retry (or two windows importing concurrently) can never duplicate
 * samples.
 */
export async function importLegacyHistory(
  observations: readonly unknown[],
): Promise<ImportOutcome> {
  if (!isTauriRuntime()) return { accepted: 0, rejected: 0 };
  return invoke<ImportOutcome>("import_legacy_history", { observations });
}

/**
 * Reads the legacy localStorage blob left by the pre-phase-2 TS store.
 * Returns `present: false` when there is nothing to migrate. An
 * unparseable blob is reported as garbage so the caller can remove it
 * without bothering the Rust side — invalid legacy history never blocks
 * startup.
 */
export function readLegacyHistory(): {
  present: boolean;
  observations: unknown[];
} {
  try {
    const raw = localStorage.getItem(LEGACY_QUOTA_HISTORY_KEY);
    if (raw === null) return { present: false, observations: [] };
    const parsed: unknown = JSON.parse(raw);
    if (Array.isArray(parsed)) return { present: true, observations: parsed };
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      Array.isArray((parsed as { observations?: unknown }).observations)
    ) {
      return {
        present: true,
        observations: (parsed as { observations: unknown[] }).observations,
      };
    }
    return { present: true, observations: [] };
  } catch {
    // Corrupt JSON: nothing to import, but the key must still go.
    return { present: true, observations: [] };
  }
}

/** Removes the legacy localStorage blob (after import or on clear). */
export function removeLegacyHistory(): void {
  try {
    localStorage.removeItem(LEGACY_QUOTA_HISTORY_KEY);
  } catch {
    // Best-effort: an unremovable key only means a harmless re-import
    // attempt on the next launch (the Rust import is idempotent).
  }
}
