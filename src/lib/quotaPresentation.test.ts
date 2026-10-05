import { describe, expect, it } from "vitest";
import {
  floatingQuotaPresentation,
  HALO_REMAINING_QUOTA_SCALE,
  historyQuotaPresentation,
  mainQuotaPresentation,
  predictionQuotaPresentation,
  quotaColorLevel,
  quotaPresentation,
  trayQuotaPresentation,
} from "./quotaPresentation";
import { formatResetLine } from "./format";
import {
  ATTENTION_CRITICAL_PERCENT,
  attentionItems,
  toneFor,
} from "./dashboard";
import type { ProviderUsage } from "../types";

describe("quota presentation perspectives", () => {
  it.each([
    [0, 0, 100],
    [20, 20, 80],
    [80, 80, 20],
    [95, 95, 5],
    [100, 100, 0],
  ])(
    "maps %i used to %i Used fill and %i Remaining fill",
    (used, usedFill, remainingFill) => {
      expect(quotaPresentation(used, "used")).toMatchObject({
        canonicalUsedPercent: used,
        displayPercent: usedFill,
        label: `${usedFill}% used`,
        ariaLabel: `${usedFill}% used`,
        meterPercent: usedFill,
        meterAriaValueNow: usedFill,
        meterAriaValueText: `${usedFill}% used`,
        severityLabel: used >= 95 ? "Critical" : used >= 80 ? "Near limit" : null,
      });
      expect(quotaPresentation(used, "remaining")).toMatchObject({
        canonicalUsedPercent: used,
        displayPercent: remainingFill,
        label: `${remainingFill}% remaining`,
        ariaLabel: `${remainingFill}% remaining`,
        meterPercent: remainingFill,
        meterAriaValueNow: remainingFill,
        meterAriaValueText: `${remainingFill}% remaining`,
        severityLabel: used >= 95 ? "Critical" : used >= 80 ? "Near limit" : null,
      });
    },
  );

  it.each([Number.NaN, Infinity, "20", null, undefined])(
    "treats malformed value %s as unavailable",
    (value) => {
      expect(quotaPresentation(value, "remaining")).toMatchObject({
        canonicalUsedPercent: null,
        displayPercent: null,
        label: "Usage unavailable",
        meterPercent: null,
        meterAriaValueNow: null,
        meterAriaValueText: "unavailable",
      });
    },
  );

  it.each([
    [-25, 0, 100],
    [125, 100, 0],
  ])(
    "clamps finite defensive outlier %i to safe presentation geometry",
    (used, usedFill, remainingFill) => {
      expect(quotaPresentation(used, "used")).toMatchObject({
        canonicalUsedPercent: used,
        displayPercent: usedFill,
        meterPercent: usedFill,
      });
      expect(quotaPresentation(used, "remaining")).toMatchObject({
        canonicalUsedPercent: used,
        displayPercent: remainingFill,
        meterPercent: remainingFill,
      });
    },
  );

  it.each(["used", "remaining"] as const)(
    "keeps 95%% used Critical in %s mode",
    (perspective) => {
      const used = ATTENTION_CRITICAL_PERCENT;
      expect(toneFor(used)).toBe("critical");
      const usage: ProviderUsage = {
        id: "zai",
        name: "Z.ai",
        status: "ok",
        health: "live",
        checkedAt: "2026-09-29T12:00:00Z",
        limits: [{ label: "Weekly", usedPercent: used }],
      };
      const items = attentionItems([usage], () => undefined, perspective);
      expect(items).toHaveLength(1);
      expect(items[0]).toMatchObject({
        kind: "critical",
        line:
          perspective === "used"
            ? "Critical · 95% used · Weekly"
            : "Critical · 5% remaining · Weekly",
      });
    },
  );

  it("keeps a full quota Critical with zero Remaining fill", () => {
    expect(toneFor(100)).toBe("critical");
    expect(quotaPresentation(100, "remaining")).toMatchObject({
      meterPercent: 0,
      isLimitReached: true,
    });
  });

  it("exposes the same contract for main, floating, and tray surfaces", () => {
    expect(mainQuotaPresentation(20, "remaining").label).toBe("80% remaining");
    expect(floatingQuotaPresentation(20, "remaining").meterPercent).toBe(80);
    expect(trayQuotaPresentation(20, "remaining").ariaLabel).toBe("80% remaining");
  });

  it("derives exact prediction copy without changing the projection", () => {
    expect(predictionQuotaPresentation(74, "remaining")).toEqual({
      displayPercent: 26,
      label: "Projected remaining at reset: 26%",
    });
  });

  it("inverts history display values without mutating observations", () => {
    const observations = [{ usedPercent: 20, observedAt: "2026-09-29T12:00:00Z" }];
    const before = structuredClone(observations);
    expect(historyQuotaPresentation(observations[0].usedPercent, "remaining").displayPercent).toBe(80);
    expect(observations).toEqual(before);
  });

  it("leaves reset countdown copy unchanged by presentation mode", () => {
    const now = new Date("2026-09-29T12:00:00Z");
    const resetAt = "2026-10-01T09:15:00Z";
    quotaPresentation(20, "remaining");
    expect(formatResetLine(resetAt, now)).toMatch(/^Resets .+ · in 1d 21h$/);
  });
});

describe("quota color levels", () => {
  it.each([
    [0, "healthy"], // 100% remaining
    [10, "healthy"], // 90% remaining — the teal floor is inclusive
    [11, "good"], // 89% remaining
    [30, "good"], // 70% remaining
    [31, "fair"], // 69% remaining
    [50, "fair"], // 50% remaining
    [51, "medium"], // 49% remaining
    [70, "medium"], // 30% remaining
    [71, "low"], // 29% remaining
    [85, "low"], // 15% remaining
    [86, "critical"], // 14% remaining
    [100, "critical"], // 0% remaining
  ] as const)(
    "maps %i%% used to the %s Halo level in both perspectives",
    (used, level) => {
      expect(quotaColorLevel(quotaPresentation(used, "remaining"))).toBe(level);
      expect(quotaColorLevel(quotaPresentation(used, "used"))).toBe(level);
    },
  );

  it("carries the approved Halo hex values in band order", () => {
    expect(HALO_REMAINING_QUOTA_SCALE.map((band) => band.color)).toEqual([
      "#2ee6d0",
      "#3fdc7e",
      "#b5e04a",
      "#f5b942",
      "#fb8a3c",
      "#ff5d5d",
    ]);
  });

  it("maps malformed values to unavailable", () => {
    for (const value of [Number.NaN, Infinity, null, undefined, "20"]) {
      expect(quotaColorLevel(quotaPresentation(value, "remaining"))).toBe(
        "unavailable",
      );
    }
  });

  it("derives the level from the clamped canonical value, never the display value", () => {
    // Defensive outliers clamp for display only; the level still reads them.
    expect(quotaColorLevel(quotaPresentation(125, "remaining"))).toBe("critical");
    expect(quotaColorLevel(quotaPresentation(-25, "remaining"))).toBe("healthy");
  });
});
