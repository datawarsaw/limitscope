import { formatTokens, type UsageDailyDay } from "./usageIntelligence";
import { formatDayKey } from "./usagePresentation";

/**
 * Daily activity presentation (v0.9): pure helpers for the Usage-first
 * overview's daily token-activity heatmap.
 *
 * Honesty rules, enforced here so the component stays dumb:
 * - intensity is RELATIVE — scaled to the busiest observed day in the
 *   selected range — and never reads as quota consumption or total account
 *   usage;
 * - five levels: 0 = observed with zero reported tokens (evidence supports
 *   "no reported activity"), 1–4 = quartiles of the range's own maximum;
 * - a day without recorded events has no level at all — missing is not a
 *   color, and its label never says zero usage.
 */

export type DailyIntensityLevel = 0 | 1 | 2 | 3 | 4;

export const DAILY_ACTIVITY_LEVELS: ReadonlyArray<{
  level: DailyIntensityLevel;
  label: string;
}> = [
  { level: 0, label: "no reported tokens" },
  { level: 1, label: "light" },
  { level: 2, label: "moderate" },
  { level: 3, label: "high" },
  { level: 4, label: "peak" },
];

/** The heaviest reported-token day among observed days (0 when none). */
export function dailyActivityMax(days: readonly UsageDailyDay[]): number {
  return days.reduce(
    (max, day) => Math.max(max, day.observed ? (day.totalTokens ?? 0) : 0),
    0,
  );
}

/**
 * The day's intensity level against the range's own maximum. Unobserved
 * days have no level (`null`) — the caller renders them as missing, never
 * as a color.
 */
export function dailyActivityLevel(
  day: UsageDailyDay,
  maxTokens: number,
): DailyIntensityLevel | null {
  if (!day.observed) return null;
  const tokens = day.totalTokens ?? 0;
  if (maxTokens <= 0 || tokens <= 0) return 0;
  const share = tokens / maxTokens;
  if (share <= 0.25) return 1;
  if (share <= 0.5) return 2;
  if (share <= 0.75) return 3;
  return 4;
}

/** Accessible, date-stable label for one cell; also the tooltip text. */
export function dailyCellAriaLabel(day: UsageDailyDay): string {
  const when = formatDayKey(day.date);
  if (!day.observed) {
    return day.beforeCollection
      ? `${when}: before local collection started — no data`
      : `${when}: no recorded events — not confirmed zero usage`;
  }
  const tokens = formatTokens(day.totalTokens ?? 0);
  return `${when}: ${tokens} reported tokens across ${day.events} recorded ${
    day.events === 1 ? "event" : "events"
  }`;
}
