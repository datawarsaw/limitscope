// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { QuotaWindowList } from "./QuotaWindowList";
import { formatPlanValue } from "../../lib/zcodePlans";
import { formatResetTime } from "../../lib/format";
import type { ProviderUsage, ZCodePlansObservation } from "../../types";

const NOW = new Date("2026-09-30T12:00:00.000Z");

function zaiUsage(zcodePlans?: ZCodePlansObservation): ProviderUsage {
  return {
    id: "zai",
    name: "Z.ai",
    status: "ok",
    health: "live",
    checkedAt: "2026-09-30T11:59:00.000Z",
    limits: [
      { label: "5-hour", usedPercent: 12 },
      { label: "Weekly", usedPercent: 64, resetAt: "2026-10-04T09:12:03Z" },
    ],
    ...(zcodePlans ? { zcodePlans } : {}),
  };
}

function trustBuildObservation(): ZCodePlansObservation {
  return {
    plans: [
      {
        planId: "plan-trust-build",
        userPlanId: "up-1",
        name: "ZCode Trust Build",
        status: "active",
        endsAt: "2026-10-26T07:33:20Z",
        balances: [
          {
            userPlanId: "up-1",
            entitlementId: "ent-token",
            bucketId: "bucket-1",
            model: "GLM-5.3-Flash",
            unit: "token",
            limit: 100_000_000,
            used: 5_200_000,
            remaining: 94_800_000,
            period: "one_time",
            expiresAt: "2026-10-26T07:33:20Z",
          },
        ],
      },
    ],
  };
}

function renderList(usage: ProviderUsage) {
  return render(
    <QuotaWindowList
      usage={usage}
      now={NOW}
      predictionFor={() => undefined}
      perspective="used"
    />,
  );
}

afterEach(cleanup);

describe("Z.ai card without supplemental plans", () => {
  it("renders the regular windows unchanged and no plan groups", () => {
    renderList(zaiUsage());
    expect(screen.getByText("5-hour")).toBeTruthy();
    expect(screen.getByText("Weekly")).toBeTruthy();
    expect(screen.queryByRole("group", { name: /^ZCode plan/ })).toBeNull();
    expect(document.querySelector(".zcode-plans")).toBeNull();
  });
});

describe("one supplemental plan", () => {
  it("renders the plan, its absolute balance, and the expiry", () => {
    renderList(zaiUsage(trustBuildObservation()));

    const group = screen.getByRole("group", { name: "ZCode plan ZCode Trust Build" });
    // The regular windows remain recognizable above the plan group.
    expect(screen.getByText("5-hour")).toBeTruthy();
    // Absolute values, never a percentage.
    expect(group.textContent).toContain("GLM-5.3-Flash");
    expect(group.textContent).toContain("5.2M / 100M token");
    expect(group.textContent).not.toMatch(/%\s*token/);
    // One-time packages expire; the date matches the shared readout.
    expect(group.textContent).toContain(
      `Expires ${formatResetTime("2026-10-26T07:33:20Z", NOW)}`,
    );
    // Stable identifiers ride the DTO but stay out of the rendered text.
    expect(group.textContent).not.toContain("plan-trust-build");
    expect(group.textContent).not.toContain("bucket-1");
  });
});

describe("two simultaneous plans", () => {
  it("renders both plans independently with separate pools", () => {
    const observation: ZCodePlansObservation = {
      plans: [
        {
          planId: "plan-trust-build",
          name: "ZCode Trust Build",
          status: "active",
          balances: [
            {
              model: "GLM-5.3-Flash",
              unit: "token",
              limit: 100_000_000,
              used: 5_200_000,
              remaining: 94_800_000,
              period: "one_time",
            },
          ],
        },
        {
          planId: "plan-start",
          name: "Start Plan",
          status: "active",
          balances: [
            {
              model: "GLM-5.3",
              unit: "credit",
              limit: 6,
              used: 1,
              remaining: 5,
              period: "daily",
              periodEnd: "2026-10-01T00:00:00Z",
            },
          ],
        },
      ],
    };
    renderList(zaiUsage(observation));

    const trust = screen.getByRole("group", { name: "ZCode plan ZCode Trust Build" });
    const start = screen.getByRole("group", { name: "ZCode plan Start Plan" });
    expect(trust.textContent).toContain("5.2M / 100M token");
    expect(start.textContent).toContain("1 / 6 credit");
    // A repeating period resets rather than expires.
    expect(start.textContent).toContain("Resets");
    // No cross-plan or cross-unit sum appears anywhere.
    const all = document.body.textContent ?? "";
    expect(all).not.toContain("101");
    expect(all).not.toContain("100000006");
    expect(all).not.toContain("94.8M + 5");
  });
});

describe("mixed units stay separate rows", () => {
  it("never merges token and credit buckets of one plan", () => {
    const observation: ZCodePlansObservation = {
      plans: [
        {
          planId: "plan-mixed",
          name: "Mixed Package",
          status: "active",
          balances: [
            {
              model: "GLM-5.3-Flash",
              unit: "token",
              limit: 100_000_000,
              used: 5_200_000,
              remaining: 94_800_000,
            },
            {
              meter: "credits",
              unit: "credit",
              limit: 6,
              used: 1,
              remaining: 5,
            },
          ],
        },
      ],
    };
    renderList(zaiUsage(observation));
    const group = screen.getByRole("group", { name: "ZCode plan Mixed Package" });
    expect(group.textContent).toContain("5.2M / 100M token");
    expect(group.textContent).toContain("1 / 6 credit");
  });

  it("renders the remainder when no limit was reported", () => {
    const observation: ZCodePlansObservation = {
      plans: [
        {
          planId: "plan-remaining-only",
          name: "Partial Report",
          status: "active",
          balances: [{ unit: "token", remaining: 94_800_000 }],
        },
      ],
    };
    renderList(zaiUsage(observation));
    const group = screen.getByRole("group", { name: "ZCode plan Partial Report" });
    expect(group.textContent).toContain("94.8M left token");
  });
});

describe("absolute value formatting", () => {
  it("formats compact absolute counts without inventing precision", () => {
    expect(formatPlanValue(94_800_000)).toBe("94.8M");
    expect(formatPlanValue(100_000_000)).toBe("100M");
    expect(formatPlanValue(5_200_000)).toBe("5.2M");
    expect(formatPlanValue(948)).toBe("948");
    expect(formatPlanValue(0)).toBe("0");
    expect(formatPlanValue(undefined)).toBe("—");
    expect(formatPlanValue(Number.NaN)).toBe("—");
  });
});
