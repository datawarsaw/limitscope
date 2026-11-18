// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ProviderUsage } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { FloatingProviderPopover } from "./FloatingProviderPopover";

const now = new Date("2026-11-18T09:00:00Z");

/**
 * The historical baseline fixture. These are FIXTURE values, not a live
 * reading: Vibe EUR 127.50 total with EUR 0.34 used, API EUR 12.75 total
 * with EUR 1.80 used, both resetting at the same December instant.
 */
const FIXTURE_RESET = "2026-12-01T00:00:00Z";

const FIXTURE: ProviderUsage = {
  id: "mistral",
  name: "Mistral",
  status: "ok",
  health: "live",
  checkedAt: "2026-11-18T08:59:00Z",
  account: {
    label: "key ••alue",
    note: "Read-only Mistral billing.",
    identity: "credential-v1:mistral-a",
  },
  planType: "EDU",
  limits: [
    { label: "Vibe Budget", usedPercent: 0.27, resetAt: FIXTURE_RESET },
    { label: "API Budget", usedPercent: 14.12, resetAt: FIXTURE_RESET },
  ],
  mistralBudgets: {
    planName: "EDU",
    rows: [
      {
        key: "vibe",
        label: "Vibe Budget",
        available: true,
        currency: "EUR",
        totalCents: 12750,
        usedCents: 34,
        remainingCents: 12716,
        usedPercent: 0.27,
        resetAt: FIXTURE_RESET,
        amountDerived: true,
      },
      {
        key: "api",
        label: "API Budget",
        available: true,
        currency: "EUR",
        totalCents: 1275,
        usedCents: 180,
        remainingCents: 1095,
        usedPercent: 14.12,
        resetAt: FIXTURE_RESET,
        amountDerived: true,
      },
    ],
  },
};

function card(usage: ProviderUsage) {
  const item = floatingQuotaItems([usage])[0];
  return (
    <FloatingProviderPopover
      item={item}
      pinned={false}
      now={now}
      note={null}
      align={0}
      exiting={false}
      onMouseEnter={() => {}}
      onMouseLeave={() => {}}
      onFocus={() => {}}
      onBlur={() => {}}
    />
  );
}

/**
 * The reset readout of one instant in the machine's own local time. The
 * fixture asserts the FORMAT contract (local day, short month, 24-hour
 * time), never an absolute timestamp: the same instant reads differently in
 * a different zone.
 */
function localResetText(iso: string): string {
  const target = new Date(iso);
  const months = [
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
  ];
  const hours = String(target.getHours()).padStart(2, "0");
  const minutes = String(target.getMinutes()).padStart(2, "0");
  return `${target.getDate()} ${months[target.getMonth()]} ${hours}:${minutes}`;
}

afterEach(cleanup);

describe("Mistral monthly budgets on the floating deck", () => {
  it("shows the baseline fixture: both budgets, their money and their reset", () => {
    render(card(FIXTURE));
    const section = screen.getByRole("region", { name: "Mistral monthly budgets" });
    const text = section.textContent ?? "";

    // The two rows, each with its own exact money.
    expect(text).toContain("Vibe Budget");
    expect(text).toContain("API Budget");
    expect(text).toContain("EUR 0.34 used");
    expect(text).toContain("EUR 127.50 total");
    expect(text).toContain("EUR 127.16 remaining");
    expect(text).toContain("EUR 1.80 used");
    expect(text).toContain("EUR 12.75 total");
    expect(text).toContain("EUR 10.95 remaining");

    // The plan is shown only because the source verified it.
    expect(text).toContain("EDU plan");

    // The reset is the source's own instant, read in local time.
    expect(text).toContain(`Resets ${localResetText(FIXTURE_RESET)}`);
    expect(text).not.toContain(FIXTURE_RESET);
    if (new Date(FIXTURE_RESET).getTimezoneOffset() === 60) {
      // Central European winter time is the reference display.
      expect(text).toContain("Resets 1 Dec 01:00");
    }
  });

  it("states the remaining percentage of each budget independently", () => {
    const view = render(card(FIXTURE));
    const rows = view.container.querySelectorAll("[data-mistral-row]");
    expect(rows).toHaveLength(2);
    // The Vibe row is the primary window; the API row keeps its own value.
    // The bar rounds the remaining percentage to whole percent.
    expect(rows[0].querySelector(".fq-mistral-state")?.textContent).toBe(
      "100% remaining",
    );
    expect(rows[1].querySelector(".fq-mistral-state")?.textContent).toBe(
      "86% remaining",
    );
  });

  it("makes the Vibe budget the card headline", () => {
    const view = render(card(FIXTURE));
    expect(
      view.container.querySelector(".fq-value-num")?.textContent,
    ).toBe("100%");
    expect(
      view.container.querySelector(".fq-primary-reset")?.textContent,
    ).toContain("Vibe Budget");
  });

  it("never repeats the same two pools in the generic window list", () => {
    const view = render(card(FIXTURE));
    expect(view.container.querySelector(".fq-windows")).toBeNull();
    expect(view.container.querySelectorAll(".fq-window-row")).toHaveLength(0);
  });

  it("keeps an unknown budget unknown and borrows nothing from its sibling", () => {
    const usage: ProviderUsage = {
      ...FIXTURE,
      limits: [FIXTURE.limits[0]],
      mistralBudgets: {
        planName: FIXTURE.mistralBudgets?.planName,
        rows: [
          FIXTURE.mistralBudgets?.rows[0] as NonNullable<
            ProviderUsage["mistralBudgets"]
          >["rows"][number],
          {
            key: "api",
            label: "API Budget",
            available: false,
            unavailableReason: "initial_budget",
          },
        ],
      },
    };
    const view = render(card(usage));
    const rows = view.container.querySelectorAll("[data-mistral-row]");
    expect(rows).toHaveLength(2);
    expect(rows[1].textContent).toContain("Unavailable");
    expect(rows[1].textContent).toContain("Budget total not reported");
    // No number is invented for the unknown row.
    expect(rows[1].textContent).not.toContain("EUR");
    expect(rows[1].textContent).not.toContain("remaining");
    // Its sibling is untouched.
    expect(rows[0].textContent).toContain("EUR 127.50 total");
  });

  it("names the missing fact of every unknown reason", () => {
    const reasons: [string, string][] = [
      ["missing", "Not reported by the source"],
      ["currency", "Currency not reported"],
      ["initial_budget", "Budget total not reported"],
      ["usage_percentage", "Usage not reported"],
      ["reset_at", "Reset time not readable"],
      ["expired", "Cycle ended"],
    ];
    for (const [reason, expected] of reasons) {
      const usage: ProviderUsage = {
        ...FIXTURE,
        mistralBudgets: {
          planName: undefined,
          rows: [
            { key: "vibe", label: "Vibe Budget", available: false, unavailableReason: reason },
          ],
        },
      };
      const view = render(card(usage));
      expect(view.container.textContent).toContain(expected);
      // An unverified plan is absent, never guessed.
      expect(view.container.textContent).not.toContain("EDU plan");
      view.unmount();
    }
  });

  it("shows no Mistral section for another provider", () => {
    render(
      card({
        id: "zai",
        name: "Z.ai",
        status: "ok",
        health: "live",
        checkedAt: now.toISOString(),
        limits: [{ label: "Weekly", usedPercent: 20 }],
      }),
    );
    expect(
      screen.queryByRole("region", { name: "Mistral monthly budgets" }),
    ).toBeNull();
  });

  it("keeps the account attribution on the card", () => {
    const view = render(card(FIXTURE));
    expect(view.container.textContent).toContain("key ••alue");
  });
});
