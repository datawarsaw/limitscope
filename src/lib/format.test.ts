import { describe, expect, it } from "vitest";
import {
  formatAge,
  formatCountdown,
  formatResetLine,
  formatResetTime,
  formatTime,
} from "./format";

const NOW = new Date("2026-09-27T12:00:00Z");

describe("formatAge", () => {
  it("renders days for a snapshot from over a week ago", () => {
    expect(formatAge("2026-09-18T02:13:32.735Z", NOW)).toBe("9d ago");
  });

  it("renders hours and minutes below a day", () => {
    expect(formatAge("2026-09-27T09:00:00Z", NOW)).toBe("3h ago");
    expect(formatAge("2026-09-27T11:48:00Z", NOW)).toBe("12m ago");
  });

  it("reads under a minute as just now", () => {
    expect(formatAge("2026-09-27T11:59:30Z", NOW)).toBe("just now");
  });

  it("treats a future-dated stamp (clock skew) as just now", () => {
    expect(formatAge("2026-09-27T12:00:10Z", NOW)).toBe("just now");
  });

  it("safely handles an invalid date string", () => {
    expect(formatAge("invalid-date", NOW)).toBe("unknown");
  });
});

describe("formatCountdown", () => {
  it("renders days and hours a few days out", () => {
    // 5d 13h 15m ahead — hours truncate, never round up.
    expect(formatCountdown("2026-10-03T01:15:00Z", NOW)).toBe("5d 13h");
  });

  it("renders hours and minutes below a day", () => {
    expect(formatCountdown("2026-09-27T14:17:00Z", NOW)).toBe("2h 17m");
  });

  it("renders bare minutes below an hour", () => {
    expect(formatCountdown("2026-09-27T12:18:00Z", NOW)).toBe("18m");
  });

  it("reads the last minute as <1m", () => {
    expect(formatCountdown("2026-09-27T12:00:30Z", NOW)).toBe("<1m");
  });

  it("keeps zero-padding on exact unit boundaries", () => {
    expect(formatCountdown("2026-09-28T12:00:00Z", NOW)).toBe("1d 0h");
    expect(formatCountdown("2026-09-27T14:00:00Z", NOW)).toBe("2h 0m");
  });

  it("reads a past timestamp as expired (null)", () => {
    expect(formatCountdown("2026-09-27T11:59:59Z", NOW)).toBeNull();
    expect(formatCountdown("2026-09-20T12:00:00Z", NOW)).toBeNull();
  });

  it("reads the exact reset instant as expired", () => {
    expect(formatCountdown("2026-09-27T12:00:00Z", NOW)).toBeNull();
  });

  it("returns null for an invalid timestamp without throwing", () => {
    expect(formatCountdown("not-a-date", NOW)).toBeNull();
  });
});

describe("formatResetLine", () => {
  it("formats >1 day reset with days and hours", () => {
    expect(formatResetLine("2026-10-03T01:15:00Z", NOW)).toMatch(
      /^Resets .+ · in 5d 13h$/,
    );
  });

  it("formats hours reset with hours and minutes", () => {
    expect(formatResetLine("2026-09-27T14:17:00Z", NOW)).toMatch(
      /^Resets .+ · in 2h 17m$/,
    );
  });

  it("formats minutes reset with minutes only", () => {
    expect(formatResetLine("2026-09-27T12:18:00Z", NOW)).toMatch(
      /^Resets .+ · in 18m$/,
    );
  });

  it("formats <1 minute reset as <1m", () => {
    expect(formatResetLine("2026-09-27T12:00:30Z", NOW)).toMatch(
      /^Resets .+ · in <1m$/,
    );
  });

  it("reads an expired reset as passed, without a negative duration", () => {
    expect(formatResetLine("2026-09-27T11:00:00Z", NOW)).toBe(
      "Reset time passed",
    );
  });

  it("reads the exact reset instant as passed", () => {
    expect(formatResetLine("2026-09-27T12:00:00Z", NOW)).toBe(
      "Reset time passed",
    );
  });

  it("safely returns empty string for an invalid resetAt without throwing", () => {
    expect(formatResetLine("not-a-date", NOW)).toBe("");
  });
});

describe("formatResetTime and formatTime safety", () => {
  it("returns dash for invalid date strings", () => {
    expect(formatResetTime("invalid")).toBe("—");
    expect(formatTime("invalid")).toBe("—");
  });

  it("formats reset time relative to provided now", () => {
    const sameDay = "2026-09-27T18:30:00Z";
    const res = formatResetTime(sameDay, NOW);
    expect(res).toBeTruthy();
    expect(res).not.toBe("—");
  });
});
