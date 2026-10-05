import type { QuotaObservation } from "./quotaHistory";
import type { QuotaPrediction } from "./prediction/types";
import { formatAge, formatTime } from "./format";
import type { ProviderUsage } from "../types";

export type ProviderStatusPresentation = {
  label: "Live" | "Cached" | "Stale" | "Refresh failed" | "Cooldown" | "Unavailable" | "Mock" | "Unknown";
  className: "ok" | "stale" | "error" | "unknown" | "mock";
};

function validTime(value: string | undefined): string | undefined {
  if (!value || Number.isNaN(Date.parse(value))) return undefined;
  return value;
}

/**
 * Narrows persisted history to the accounts that are current right now, so
 * predictions can never blend two accounts of one provider (MIC-297 follow-up:
 * replacing a stored credential must not let the new account inherit the old
 * account's burn rates, cycle segmentation, or projections).
 *
 * A provider whose current usage proves an identity admits only observations
 * carrying exactly that identity — its own history, and never the legacy
 * unattributed entries written before attribution existed. Every other
 * provider (no account concept, or attribution unavailable this cycle)
 * admits only unattributed observations, which is exactly the pre-existing
 * single-account behavior.
 */
export function historyForCurrentAccounts(
  history: readonly QuotaObservation[],
  usages: readonly ProviderUsage[],
): QuotaObservation[] {
  const currentIdentity = new Map<string, string>();
  for (const usage of usages) {
    const identity = usage.account?.identity;
    if (typeof identity === "string" && identity.trim() !== "") {
      currentIdentity.set(usage.id, identity);
    }
  }
  return history.filter((observation) => {
    const identity = currentIdentity.get(observation.providerId);
    return observation.account === identity;
  });
}

export function predictionKey(providerId: string, windowLabel: string): string {
  return `${providerId}|${windowLabel}`;
}

export function predictionsByIdentity(
  predictions: readonly QuotaPrediction[],
): ReadonlyMap<string, QuotaPrediction> {
  return new Map(
    predictions.map((prediction) => [
      predictionKey(prediction.providerId, prediction.windowLabel),
      prediction,
    ]),
  );
}

/** Applies the product visibility gate independently of engine output. */
export function visiblePrediction(
  usage: ProviderUsage,
  limitUsedPercent: number,
  prediction: QuotaPrediction | undefined,
): QuotaPrediction | undefined {
  if (!prediction || usage.simulated) return undefined;
  if (usage.health === "stale" || usage.health === "unknown") return undefined;
  if (usage.dataFreshness === "stale") return undefined;
  // A freshness verdict we cannot date is indeterminate, and indeterminate
  // data supports no projection: age we cannot read must not look fresh here
  // any more than it may look fresh on the card.
  if (usage.dataFreshness && !validTime(usage.sourceUpdatedAt)) return undefined;
  if (prediction.basis.isStale || prediction.basis.resetExpired) return undefined;
  if (prediction.confidence !== "medium" && prediction.confidence !== "high") {
    return undefined;
  }
  if (!Number.isFinite(limitUsedPercent) || limitUsedPercent >= 100) return undefined;
  return prediction.projectedPercentAtReset === undefined ? undefined : prediction;
}

/**
 * Names the runtime's normalized health for display. `health` is the single
 * verdict the Rust runtime computed; this function only picks the label and
 * visual class — it never reconstructs health from other fields. The
 * freshness branch (`Cached`) stays because a live fetch from a cached
 * source snapshot is exactly a live health with a fresh verdict.
 */
export function providerStatusPresentation(
  usage: ProviderUsage,
): ProviderStatusPresentation {
  if (usage.simulated) return { label: "Mock", className: "mock" };
  if (usage.health === "error") return { label: "Refresh failed", className: "error" };
  if (usage.health === "cooldown") return { label: "Cooldown", className: "error" };
  if (usage.health === "unavailable") return { label: "Unavailable", className: "error" };
  if (usage.dataFreshness === "stale") return { label: "Stale", className: "stale" };
  if (usage.dataFreshness || validTime(usage.sourceUpdatedAt)) {
    return usage.dataFreshness === "fresh" && validTime(usage.sourceUpdatedAt)
      ? { label: "Cached", className: "ok" }
      : { label: "Stale", className: "stale" };
  }
  if (usage.health === "stale") return { label: "Stale", className: "stale" };
  if (usage.health === "unknown") return { label: "Unknown", className: "unknown" };
  return { label: "Live", className: "ok" };
}

export function providerSourceNote(
  usage: ProviderUsage,
  now: Date,
  options?: { includeAccount?: boolean },
): string {
  const status = providerStatusPresentation(usage);
  // Masked account attribution (e.g. Grok's xAI account): lets the user tell
  // which account a card describes; the label is masked by the provider layer
  // and is never token material (see AccountAttribution). Surfaces that show
  // the account on a dedicated line opt out here with includeAccount: false.
  const accountNote =
    options?.includeAccount === false ||
    !usage.account?.label ||
    status.label === "Mock"
      ? ""
      : ` · account ${usage.account.label}`;
  const note = sourceNoteText(usage, status, now);
  return note ? `${note}${accountNote}` : accountNote.trimStart();
}

function sourceNoteText(
  usage: ProviderUsage,
  status: ProviderStatusPresentation,
  now: Date,
): string {
  if (status.label === "Mock") return "Deterministic sample data";
  if (status.label === "Unavailable") return "Provider data unavailable";
  if (status.label === "Cooldown") {
    if (usage.limits.length === 0) return "Waiting for the server's retry window";
    const retainedSource = validTime(usage.sourceUpdatedAt) ?? usage.checkedAt;
    return `Cooling down · showing data from ${formatTime(retainedSource)} (${formatAge(retainedSource, now)})`;
  }
  if (status.label === "Refresh failed") {
    if (usage.limits.length === 0) return "No saved provider data";
    // The retained snapshot's own age is the truth: checkedAt is only the
    // last successful read (of a live source or of a cache) and can make an
    // old underlying snapshot look recent.
    const sourceUpdatedAt = validTime(usage.sourceUpdatedAt);
    if (sourceUpdatedAt) {
      return usage.dataFreshness === "stale"
        ? `Refresh failed · stale data from ${formatTime(sourceUpdatedAt)} (${formatAge(sourceUpdatedAt, now)})`
        : `Showing last good data from ${formatTime(sourceUpdatedAt)} (${formatAge(sourceUpdatedAt, now)})`;
    }
    return `Showing last good data from ${formatTime(usage.checkedAt)} (${formatAge(usage.checkedAt, now)})`;
  }
  if (status.label === "Stale") {
    const sourceUpdatedAt = validTime(usage.sourceUpdatedAt);
    return sourceUpdatedAt
      ? `Showing data from ${formatTime(sourceUpdatedAt)}`
      : "Source update time unknown";
  }
  if (status.label === "Unknown") return "Data source unknown";
  if (status.label === "Cached" && usage.sourceUpdatedAt) {
    return `Source updated ${formatAge(usage.sourceUpdatedAt, now)}`;
  }
  return `Checked ${formatTime(usage.checkedAt)}`;
}

/** A cycle is globally successful when at least one real source returned data. */
export function refreshCycleSucceeded(usages: readonly ProviderUsage[]): boolean {
  return usages.some(
    (usage) =>
      !usage.simulated && (usage.health === "live" || usage.health === "stale"),
  );
}

export function predictionBasisLabel(prediction: QuotaPrediction): string {
  const minutes = Math.max(0, Math.round(prediction.basis.fitSpanMinutes));
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  const span = hours > 0
    ? `${hours}h${remainingMinutes > 0 ? ` ${remainingMinutes}m` : ""}`
    : `${Math.max(1, minutes)}m`;
  return `Based on ${prediction.basis.fitSampleCount} samples over ${span}; current reset cycle only`;
}
