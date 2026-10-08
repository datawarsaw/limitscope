import { invoke } from "@tauri-apps/api/core";

/**
 * Grok Bot desktop usage — the manual, read-only Windows accessibility read.
 *
 * The backend command (`refresh_grok_bot_usage`) is invoked exclusively from
 * an explicit user action; this module deliberately exposes no polling,
 * scheduling, or auto-refresh of its own. The user owns navigation: Grok Bot
 * must already show Settings → Usage & Billing for a read to succeed.
 */
export type GrokBotStatus = "ok" | "not_running" | "screen_not_visible" | "unknown";

/** One successful reading as the backend reports it (camelCase wire). */
export interface GrokBotReading {
  usedPercent?: number;
  resetText?: string;
  appVersion?: string;
  observedAt: string;
}

/** The refresh command's answer: the fresh attempt plus the retained reading. */
export interface GrokBotRefreshResult extends GrokBotReading {
  status: GrokBotStatus;
  lastKnown?: GrokBotReading;
}

/**
 * The one explicit acquisition entry point. Transport failures (and non-
 * Tauri contexts such as the dev browser) surface as an `unknown` attempt —
 * the reading contract has no error branch that could masquerade as data.
 */
export async function refreshGrokBotUsage(): Promise<GrokBotRefreshResult> {
  try {
    return await invoke<GrokBotRefreshResult>("refresh_grok_bot_usage");
  } catch (error) {
    console.warn("grok bot refresh unavailable", error);
    return { status: "unknown", observedAt: new Date().toISOString() };
  }
}

/**
 * Remaining quota derived from the verified used percentage. Unavailable
 * data stays `null` — it is never presented as 0% remaining.
 */
export function grokBotRemainingPercent(
  usedPercent: number | null | undefined,
): number | null {
  if (usedPercent === null || usedPercent === undefined) return null;
  if (!Number.isFinite(usedPercent)) return null;
  const remaining = 100 - usedPercent;
  if (remaining < 0 || remaining > 100) return null;
  return Math.round(remaining * 100) / 100;
}

/** What the card should display after a refresh attempt. */
export interface GrokBotEffective {
  source: "fresh" | "last-known" | "none";
  reading: GrokBotReading | null;
}

/**
 * A successful attempt shows its own reading; a failed attempt falls back to
 * the backend-retained last successful reading, which keeps its original
 * observation stamp so the UI can mark it as last known rather than fresh.
 */
export function grokBotEffectiveReading(
  result: GrokBotRefreshResult | null,
): GrokBotEffective {
  if (result && result.status === "ok") {
    return { source: "fresh", reading: result };
  }
  if (result?.lastKnown) {
    return { source: "last-known", reading: result.lastKnown };
  }
  return { source: "none", reading: null };
}

/**
 * Human guidance for the non-ok states, shown instead of values. `null` for
 * a fresh successful read — there is nothing to explain.
 */
export function grokBotStatusMessage(result: GrokBotRefreshResult | null): string | null {
  if (!result) {
    return "No Grok Bot data yet — open Grok Bot Settings → Usage & Billing, then Refresh.";
  }
  switch (result.status) {
    case "ok":
      return null;
    case "not_running":
      return "Grok Bot isn't running.";
    case "screen_not_visible":
      return "Open Usage & Billing in Grok Bot settings, then Refresh.";
    case "unknown":
      return "Couldn't read Grok Bot usage.";
  }
}
