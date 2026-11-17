import { describe, expect, it } from "vitest";
import type { ProviderUsage } from "../types";
import {
  CODEX_PROVIDER_ID,
  RESET_CREDITS_TTL_MS,
  formatResetCredits,
  formatCreditExpiry,
  isResetCreditsFresh,
} from "./resetCredits";

const NOW = Date.parse("2026-09-30T12:00:00.000Z");
const FRESH_AT = new Date(NOW - 60_000).toISOString();

function codexUsage(
  resetCredits: ProviderUsage["resetCredits"],
  id: string = CODEX_PROVIDER_ID,
): ProviderUsage {
  return {
    id,
    name: "OpenAI / Codex",
    status: "ok",
    health: "live",
    checkedAt: FRESH_AT,
    limits: [{ label: "Weekly", usedPercent: 12.5, resetAt: "2026-10-03T07:15:21Z" }],
    resetCredits,
  };
}

describe("formatResetCredits", () => {
  it("renders an explicit zero without collapsing it to unknown", () => {
    const lines = formatResetCredits(
      codexUsage({ bankedCredits: 0, checkedAt: FRESH_AT, source: "codex-wham-usage" }),
      NOW,
    );
    expect(lines?.bankLine).toBe("0 banked reset credits");
    expect(lines?.applicabilityLine).toBe("No credit applicability information");
  });

  it("uses singular wording for a single banked credit", () => {
    const lines = formatResetCredits(
      codexUsage({ bankedCredits: 1, checkedAt: FRESH_AT, source: "codex-wham-usage" }),
      NOW,
    );
    expect(lines?.bankLine).toBe("1 banked reset credit");
  });

  it("renders multiple banked credits with plural wording", () => {
    const lines = formatResetCredits(
      codexUsage({ bankedCredits: 3, checkedAt: FRESH_AT, source: "codex-wham-usage" }),
      NOW,
    );
    expect(lines?.bankLine).toBe("3 banked reset credits");
  });

  it("renders applicability 0 and 1 as explicit counts", () => {
    const zero = formatResetCredits(
      codexUsage({
        bankedCredits: 3,
        currentlyApplicable: 0,
        checkedAt: FRESH_AT,
        source: "codex-wham-usage",
      }),
      NOW,
    );
    expect(zero?.applicabilityLine).toBe("0 credits currently applicable");
    const one = formatResetCredits(
      codexUsage({
        bankedCredits: 3,
        currentlyApplicable: 1,
        checkedAt: FRESH_AT,
        source: "codex-wham-usage",
      }),
      NOW,
    );
    expect(one?.applicabilityLine).toBe("1 credit currently applicable");
  });

  it("reads missing applicability as unknown, never zero", () => {
    const lines = formatResetCredits(
      codexUsage({ bankedCredits: 3, checkedAt: FRESH_AT, source: "codex-wham-usage" }),
      NOW,
    );
    expect(lines?.applicabilityLine).toBe("No credit applicability information");
  });

  it("never derives a balance from windows, resets, or history", () => {
    // Two windows with reset timestamps and no DTO: no credit lines.
    const usage = codexUsage(undefined);
    usage.limits = [
      { label: "Weekly", usedPercent: 80, resetAt: "2026-10-03T07:15:21Z" },
      { label: "5-minute", usedPercent: 20, resetAt: "2026-09-30T12:05:00Z" },
    ];
    expect(formatResetCredits(usage, NOW)).toBeNull();
  });

  it("refuses non-Codex providers even with a forged DTO", () => {
    const usage = codexUsage(
      { bankedCredits: 3, checkedAt: FRESH_AT, source: "codex-wham-usage" },
      "zai",
    );
    expect(formatResetCredits(usage, NOW)).toBeNull();
  });

  it("rejects malformed banked counts without coercion", () => {
    for (const bankedCredits of ["3", 1.5, -1, NaN, Infinity]) {
      const usage = codexUsage(
        {
          bankedCredits: bankedCredits as number,
          checkedAt: FRESH_AT,
          source: "codex-wham-usage",
        },
      );
      expect(formatResetCredits(usage, NOW)).toBeNull();
    }
  });

  it("reads stale observations as unavailable", () => {
    const staleAt = new Date(NOW - RESET_CREDITS_TTL_MS - 1000).toISOString();
    expect(
      formatResetCredits(
        codexUsage({ bankedCredits: 3, checkedAt: staleAt, source: "codex-wham-usage" }),
        NOW,
      ),
    ).toBeNull();
    expect(isResetCreditsFresh(staleAt, NOW)).toBe(false);
    expect(isResetCreditsFresh(FRESH_AT, NOW)).toBe(true);
    expect(isResetCreditsFresh("not-a-timestamp", NOW)).toBe(false);
  });

  it("never promises usability in its wording", () => {
    const lines = formatResetCredits(
      codexUsage({
        bankedCredits: 3,
        currentlyApplicable: 0,
        checkedAt: FRESH_AT,
        source: "codex-wham-usage",
      }),
      NOW,
    );
    const text = (lines?.bankLine ?? "") + " " + (lines?.applicabilityLine ?? "");
    expect(text).not.toMatch(/resets? (left|available)/i);
    expect(text).not.toMatch(/replenishments? available/i);
  });
});

describe("formatCreditExpiry", () => {
  const fresh = { bankedCredits: 2, currentlyApplicable: 0, checkedAt: FRESH_AT,
    source: "codex-wham-usage" as const, nextCreditExpiresAt: "2026-10-04T12:30:00Z" };

  it("formats the verified future date and time in the app's system locale", () => {
    const expected = new Intl.DateTimeFormat(undefined, {
      year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit",
    }).format(new Date(fresh.nextCreditExpiresAt));
    expect(formatCreditExpiry(fresh, NOW)).toBe(`Next credit expiry ${expected}`);
    expect(fresh.bankedCredits).toBe(2);
    expect(fresh.currentlyApplicable).toBe(0);
  });

  it("withholds missing, null, malformed, elapsed or zero-bank deadlines", () => {
    expect(formatCreditExpiry(undefined, NOW)).toBe("Expiry not reported");
    for (const nextCreditExpiresAt of [undefined, null, "bad", "2026-10-04", "2026-10-04T12:00:00",
      "2026-02-30T00:00:00Z", "2026-09-30T12:00:00Z", "2026-09-30T11:59:59Z", 123]) {
      expect(formatCreditExpiry({ ...fresh, nextCreditExpiresAt: nextCreditExpiresAt as string }, NOW)).toBe("Expiry not reported");
    }
    expect(formatCreditExpiry({ ...fresh, bankedCredits: 0 }, NOW)).toBe("Expiry not reported");
    expect(formatCreditExpiry({ ...fresh, bankedCredits: -1 }, NOW)).toBe("Expiry not reported");
  });

  it("shares the existing 15-minute TTL without restamping an old expiry", () => {
    const observed = Date.parse(FRESH_AT);
    expect(formatCreditExpiry(fresh, observed + RESET_CREDITS_TTL_MS)).toMatch(/^Next credit expiry /);
    expect(formatCreditExpiry(fresh, observed + RESET_CREDITS_TTL_MS + 1)).toBe("Expiry not reported");
    expect(formatCreditExpiry({ ...fresh, checkedAt: "bad" }, NOW)).toBe("Expiry not reported");
    expect(formatCreditExpiry({ ...fresh, checkedAt: new Date(NOW + 300_001).toISOString() }, NOW)).toBe("Expiry not reported");
    expect(formatCreditExpiry({ ...fresh, nextCreditExpiresAt: undefined, checkedAt: new Date(NOW).toISOString() }, NOW)).toBe("Expiry not reported");
  });
});
