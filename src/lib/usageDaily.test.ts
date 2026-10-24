import { describe, expect, it } from "vitest";
import {
  dailyActivityLevel,
  dailyActivityMax,
  dailyCellAriaLabel,
} from "./usageDaily";
import type { UsageDailyDay } from "./usageIntelligence";

/**
 * Focused pins for the Daily activity presentation helpers: five-level
 * relative normalization, the missing-is-not-a-color rule, and honest
 * accessible labels.
 */

function day(overrides: Partial<UsageDailyDay>): UsageDailyDay {
  return {
    date: "2026-10-24",
    observed: true,
    events: 3,
    totalTokens: 1_000,
    beforeCollection: false,
    ...overrides,
  };
}

/** The cell label's date wording, computed the same way (locale-stable). */
function dayKey(date: string): string {
  const [year, month, dayNumber] = date.split("-").map(Number);
  return new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  }).format(new Date(year, month - 1, dayNumber));
}

describe("dailyActivityMax", () => {
  it("returns the heaviest observed day and ignores missing days", () => {
    const days = [
      day({ date: "2026-10-22", totalTokens: 2_000_000 }),
      day({ date: "2026-10-23", observed: false, totalTokens: undefined }),
      day({ date: "2026-10-24", totalTokens: 4_500_000 }),
    ];
    expect(dailyActivityMax(days)).toBe(4_500_000);
  });

  it("is zero when no day is observed", () => {
    expect(dailyActivityMax([day({ observed: false, totalTokens: undefined })])).toBe(0);
    expect(dailyActivityMax([])).toBe(0);
  });
});

describe("dailyActivityLevel", () => {
  const max = 4_000;

  it("normalizes to quartile levels of the range's own maximum", () => {
    // Levels own (0,25%], (25%,50%], (50%,75%], (75%,100%] of the maximum.
    expect(dailyActivityLevel(day({ totalTokens: 4_000 }), max)).toBe(4);
    expect(dailyActivityLevel(day({ totalTokens: 3_000 }), max)).toBe(3);
    expect(dailyActivityLevel(day({ totalTokens: 2_999 }), max)).toBe(3);
    expect(dailyActivityLevel(day({ totalTokens: 2_000 }), max)).toBe(2);
    expect(dailyActivityLevel(day({ totalTokens: 1_999 }), max)).toBe(2);
    expect(dailyActivityLevel(day({ totalTokens: 1_000 }), max)).toBe(1);
    expect(dailyActivityLevel(day({ totalTokens: 1 }), max)).toBe(1);
  });

  it("keeps level 0 for observed days with zero reported tokens", () => {
    expect(dailyActivityLevel(day({ totalTokens: 0, events: 2 }), max)).toBe(0);
  });

  it("collapses to level 0 when every observed day reported zero", () => {
    expect(dailyActivityLevel(day({ totalTokens: 0 }), 0)).toBe(0);
  });

  it("gives missing days no level at all", () => {
    expect(
      dailyActivityLevel(day({ observed: false, totalTokens: undefined }), max),
    ).toBeNull();
  });
});

describe("dailyCellAriaLabel", () => {
  it("names reported tokens and recorded events for an observed day", () => {
    const label = dailyCellAriaLabel(
      day({ date: "2026-10-24", totalTokens: 1_200_000, events: 12 }),
    );
    expect(label.startsWith(dayKey("2026-10-24"))).toBe(true);
    expect(label).toContain("1.2M reported tokens across 12 recorded events");
  });

  it("singles one event", () => {
    expect(
      dailyCellAriaLabel(day({ totalTokens: 500, events: 1 })),
    ).toContain("across 1 recorded event");
  });

  it("keeps an observed zero a reported zero", () => {
    expect(
      dailyCellAriaLabel(day({ totalTokens: 0, events: 4 })),
    ).toContain("0 reported tokens across 4 recorded events");
  });

  it("says missing, never zero usage, for a day without events", () => {
    expect(
      dailyCellAriaLabel(day({ observed: false, totalTokens: undefined, events: 0 })),
    ).toContain("no recorded events — not confirmed zero usage");
  });

  it("states that pre-collection days can have no data", () => {
    expect(
      dailyCellAriaLabel(
        day({ observed: false, totalTokens: undefined, events: 0, beforeCollection: true }),
      ),
    ).toContain("before local collection started — no data");
  });
});
