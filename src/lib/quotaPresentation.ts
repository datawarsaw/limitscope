import { CRITICAL_PERCENT, NEAR_LIMIT_PERCENT } from "./thresholds";

/**
 * Presentation-only quota perspective.
 *
 * Runtime providers, history, predictions, and severity policy continue to
 * use canonical `usedPercent`. This module derives what a surface should say
 * and how its meter should fill without changing those canonical values.
 */
export const QUOTA_PERSPECTIVES = ["used", "remaining"] as const;
export type QuotaPerspective = (typeof QUOTA_PERSPECTIVES)[number];

export type QuotaPresentation = {
  /** Finite canonical source value, preserved separately from display math. */
  canonicalUsedPercent: number | null;
  perspective: QuotaPerspective;
  /** Rounded percentage shown to the user, or null for malformed input. */
  displayPercent: number | null;
  displayPercentText: string;
  /** Human copy that always names the perspective, e.g. `80% remaining`. */
  label: string;
  ariaLabel: string;
  /** Fill percentage for a meter. Remaining mode fills remaining capacity. */
  meterPercent: number | null;
  /** Matches the displayed metric for `aria-valuenow`. */
  meterAriaValueNow: number | null;
  meterAriaValueText: string;
  /** Explicit canonical severity copy; never inferred from displayPercent. */
  severityLabel: "Near limit" | "Critical" | null;
  /** Canonical limit state. Never derived from the displayed perspective. */
  isLimitReached: boolean;
};

export function isQuotaPerspective(value: unknown): value is QuotaPerspective {
  return (QUOTA_PERSPECTIVES as readonly unknown[]).includes(value);
}

function finitePercent(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, value));
}

/**
 * Derive the shared display contract from one canonical used percentage.
 * Nonnumeric input stays visible as unavailable rather than producing a
 * number. Finite defensive outliers are clamped only for presentation; the
 * source value remains separate and is never fed back into product logic.
 */
export function quotaPresentation(
  usedPercent: unknown,
  perspective: QuotaPerspective,
): QuotaPresentation {
  const canonicalUsedPercent = finitePercent(usedPercent);
  if (canonicalUsedPercent === null) {
    return {
      canonicalUsedPercent: null,
      perspective,
      displayPercent: null,
      displayPercentText: "—",
      label: "Usage unavailable",
      ariaLabel: "Usage unavailable",
      meterPercent: null,
      meterAriaValueNow: null,
      meterAriaValueText: "unavailable",
      severityLabel: null,
      isLimitReached: false,
    };
  }

  const normalizedUsedPercent = clampPercent(canonicalUsedPercent);
  const derived =
    perspective === "used" ? normalizedUsedPercent : 100 - normalizedUsedPercent;
  const displayPercent = Math.round(derived);
  const perspectiveLabel = perspective === "used" ? "used" : "remaining";
  const label = `${displayPercent}% ${perspectiveLabel}`;
  return {
    canonicalUsedPercent,
    perspective,
    displayPercent,
    displayPercentText: `${displayPercent}%`,
    label,
    ariaLabel: label,
    meterPercent: derived,
    meterAriaValueNow: displayPercent,
    meterAriaValueText: label,
    severityLabel:
      normalizedUsedPercent >= CRITICAL_PERCENT
        ? "Critical"
        : normalizedUsedPercent >= NEAR_LIMIT_PERCENT
          ? "Near limit"
          : null,
    isLimitReached: normalizedUsedPercent >= 100,
  };
}

/** Compact aliases for consumers that want a surface-specific name. */
export function mainQuotaPresentation(
  usedPercent: unknown,
  perspective: QuotaPerspective,
): QuotaPresentation {
  return quotaPresentation(usedPercent, perspective);
}

export function floatingQuotaPresentation(
  usedPercent: unknown,
  perspective: QuotaPerspective,
): QuotaPresentation {
  return quotaPresentation(usedPercent, perspective);
}

export function trayQuotaPresentation(
  usedPercent: unknown,
  perspective: QuotaPerspective,
): QuotaPresentation {
  return quotaPresentation(usedPercent, perspective);
}

/**
 * Semantic quota color level from the approved Halo direction: a six-step
 * scale on REMAINING quota, shared by the floating bar, its detail card, and
 * every primary/secondary meter on them. The level is derived from the
 * clamped canonical value — never from the displayed perspective or the
 * rounded readout — so a "remaining" surface and a "used" surface agree on
 * color for the same data.
 */
export type QuotaColorLevel =
  | "healthy" // 90–100% remaining
  | "good" // 70–89%
  | "fair" // 50–69%
  | "medium" // 30–49%
  | "low" // 15–29%
  | "critical" // 0–14%
  | "unavailable";

/**
 * Single TS source for the Halo remaining-quota scale: the six band floors
 * plus their approved hex values. `floating.css` paints each level with its
 * hex and `floating.css.test.ts` guards that mirror against drift. Bands run
 * from high remaining to low and the first floor the remaining percentage
 * meets wins, so floors are inclusive (70 remaining is "good", 69.9 "fair").
 */
export const HALO_REMAINING_QUOTA_SCALE = [
  { minRemainingPercent: 90, level: "healthy", color: "#2ee6d0" },
  { minRemainingPercent: 70, level: "good", color: "#3fdc7e" },
  { minRemainingPercent: 50, level: "fair", color: "#b5e04a" },
  { minRemainingPercent: 30, level: "medium", color: "#f5b942" },
  { minRemainingPercent: 15, level: "low", color: "#fb8a3c" },
  { minRemainingPercent: 0, level: "critical", color: "#ff5d5d" },
] as const;

export function quotaColorLevel(
  presentation: QuotaPresentation,
): QuotaColorLevel {
  if (presentation.canonicalUsedPercent === null) return "unavailable";
  const remainingPercent = 100 - clampPercent(presentation.canonicalUsedPercent);
  return (
    HALO_REMAINING_QUOTA_SCALE.find(
      (band) => remainingPercent >= band.minRemainingPercent,
    )?.level ?? "critical"
  );
}

/** History/trend display value; observations themselves remain canonical. */
export function quotaHistoryPresentation(
  usedPercent: unknown,
  perspective: QuotaPerspective,
): QuotaPresentation {
  return quotaPresentation(usedPercent, perspective);
}

export function historyQuotaPresentation(
  usedPercent: unknown,
  perspective: QuotaPerspective,
): QuotaPresentation {
  return quotaHistoryPresentation(usedPercent, perspective);
}

export type QuotaPredictionPresentation = {
  displayPercent: number | null;
  label: string;
};

/** Exact display-only projection copy derived from a canonical projection. */
export function predictionQuotaPresentation(
  projectedUsedPercent: unknown,
  perspective: QuotaPerspective,
): QuotaPredictionPresentation {
  const presentation = quotaPresentation(projectedUsedPercent, perspective);
  if (presentation.displayPercent === null) {
    return { displayPercent: null, label: "Projection unavailable" };
  }
  return {
    displayPercent: presentation.displayPercent,
    label:
      presentation.perspective === "used"
        ? `Projected at reset: ${presentation.displayPercent}% used`
        : `Projected remaining at reset: ${presentation.displayPercent}%`,
  };
}
