import { describe, expect, it } from "vitest";
import type { LimitWindow, ProviderUsage } from "../types";
import {
  isUsableQuotaWindow,
  primaryQuotaWindow,
  quotaStripItems,
} from "./quotaStrip";

const CHECKED_AT = "2026-09-28T10:00:00.000Z";
const SOURCE_AT = "2026-09-28T09:50:00.000Z";

function limit(label: string, usedPercent: number): LimitWindow {
  return { label, usedPercent };
}

function usage(
  id: string,
  limits: LimitWindow[],
  extra: Partial<ProviderUsage> = {},
): ProviderUsage {
  return {
    id,
    name: id,
    status: "ok",
    health: "live",
    checkedAt: CHECKED_AT,
    limits,
    ...extra,
  };
}

describe("isUsableQuotaWindow", () => {
  it("accepts a labeled finite percentage inside 0-100", () => {
    expect(isUsableQuotaWindow(limit("Weekly", 0))).toBe(true);
    expect(isUsableQuotaWindow(limit("Weekly", 63.6))).toBe(true);
    expect(isUsableQuotaWindow(limit("Weekly", 100))).toBe(true);
  });

  it("rejects a missing, non-finite, or out-of-range percentage", () => {
    expect(isUsableQuotaWindow(limit("Weekly", Number.NaN))).toBe(false);
    expect(isUsableQuotaWindow(limit("Weekly", Number.POSITIVE_INFINITY))).toBe(
      false,
    );
    expect(isUsableQuotaWindow(limit("Weekly", Number.NEGATIVE_INFINITY))).toBe(
      false,
    );
    expect(isUsableQuotaWindow(limit("Weekly", -1))).toBe(false);
    expect(isUsableQuotaWindow(limit("Weekly", 100.5))).toBe(false);
  });

  it("rejects a window with no label", () => {
    expect(isUsableQuotaWindow(limit("", 40))).toBe(false);
    expect(isUsableQuotaWindow(limit("   ", 40))).toBe(false);
  });
});

describe("primaryQuotaWindow", () => {
  it("returns the only usable window", () => {
    expect(primaryQuotaWindow(usage("codex", [limit("Weekly", 7)]))).toEqual(
      limit("Weekly", 7),
    );
  });

  it("picks the highest used percentage, wherever it sits in the list", () => {
    const ascending = usage("codex", [
      limit("5-hour", 30),
      limit("Weekly", 80),
      limit("30-day", 55),
    ]);
    const descending = usage("codex", [
      limit("5-hour", 80),
      limit("Weekly", 55),
      limit("30-day", 30),
    ]);

    expect(primaryQuotaWindow(ascending)?.label).toBe("Weekly");
    expect(primaryQuotaWindow(descending)?.label).toBe("5-hour");
  });

  it("keeps the first window of the provider's order when percentages tie", () => {
    const first = usage("zai", [
      limit("Tokens", 64),
      limit("Weekly", 64),
    ]);
    const reversed = usage("zai", [
      limit("Weekly", 64),
      limit("Tokens", 64),
    ]);

    expect(primaryQuotaWindow(first)?.label).toBe("Tokens");
    expect(primaryQuotaWindow(reversed)?.label).toBe("Weekly");
  });

  it("never averages or sums windows", () => {
    const primary = primaryQuotaWindow(
      usage("zai", [limit("5-hour", 50), limit("Weekly", 90)]),
    );

    expect(primary?.usedPercent).toBe(90);
  });

  it("ignores malformed windows even when they carry the highest number", () => {
    const broken = usage("antigravity", [
      limit("Broken", Number.NaN),
      limit("Out of range", 140),
      limit("   ", 99),
      limit("Weekly", 8),
    ]);

    expect(primaryQuotaWindow(broken)?.label).toBe("Weekly");
  });

  it("returns undefined when no window is usable", () => {
    expect(primaryQuotaWindow(usage("codex", []))).toBeUndefined();
    expect(
      primaryQuotaWindow(
        usage("codex", [limit("Broken", Number.NaN), limit("", 12)]),
      ),
    ).toBeUndefined();
  });
});

describe("quotaStripItems", () => {
  it("keeps registry order and reports name, rounded percent, and window", () => {
    const items = quotaStripItems([
      usage("openai-codex", [limit("Weekly", 7)]),
      usage("zai", [limit("5-hour", 12), limit("Weekly", 63.6)]),
      usage("opencode-go", [limit("5-hour", 50)]),
    ]);

    expect(items.map((item) => item.providerId)).toEqual([
      "openai-codex",
      "zai",
      "opencode-go",
    ]);
    expect(items[1]).toMatchObject({
      percent: 64,
      windowLabel: "Weekly",
    });
    expect(items[0].status.label).toBe("Live");
  });

  it("trims the window label it reports", () => {
    const items = quotaStripItems([usage("codex", [limit("  Weekly  ", 7)])]);

    expect(items[0].windowLabel).toBe("Weekly");
  });

  it("omits simulated providers entirely", () => {
    const items = quotaStripItems([
      usage("claude", [limit("5-hour", 88)], { simulated: true }),
      usage("codex", [limit("Weekly", 7)]),
      usage("grok", [limit("30-day", 47)], { simulated: true }),
    ]);

    expect(items.map((item) => item.providerId)).toEqual(["codex"]);
  });

  it("keeps the real usage when a simulated one shares its id", () => {
    const items = quotaStripItems([
      usage("codex", [limit("Weekly", 88)], { simulated: true }),
      usage("codex", [limit("Weekly", 7)]),
    ]);

    expect(items).toHaveLength(1);
    expect(items[0].percent).toBe(7);
  });

  it("omits a provider with no usable window instead of showing 0%", () => {
    const items = quotaStripItems([
      usage("openai-codex", []),
      usage("zai", [limit("Weekly", 64)]),
      usage("antigravity", [limit("Weekly", Number.NaN)]),
    ]);

    expect(items.map((item) => item.providerId)).toEqual(["zai"]);
  });

  it("keeps a stale provider's windows with a stale status", () => {
    const items = quotaStripItems([
      usage(
        "antigravity",
        [limit("Weekly", 8)],
        {
          status: "stale",
          health: "stale",
          sourceUpdatedAt: SOURCE_AT,
          dataFreshness: "stale",
        },
      ),
    ]);

    expect(items[0]).toMatchObject({
      percent: 8,
      windowLabel: "Weekly",
    });
    expect(items[0].status).toEqual({ label: "Stale", className: "stale" });
  });

  it("keeps a cached provider distinct from a live one", () => {
    const items = quotaStripItems([
      usage(
        "antigravity",
        [limit("Weekly", 8)],
        { sourceUpdatedAt: SOURCE_AT, dataFreshness: "fresh" },
      ),
      usage("zai", [limit("Weekly", 64)]),
    ]);

    expect(items[0].status).toEqual({ label: "Cached", className: "ok" });
    expect(items[1].status).toEqual({ label: "Live", className: "ok" });
  });

  it("keeps a failed refresh's retained windows with a failure status", () => {
    const items = quotaStripItems([
      usage(
        "opencode-go",
        [limit("5-hour", 50), limit("Weekly", 71)],
        { status: "error", health: "error", error: "Refresh failed: HTTP 503" },
      ),
    ]);

    expect(items[0].percent).toBe(71);
    expect(items[0].status).toEqual({
      label: "Refresh failed",
      className: "error",
    });
  });

  it("never merges two attributed usages of one provider", () => {
    const items = quotaStripItems([
      usage("opencode-go", [limit("Weekly", 50)]),
      usage("opencode-go", [limit("Weekly", 90)]),
    ]);

    expect(items).toHaveLength(1);
    // The first usage represents the provider: never an average (70) or a
    // sum (140) of two attributed accounts.
    expect(items[0].percent).toBe(50);
  });
});
