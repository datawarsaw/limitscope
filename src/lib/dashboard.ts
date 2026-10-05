import type { QuotaPrediction } from "./prediction/types";
import { isUsableQuotaWindow, primaryQuotaWindow } from "./quotaStrip";
import { visiblePrediction } from "./v03Integration";
import type { LimitWindow, ProviderUsage } from "../types";
import {
  quotaPresentation,
  type QuotaPerspective,
} from "./quotaPresentation";
import { CRITICAL_PERCENT, NEAR_LIMIT_PERCENT } from "./thresholds";

/**
 * Presentation-only derivations for the v0.5 main-window dashboard: which
 * provider the overview anchors on, what the provider rail shows, and what
 * the "Needs attention" rail may claim. Everything here reads existing
 * runtime snapshots — no window is averaged, summed, or invented, and every
 * quota number shown is a number a provider actually reported.
 */

/** Rail/detail short names for the five production providers. The wire names
 * (`usage.name`) are unchanged everywhere else; this is display compaction
 * for the narrow rail only, with a generic fallback for unknown providers. */
const SHORT_NAMES: Readonly<Record<string, string>> = {
  "openai-codex": "Codex",
  zai: "Z.ai",
  "opencode-go": "OpenCode",
  antigravity: "Antigravity",
  grok: "Grok",
};

export function shortProviderName(usage: ProviderUsage): string {
  const curated = SHORT_NAMES[usage.id];
  if (curated) return curated;
  // "OpenAI / Codex" → "Codex"; "Grok (xAI)" → "Grok".
  let name = usage.name;
  const slash = name.lastIndexOf(" / ");
  if (slash !== -1) name = name.slice(slash + 3);
  return name.replace(/\s*\([^)]*\)\s*$/, "").trim() || usage.name;
}

/** Rail-style short name straight from a provider id — for surfaces (the
 * Usage view's trend titles) that hold ids from history, not usages. */
export function shortProviderId(providerId: string): string {
  return SHORT_NAMES[providerId] ?? providerId;
}

/** Monogram badge for the rail: the short name's first character. */
export function providerMark(usage: ProviderUsage): string {
  const short = shortProviderName(usage);
  return (short.charAt(0) || usage.name.charAt(0) || "?").toUpperCase();
}

/** One rail entry: everything the navigation surface needs per provider. */
export type ProviderRailEntry = {
  usage: ProviderUsage;
  shortName: string;
  mark: string;
  /** The provider's primary window (highest usable percent), if it has one. */
  primary: LimitWindow | undefined;
  /** Whole percent of the primary window, rounded like every other readout. */
  percent: number | undefined;
};

export function providerRailEntries(
  usages: readonly ProviderUsage[],
): ProviderRailEntry[] {
  const seen = new Set<string>();
  const entries: ProviderRailEntry[] = [];
  for (const usage of usages) {
    if (seen.has(usage.id)) continue;
    seen.add(usage.id);
    const primary = primaryQuotaWindow(usage);
    entries.push({
      usage,
      shortName: shortProviderName(usage),
      mark: providerMark(usage),
      primary,
      percent: primary === undefined ? undefined : Math.round(primary.usedPercent),
    });
  }
  return entries;
}

/**
 * Deterministic default selection: the non-simulated provider whose primary
 * window has the highest usable used percent — the same concept the quota
 * strip uses to answer "who is closest to their limit?". Ties keep the
 * earliest provider in registry order. Providers are never combined, and a
 * provider without a usable window never wins by accident. If no real
 * provider qualifies (dev mock data only), the first usage anchors the view.
 */
export function defaultSelectedProviderId(
  usages: readonly ProviderUsage[],
): string | null {
  let bestId: string | null = null;
  let bestPercent = -1;
  const seen = new Set<string>();
  for (const usage of usages) {
    if (seen.has(usage.id)) continue;
    seen.add(usage.id);
    if (usage.simulated) continue;
    const primary = primaryQuotaWindow(usage);
    if (!primary) continue;
    if (primary.usedPercent > bestPercent) {
      bestPercent = primary.usedPercent;
      bestId = usage.id;
    }
  }
  if (bestId !== null) return bestId;
  const firstReal = usages.find((usage) => !usage.simulated);
  return (firstReal ?? usages[0])?.id ?? null;
}

/** Quota-window severity tones shared by the rail meters and the detail rows. */
export type UsageTone = "calm" | "warn" | "critical";

export function toneFor(percent: number): UsageTone {
  if (percent >= 85) return "critical";
  if (percent >= 65) return "warn";
  return "calm";
}

/** The kinds of things the attention rail may say, most severe first. */
export type AttentionKind =
  | "critical" // primary window at/over 95%
  | "error" // refresh failed
  | "stale" // source data past its freshness threshold
  | "exhaustion" // eligible prediction expects the limit before reset
  | "warn" // primary window at/over 80%
  | "unknown"; // data source unknown

export type AttentionItem = {
  providerId: string;
  /** Short rail name of the provider. */
  providerName: string;
  kind: AttentionKind;
  /** Concise claim, e.g. "96% Weekly credits" or "Source data stale". */
  line: string;
};

const KIND_ORDER: readonly AttentionKind[] = [
  "critical",
  "error",
  "stale",
  "exhaustion",
  "warn",
  "unknown",
];

/** Thresholds for usage-derived attention items (percent of primary window).
 * Sourced from the canonical threshold levels; the rail's own semantics
 * (which item fires when) are unchanged. */
export const ATTENTION_CRITICAL_PERCENT = CRITICAL_PERCENT;
export const ATTENTION_WARN_PERCENT = NEAR_LIMIT_PERCENT;

type AttentionSource = {
  usage: ProviderUsage;
  shortName: string;
  primary: LimitWindow | undefined;
  /** Whole percent of the primary window, rounded for display. */
  percent: number | undefined;
};

/**
 * The attention rail's items: at most one per provider — its most severe
 * condition — derived only from existing runtime state. Simulated providers
 * never appear (a production summary, like the quota strip). Usage thresholds
 * read the provider's primary window; the exhaustion item appears only when
 * the prediction engine's own visibility gate already accepts the prediction
 * (`visiblePrediction`) and it concludes exhaustion lands before the reset.
 */
export function attentionItems(
  usages: readonly ProviderUsage[],
  predictionFor: (
    providerId: string,
    windowLabel: string,
  ) => QuotaPrediction | undefined,
  perspective: QuotaPerspective = "used",
): AttentionItem[] {
  const items: AttentionItem[] = [];
  const seen = new Set<string>();
  for (const entry of providerRailEntries(usages)) {
    const usage = entry.usage;
    if (usage.simulated) continue;
    if (seen.has(usage.id)) continue;
    seen.add(usage.id);

    const source: AttentionSource = {
      usage,
      shortName: entry.shortName,
      primary: entry.primary,
      percent: entry.percent,
    };
    const item = attentionItemFor(source, predictionFor, perspective);
    if (item) items.push(item);
  }
  // Stable severity order; registry order is preserved within a kind.
  return items.sort(
    (a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind),
  );
}

function attentionItemFor(
  source: AttentionSource,
  predictionFor: (
    providerId: string,
    windowLabel: string,
  ) => QuotaPrediction | undefined,
  perspective: QuotaPerspective,
): AttentionItem | null {
  const { usage, shortName, primary, percent } = source;

  // A ≥95% primary window is the loudest displayed fact: the number stays on
  // screen through stale and retained-error states, so it outranks the
  // freshness caveats those states add.
  if (primary !== undefined && percent !== undefined) {
    const windowLabel = primary.label.trim();
    const presentation = quotaPresentation(primary.usedPercent, perspective);
    if (percent >= ATTENTION_CRITICAL_PERCENT) {
      return {
        providerId: usage.id,
        providerName: shortName,
        kind: "critical",
        line: `Critical · ${presentation.label} · ${windowLabel}`,
      };
    }
  }
  // Failure states read the runtime's normalized health directly; each kind
  // names its own honest line under the shared "error" severity.
  if (usage.health === "error" || usage.health === "cooldown" || usage.health === "unavailable") {
    return {
      providerId: usage.id,
      providerName: shortName,
      kind: "error",
      line:
        usage.health === "cooldown"
          ? "Cooldown"
          : usage.health === "unavailable"
            ? "Unavailable"
            : "Refresh failed",
    };
  }
  if (usage.dataFreshness === "stale" || usage.health === "stale") {
    return { providerId: usage.id, providerName: shortName, kind: "stale", line: "Source data stale" };
  }
  if (primary !== undefined && percent !== undefined) {
    const windowLabel = primary.label.trim();
    const presentation = quotaPresentation(primary.usedPercent, perspective);
    // The gate wants the raw window percent: the display figure is rounded,
    // and a 99.6% window must still read as <100 while it truly is.
    const prediction = visiblePrediction(
      usage,
      primary.usedPercent,
      predictionFor(usage.id, windowLabel),
    );
    if (prediction?.willExhaustBeforeReset === true) {
      return {
        providerId: usage.id,
        providerName: shortName,
        kind: "exhaustion",
        line: "May reach limit before reset",
      };
    }
    if (percent >= ATTENTION_WARN_PERCENT) {
      return {
        providerId: usage.id,
        providerName: shortName,
        kind: "warn",
        line: `Warning · ${presentation.label} · ${windowLabel}`,
      };
    }
  }
  if (usage.health === "unknown") {
    return { providerId: usage.id, providerName: shortName, kind: "unknown", line: "Data source unknown" };
  }
  return null;
}

/** The valid quota windows of a usage, in the provider's own order. */
export function usableQuotaWindows(
  usage: ProviderUsage,
): LimitWindow[] {
  return usage.limits.filter(isUsableQuotaWindow);
}
