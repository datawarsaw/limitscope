import type { ProviderUsage, ZCodePlan, ZCodePlanBalance } from "../types";

/**
 * Display contract for the ZCode plan/balance observation (Z.ai provider).
 *
 * The backend reports each active plan/package with its own balance buckets,
 * grouped exactly as upstream attributes them, and this module keeps them
 * that way: rows render per bucket, units never merge (a token pool and a
 * credit pool stay two rows), and plans never share a line. There is no
 * aggregation anywhere in this module by construction.
 *
 * The pool values are ABSOLUTE upstream counts — deliberately not the
 * coding-plan monitor endpoint's percentage-only shape, which is why they
 * render as "used / limit unit" instead of feeding the percent meters.
 *
 * Freshness is the backend's job: the runtime strips stale observations
 * before the snapshot leaves the process and never hydrates them from disk,
 * and the observation stamp is deliberately not on the wire — so anything
 * rendered here is current by construction.
 */

/** The one provider allowed to carry plan observations. */
export const ZCODE_PLANS_PROVIDER_ID = "zai";

/**
 * True when a provider entry may render plan groups: the observation must
 * exist and belong to the Z.ai entry. Plans with no balances still render —
 * an active package with no buckets is a real upstream state.
 */
export function planGroupsFor(usage: ProviderUsage): ZCodePlan[] | null {
  if (usage.id !== ZCODE_PLANS_PROVIDER_ID) return null;
  const plans = usage.zcodePlans?.plans;
  if (!Array.isArray(plans)) return null;
  return plans;
}

/**
 * Compact absolute count ("94.8M", "100M", "5", "—"). Values beyond safe
 * integer range or non-finite render as unknown rather than a wrong number.
 */
export function formatPlanValue(value: number | undefined): string {
  if (value === undefined) return "—";
  if (!Number.isFinite(value) || Math.abs(value) > Number.MAX_SAFE_INTEGER) {
    return "—";
  }
  const abs = Math.abs(value);
  if (abs >= 1_000_000_000) {
    return `${trimZeros(value / 1_000_000_000)}B`;
  }
  if (abs >= 1_000_000) {
    return `${trimZeros(value / 1_000_000)}M`;
  }
  if (abs >= 10_000) {
    return `${trimZeros(value / 1_000)}K`;
  }
  return `${value}`;
}

function trimZeros(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return `${rounded}`;
}

/**
 * The balance readout line: absolute "used / limit" when both exist, the
 * remainder when only it exists, the consumption otherwise — always with
 * the unit attached when upstream names one. Never a percentage.
 */
export function formatBalanceValue(balance: ZCodePlanBalance): string {
  const unit = balance.unit ? ` ${balance.unit}` : "";
  if (balance.used !== undefined && balance.limit !== undefined) {
    return `${formatPlanValue(balance.used)} / ${formatPlanValue(balance.limit)}${unit}`;
  }
  if (balance.remaining !== undefined) {
    return `${formatPlanValue(balance.remaining)} left${unit}`;
  }
  if (balance.used !== undefined) {
    return `${formatPlanValue(balance.used)} used${unit}`;
  }
  return unit.trim() || "—";
}

/** The bucket's display label: the model when upstream names one. */
export function balanceLabel(balance: ZCodePlanBalance): string {
  return balance.model ?? balance.meter ?? "Balance";
}
