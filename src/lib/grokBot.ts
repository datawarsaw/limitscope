import type { GrokBotUsage, ProviderUsage } from "../types";

/**
 * Display contract for the Grok Bot (X Premium+) weekly observation (Grok
 * provider).
 *
 * The backend reports one supplemental weekly pool observed through the
 * Cursor-served Grok Bot client, and this module keeps it separate from the
 * xAI billing windows on the same card: it renders as its own sub-block,
 * never feeds the percent meters, the strip, or predictions (it is not
 * LimitWindow data), and is never summed, merged, or compared with the
 * "Weekly credits" or "On-demand" rows.
 *
 * Only the server-reported percentage, reset, and plan label are shown.
 * There is no absolute weekly limit, no remaining amount, and no currency —
 * the endpoint does not report them and nothing is invented here.
 *
 * Freshness is the backend's job: the runtime strips stale observations
 * before the snapshot leaves the process and never hydrates them from disk,
 * and the observation stamp is deliberately not on the wire — so anything
 * rendered here is current by construction.
 */

/** The one provider allowed to carry a Grok Bot observation. */
export const GROK_BOT_PROVIDER_ID = "grok";

/**
 * True when a provider entry may render the Grok Bot block: the observation
 * must exist, belong to the Grok entry, and carry a finite percentage (the
 * backend guarantees one; a malformed payload must not render).
 */
export function grokBotFor(usage: ProviderUsage): GrokBotUsage | null {
  if (usage.id !== GROK_BOT_PROVIDER_ID) return null;
  const grokBot = usage.grokBot;
  if (!grokBot) return null;
  if (
    typeof grokBot.usedPercent !== "number" ||
    !Number.isFinite(grokBot.usedPercent)
  ) {
    return null;
  }
  return grokBot;
}

/**
 * The block's display label: the server-reported plan label when the
 * endpoint named one ("Grok Bot · X Premium+"), the bare source name
 * otherwise. The plan id is diagnostics-grade and never rendered.
 */
export function grokBotLabel(grokBot: GrokBotUsage): string {
  const planName = grokBot.planName?.trim();
  return planName ? `Grok Bot · ${planName}` : "Grok Bot";
}

/**
 * The usage readout, one decimal place without inventing precision
 * ("17.7% used"); integral values stay integral ("0% used", "100% used").
 */
export function formatGrokBotPercent(usedPercent: number): string {
  const rounded = Math.round(usedPercent * 10) / 10;
  return `${rounded}% used`;
}
