import type { CodexResetCredits, ProviderUsage } from "../types";

/**
 * Display contract for the Codex banked reset-credit capability (v0.7).
 *
 * The backend reports two distinct facts and this module keeps them apart:
 * a banked balance ("3 banked reset credits") and a separate applicability
 * count ("1 credit currently applicable"). A banked balance is never
 * "resets left" or "resets available" — nothing here claims a credit can be
 * used right now. Missing applicability reads as
 * "No credit applicability information", never as zero.
 *
 * Codex-only by construction: only the openai-codex provider entry is ever
 * rendered, so no other provider can grow a fake reset count. Nothing is
 * derived from quota windows, reset timestamps, history, or usage drops —
 * only the explicit resetCredits DTO counts.
 */

/** Freshness budget mirroring RESET_CREDITS_TTL_SECS in src-tauri/src/codex.rs. */
export const RESET_CREDITS_TTL_MS = 15 * 60 * 1000;

/** The one provider allowed to carry a banked-credit observation. */
export const CODEX_PROVIDER_ID = "openai-codex";

/** Clock-skew tolerance mirroring the Rust freshness check (5 minutes). */
const FUTURE_TOLERANCE_MS = 5 * 60 * 1000;

function isValidCount(value: unknown): value is number {
  return (
    typeof value === "number" &&
    Number.isSafeInteger(value) &&
    (value as number) >= 0
  );
}

/**
 * True when a credit observation may still read as current: parseable,
 * not future-dated beyond clock-skew tolerance, and within the TTL.
 * Anything else must render as unavailable, never as a current balance.
 */
export function isResetCreditsFresh(
  checkedAt: string,
  nowMs: number = Date.now(),
): boolean {
  const observed = Date.parse(checkedAt);
  if (Number.isNaN(observed)) return false;
  const age = nowMs - observed;
  return age >= -FUTURE_TOLERANCE_MS && age <= RESET_CREDITS_TTL_MS;
}

export type ResetCreditLines = {
  bankLine: string;
  applicabilityLine: string;
};

/**
 * Safe UI lines for a provider entry, or null when the capability is
 * unavailable (absent, stale, invalid, or a non-Codex provider).
 */
export function formatResetCredits(
  usage: ProviderUsage,
  nowMs: number = Date.now(),
): ResetCreditLines | null {
  if (usage.id !== CODEX_PROVIDER_ID) return null;
  const credits: CodexResetCredits | undefined = usage.resetCredits;
  if (!credits) return null;
  if (!isValidCount(credits.bankedCredits)) return null;
  if (!isResetCreditsFresh(credits.checkedAt, nowMs)) return null;
  const banked = credits.bankedCredits;
  const plural = banked === 1 ? "" : "s";
  const bankLine = banked + " banked reset credit" + plural;
  const applicability = credits.currentlyApplicable;
  let applicabilityLine = "No credit applicability information";
  if (applicability !== undefined && isValidCount(applicability)) {
    applicabilityLine =
      applicability + " credit" + (applicability === 1 ? "" : "s") + " currently applicable";
  }
  return { bankLine, applicabilityLine };
}
