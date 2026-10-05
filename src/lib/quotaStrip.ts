import type { LimitWindow, ProviderUsage } from "../types";
import {
  providerStatusPresentation,
  type ProviderStatusPresentation,
} from "./v03Integration";

/**
 * One row of the compact quota strip: the provider plus the single window
 * that represents it at a glance (see primaryQuotaWindow).
 */
export type QuotaStripItem = {
  providerId: string;
  name: string;
  /** Whole percent, rounded exactly like the detailed card. */
  percent: number;
  /** Window the percentage came from: spoken context (title / aria-label). */
  windowLabel: string;
  /** Status of the underlying usage. Reused from the card so the strip can
   * never read fresher — or calmer — than the provider it summarizes. */
  status: ProviderStatusPresentation;
};

/**
 * A window is usable for the strip when it can be shown as a real number:
 * a labeled window with a finite percentage inside 0–100. Anything else is a
 * malformed window and is ignored rather than displayed, so a broken payload
 * can neither invent a number nor dominate the summary. (The adapters clamp
 * what they receive; this is the strip's own guard against a bad payload.)
 */
export function isUsableQuotaWindow(limit: LimitWindow): boolean {
  return (
    limit.label.trim().length > 0 &&
    Number.isFinite(limit.usedPercent) &&
    limit.usedPercent >= 0 &&
    limit.usedPercent <= 100
  );
}

/**
 * Deterministic primary window: the usable window with the highest used
 * percentage, because the strip answers "which providers are closest to
 * their limit?". Ties keep the earliest window in the provider's own order.
 * Windows are never averaged or summed into a number the provider never
 * reported.
 */
export function primaryQuotaWindow(
  usage: ProviderUsage,
): LimitWindow | undefined {
  let primary: LimitWindow | undefined;
  for (const limit of usage.limits) {
    if (!isUsableQuotaWindow(limit)) continue;
    if (primary === undefined || limit.usedPercent > primary.usedPercent) {
      primary = limit;
    }
  }
  return primary;
}

/**
 * The strip's rows, in registry order — the same order as the cards below,
 * so a row is easy to match to its card.
 *
 * - Simulated providers never appear: the strip is a production summary.
 * - A provider with no usable window is omitted rather than shown as 0%.
 * - Usages are never combined (no max, sum, or average across accounts).
 *   The provider model exposes one usage per provider, so a repeated id is
 *   the same provider and the first usage is the one that represents it.
 */
export function quotaStripItems(
  usages: readonly ProviderUsage[],
): QuotaStripItem[] {
  const items: QuotaStripItem[] = [];
  const seen = new Set<string>();
  for (const usage of usages) {
    if (usage.simulated) continue;
    if (seen.has(usage.id)) continue;
    seen.add(usage.id);

    const primary = primaryQuotaWindow(usage);
    if (!primary) continue;

    items.push({
      providerId: usage.id,
      name: usage.name,
      percent: Math.round(primary.usedPercent),
      windowLabel: primary.label.trim(),
      status: providerStatusPresentation(usage),
    });
  }
  return items;
}
