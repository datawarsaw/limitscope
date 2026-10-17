import { invoke } from "@tauri-apps/api/core";

/**
 * OpenDesign Cloud usage — the manual, read-only Windows accessibility read.
 *
 * The backend command (refresh_opendesign_usage) is invoked exclusively from
 * an explicit user action in the OpenDesign panel; this module deliberately
 * exposes no polling, scheduling, or auto-refresh of its own. The user owns
 * navigation: Chrome must already show the signed-in Plan & balance screen
 * for a read to succeed.
 *
 * The backend reports the share the page itself states as remaining. Nothing
 * here converts it into a used percentage, and nothing derives an absolute
 * reset time from the page's relative countdown.
 */
export type OpenDesignStatus =
  | "ok"
  | "chrome_not_running"
  | "dashboard_not_visible"
  | "unknown";

/** Which allowance a reading belongs to, exactly as the page names it. */
export type OpenDesignWindowLabel = "5-hour" | "7-day";

/** One allowance reading. remainingPercent is the page's remaining share. */
export interface OpenDesignWindow {
  label: OpenDesignWindowLabel;
  remainingPercent: number;
  /** The page's own countdown line, verbatim. */
  resetText?: string;
}

/** One successful reading as the backend reports it (camelCase wire). */
export interface OpenDesignReading {
  windows: OpenDesignWindow[];
  plan?: string;
  periodEndsText?: string;
  observedAt: string;
}

/** The refresh command's answer: the fresh attempt plus the retained reading. */
export interface OpenDesignRefreshResult {
  status: OpenDesignStatus;
  observedAt: string;
  windows?: OpenDesignWindow[];
  plan?: string;
  periodEndsText?: string;
  lastKnown?: OpenDesignReading;
}

const WINDOW_LABELS: readonly OpenDesignWindowLabel[] = ["5-hour", "7-day"];
const STATUSES: readonly OpenDesignStatus[] = [
  "ok",
  "chrome_not_running",
  "dashboard_not_visible",
  "unknown",
];

function isWindowLabel(value: unknown): value is OpenDesignWindowLabel {
  return (WINDOW_LABELS as readonly unknown[]).includes(value);
}

function isStatus(value: unknown): value is OpenDesignStatus {
  return (STATUSES as readonly unknown[]).includes(value);
}

/**
 * Defensive validation of one window list: only the two known labels, each
 * once, with a finite percentage inside 0–100 survive. Anything malformed is
 * dropped rather than rounded, zeroed, or relabelled.
 */
export function sanitizeOpenDesignWindows(
  windows: unknown,
): OpenDesignWindow[] {
  if (!Array.isArray(windows)) return [];
  const seen = new Set<OpenDesignWindowLabel>();
  const out: OpenDesignWindow[] = [];
  for (const entry of windows) {
    if (!entry || typeof entry !== "object") continue;
    const record = entry as Record<string, unknown>;
    const label = record.label;
    const percent = record.remainingPercent;
    if (!isWindowLabel(label) || seen.has(label)) continue;
    if (typeof percent !== "number" || !Number.isFinite(percent)) continue;
    if (percent < 0 || percent > 100) continue;
    seen.add(label);
    out.push({
      label,
      remainingPercent: percent,
      ...(typeof record.resetText === "string" && record.resetText.trim()
        ? { resetText: record.resetText }
        : {}),
    });
  }
  // Stable, page-defined order: 5-hour first, then 7-day.
  return WINDOW_LABELS.map((label) =>
    out.find((window) => window.label === label),
  ).filter((window): window is OpenDesignWindow => window !== undefined);
}

function sanitizeReading(value: unknown): OpenDesignReading | undefined {
  if (!value || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  const windows = sanitizeOpenDesignWindows(record.windows);
  if (windows.length !== 2) return undefined;
  if (typeof record.observedAt !== "string" || !record.observedAt) return undefined;
  return {
    windows,
    ...(typeof record.plan === "string" && record.plan
      ? { plan: record.plan }
      : {}),
    ...(typeof record.periodEndsText === "string" && record.periodEndsText
      ? { periodEndsText: record.periodEndsText }
      : {}),
    observedAt: record.observedAt,
  };
}

/**
 * The one explicit acquisition entry point. Transport failures (and non-Tauri
 * contexts such as the dev browser) surface as an unknown attempt — the
 * reading contract has no error branch that could masquerade as data. Pass
 * the previous attempt so a rejected invoke keeps that reading and its
 * original observation stamp instead of inventing a new one.
 */
export async function refreshOpenDesignUsage(
  previous?: OpenDesignRefreshResult | null,
): Promise<OpenDesignRefreshResult> {
  try {
    const result = await invoke<unknown>("refresh_opendesign_usage");
    const parsed = parseRefreshResult(result);
    if (parsed) return parsed;
  } catch (error) {
    console.warn("opendesign refresh unavailable", error);
  }
  const lastKnown = openDesignPreservedReading(previous);
  return {
    status: "unknown",
    observedAt: new Date().toISOString(),
    ...(lastKnown ? { lastKnown } : {}),
  };
}

/**
 * Validates one command answer. The typed status is preserved; the values are
 * only attached to an ok answer that really carries both windows, so a
 * malformed success cannot be shown as data.
 */
export function parseRefreshResult(
  value: unknown,
): OpenDesignRefreshResult | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (!isStatus(record.status)) return null;
  const observedAt =
    typeof record.observedAt === "string" && record.observedAt
      ? record.observedAt
      : new Date().toISOString();
  const lastKnown = sanitizeReading(record.lastKnown);
  if (record.status !== "ok") {
    return {
      status: record.status,
      observedAt,
      ...(lastKnown ? { lastKnown } : {}),
    };
  }
  const windows = sanitizeOpenDesignWindows(record.windows);
  if (windows.length !== 2) return null;
  return {
    status: "ok",
    observedAt,
    windows,
    ...(typeof record.plan === "string" && record.plan
      ? { plan: record.plan }
      : {}),
    ...(typeof record.periodEndsText === "string" && record.periodEndsText
      ? { periodEndsText: record.periodEndsText }
      : {}),
    ...(lastKnown ? { lastKnown } : {}),
  };
}

/**
 * The reading a failed transport call must keep. A fresh success contributes
 * its own stamp; a later failure contributes the retained last-known stamp,
 * never the failure's own timestamp.
 */
export function openDesignPreservedReading(
  previous: OpenDesignRefreshResult | null | undefined,
): OpenDesignReading | undefined {
  return openDesignEffectiveReading(previous ?? null).reading ?? undefined;
}

/** What the panel should display after a refresh attempt. */
export interface OpenDesignEffective {
  source: "fresh" | "last-known" | "none";
  reading: OpenDesignReading | null;
}

/**
 * A successful attempt shows its own reading; a failed attempt falls back to
 * the backend-retained last successful reading, which keeps its original
 * observation stamp so the UI can mark it as last known rather than fresh.
 */
export function openDesignEffectiveReading(
  result: OpenDesignRefreshResult | null,
): OpenDesignEffective {
  if (result && result.status === "ok") {
    const windows = sanitizeOpenDesignWindows(result.windows);
    if (windows.length === 2) {
      return {
        source: "fresh",
        reading: {
          windows,
          ...(result.plan ? { plan: result.plan } : {}),
          ...(result.periodEndsText
            ? { periodEndsText: result.periodEndsText }
            : {}),
          observedAt: result.observedAt,
        },
      };
    }
  }
  if (result?.lastKnown) {
    const reading = sanitizeReading(result.lastKnown);
    if (reading) return { source: "last-known", reading };
  }
  return { source: "none", reading: null };
}

/** The reading for one allowance, or undefined when it is not present. */
export function openDesignWindow(
  reading: OpenDesignReading | null,
  label: OpenDesignWindowLabel,
): OpenDesignWindow | undefined {
  return reading?.windows.find((window) => window.label === label);
}

/**
 * Human guidance for the states that carry no fresh values, shown instead of
 * data. null for a fresh successful read — there is nothing to explain.
 */
export function openDesignStatusMessage(
  result: OpenDesignRefreshResult | null,
): string | null {
  if (!result) {
    return "No OpenDesign data yet — open Plan & balance in Chrome, then Refresh.";
  }
  switch (result.status) {
    case "ok":
      return null;
    case "chrome_not_running":
      return "Chrome isn't running.";
    case "dashboard_not_visible":
      return "Open the OpenDesign Plan & balance page in Chrome, then Refresh.";
    case "unknown":
      return "Couldn't read OpenDesign usage.";
  }
}

