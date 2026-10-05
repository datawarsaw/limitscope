/**
 * Single TS source for the two canonical quota threshold levels.
 *
 * Severity, notifications, analytics, and predictions all measure the same
 * pair of used-percentage levels; before v0.8 they each carried their own
 * literal. Every TS consumer imports from here so the levels can never drift
 * apart inside the frontend.
 *
 * Rust counterpart: `src-tauri/src/notifications.rs` (`NEAR_LIMIT_THRESHOLD`,
 * `CRITICAL_THRESHOLD`) — one definition per language, cross-referenced both
 * ways. Thresholds are canonical on USED percentage only; the Remaining
 * perspective is a display-only inversion (`quotaPresentation.ts`) and never
 * re-derives these levels.
 */

/** "Near limit" level, in percent of the window used. */
export const NEAR_LIMIT_PERCENT = 80;

/** "Critical" level, in percent of the window used. */
export const CRITICAL_PERCENT = 95;

/** The two canonical threshold levels, as a closed type. */
export type ThresholdPercent = 80 | 95;
