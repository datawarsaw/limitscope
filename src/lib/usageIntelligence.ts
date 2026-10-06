import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./floatingWindowChrome";

/**
 * Usage Intelligence (v0.8.9) — the token-usage data plane client.
 *
 * These are REPORTED token counts collected locally from supported AI
 * tools' own databases (Phase 1: ZCode), never account quota, billing
 * usage, exact spend, or complete historical usage. Historical backfill
 * is deliberately not imported: collection starts at the first enable,
 * surfaced through `collectionStartedAt` / `incompleteHistory`.
 */

export type UsageIntelligenceRange = "today" | "7d" | "30d";

export type UsageIntelligenceQuery = {
  range: UsageIntelligenceRange;
  /** Local midnight (epoch ms) from this device's clock; Today only. */
  todayStartMs?: number;
};

/** Token sums shared by every aggregation level (totals, provider, model). */
export type UsageTokenSums = {
  events: number;
  inputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  totalTokens: number;
};

export type UsageModelBreakdown = UsageTokenSums & {
  model: string;
};

export type UsageProviderBreakdown = UsageTokenSums & {
  provider: string;
  models: UsageModelBreakdown[];
};

export type UsageSourceDiagnostics = {
  source: string;
  state:
    | "disabled"
    | "collecting"
    | "sourceAbsent"
    | "schemaUnsupported"
    | "readFailure"
    | "scanCeiling"
    | "ok";
  watermarkAt: string;
  baselinedAt: string;
  lastObservedAt?: string;
  detail?: string;
  lastScan?: {
    accepted: number;
    duplicates: number;
    rejected: number;
  };
};

export type UsageIntelligence = {
  schemaVersion: 1;
  range: UsageIntelligenceRange;
  rangeStart: string;
  rangeEnd: string;
  generatedAt: string;
  enabled: boolean;
  eventsInRange: number;
  groups: UsageProviderBreakdown[];
  totals: UsageTokenSums;
  collectionStartedAt?: string;
  incompleteHistory: boolean;
  sources: UsageSourceDiagnostics[];
  semantics: "reportedTokenUsageCollectedLocally";
};

/** Reads the Today / 7d / 30d aggregation from the Rust-owned store. */
export async function loadUsageIntelligence(
  query: UsageIntelligenceQuery,
): Promise<UsageIntelligence> {
  if (!isTauriRuntime()) {
    throw new Error("Usage Intelligence requires the Tauri runtime");
  }
  return invoke<UsageIntelligence>("get_usage_intelligence", { query });
}

/** The one intelligence source; injectable in tests. */
export type UsageIntelligenceLoader = typeof loadUsageIntelligence;

/** Pushes the opt-in toggle to the Rust store (persisted there). */
export async function pushUsageIntelligenceEnabled(
  enabled: boolean,
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("set_usage_intelligence_enabled", { enabled });
}

/** Clears the owned Usage Intelligence store (fixed command, no args). */
export async function clearUsageIntelligenceStoreCommand(): Promise<{
  removed?: boolean;
}> {
  return invoke<{ removed?: boolean }>("clear_usage_intelligence");
}

/**
 * Local midnight of `date` on this device, as epoch ms — the honest Today
 * boundary the backend sanity-checks. Exported for tests.
 */
export function localMidnightMs(date: Date = new Date()): number {
  const midnight = new Date(date);
  midnight.setHours(0, 0, 0, 0);
  return midnight.getTime();
}

/** Compact token counts for fast comparison (K / M / B suffixes). */
export function formatTokens(value: number): string {
  if (!Number.isFinite(value) || value < 0) return "—";
  if (value >= 1_000_000_000) return `${trimZeros(value / 1_000_000_000)}B`;
  if (value >= 1_000_000) return `${trimZeros(value / 1_000_000)}M`;
  if (value >= 10_000) return `${trimZeros(value / 1_000)}K`;
  return `${value}`;
}

function trimZeros(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return `${rounded}`;
}

/** The cached column: cache read plus cache write. */
export function cachedTokens(breakdown: {
  cacheReadTokens: number;
  cacheWriteTokens: number;
}): number {
  return breakdown.cacheReadTokens + breakdown.cacheWriteTokens;
}

/** Display label for the normalized provider axis. */
export const USAGE_PROVIDER_LABELS: Record<string, string> = {
  zai: "Z.ai (ZCode)",
  "openai-codex": "OpenAI / Codex",
  unknown: "Unknown provider",
};

export function usageProviderLabel(provider: string): string {
  return USAGE_PROVIDER_LABELS[provider] ?? provider;
}

/** Friendly wording of a source's diagnostic state. */
export function usageSourceStateText(state: UsageSourceDiagnostics["state"]): string {
  switch (state) {
    case "ok":
      return "collecting";
    case "collecting":
      return "collecting now";
    case "sourceAbsent":
      return "source not found";
    case "schemaUnsupported":
      return "source schema not supported";
    case "readFailure":
      return "could not read source";
    case "scanCeiling":
      return "too many new rows to scan safely";
    case "disabled":
      return "off";
  }
}
