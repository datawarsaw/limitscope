// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { QuotaWindowList } from "./QuotaWindowList";
import { GrokBotPanel } from "./GrokBotPanel";
import {
  formatGrokBotPercent,
  grokBotFor,
  grokBotLabel,
} from "../../lib/grokBot";
import { formatResetTime } from "../../lib/format";
import type { GrokBotUsage, ProviderUsage } from "../../types";

const NOW = new Date("2026-10-06T12:00:00.000Z");

function grokUsage(grokBot?: GrokBotUsage): ProviderUsage {
  return {
    id: "grok",
    name: "Grok (xAI)",
    status: "ok",
    health: "live",
    checkedAt: "2026-10-06T11:59:00.000Z",
    limits: [
      { label: "Weekly credits", usedPercent: 54, resetAt: "2026-10-10T13:12:49Z" },
      { label: "On-demand", usedPercent: 12 },
    ],
    ...(grokBot ? { grokBot } : {}),
  };
}

function observation(overrides?: Partial<GrokBotUsage>): GrokBotUsage {
  return {
    planName: "X Premium+",
    planId: "x-premium-plus",
    cursorPlanName: "Free",
    usedPercent: 17.66,
    periodStart: "2026-10-04T09:12:03.000Z",
    resetAt: "2026-10-11T09:12:03.000Z",
    hasAvailableUsage: true,
    onDemandEnabled: false,
    ...overrides,
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

describe("Grok card without a Grok Bot observation", () => {
  it("renders the existing xAI windows unchanged and no Grok Bot block", () => {
    renderList(grokUsage());
    expect(screen.getByText("Weekly credits")).toBeTruthy();
    expect(screen.getByText("On-demand")).toBeTruthy();
    expect(screen.queryByRole("group", { name: /^Grok Bot/ })).toBeNull();
    expect(document.querySelector(".grok-bot")).toBeNull();
  });
});

describe("the Grok Bot · X Premium+ sub-block", () => {
  it("renders the plan label, the server-reported percentage, and the reset", () => {
    renderList(grokUsage(observation()));
    const block = screen.getByRole("group", { name: "Grok Bot · X Premium+" });
    expect(block).toBeTruthy();
    // The percentage comes directly from the server response, one decimal.
    expect(block.textContent).toContain("17.7% used");
    // The reset comes directly from the server response, via the shared readout.
    expect(block.textContent).toContain(
      `Resets ${formatResetTime("2026-10-11T09:12:03.000Z", NOW)}`,
    );
  });

  it("keeps the existing xAI windows rendered above it", () => {
    renderList(grokUsage(observation()));
    expect(screen.getByText("Weekly credits")).toBeTruthy();
    expect(screen.getByText("On-demand")).toBeTruthy();
  });
});

describe("no collision with the existing xAI windows", () => {
  it("never sums the Grok Bot pool with Weekly credits", () => {
    renderList(grokUsage(observation()));
    const all = document.body.textContent ?? "";
    // 54 + 17.66 must never appear as a merged figure; both originals do.
    expect(all).toContain("17.7% used");
    expect(all).toContain("54%");
    expect(all).not.toContain("71.7");
    expect(all).not.toContain("71.66");
  });

  it("renders exactly one Grok Bot group with one percentage", () => {
    renderList(grokUsage(observation()));
    expect(
      screen.getAllByRole("group", { name: "Grok Bot · X Premium+" }).length,
    ).toBe(1);
  });
});

describe("missing optional fields", () => {
  it("renders the bare Grok Bot label without a plan suffix", () => {
    renderList(grokUsage(observation({ planName: undefined, planId: undefined })));
    expect(screen.getByRole("group", { name: "Grok Bot" })).toBeTruthy();
    expect(screen.queryByRole("group", { name: /^Grok Bot ·/ })).toBeNull();
  });

  it("renders no reset line when the server reported none", () => {
    renderList(grokUsage(observation({ resetAt: undefined })));
    const block = screen.getByRole("group", { name: "Grok Bot · X Premium+" });
    expect(block.textContent).not.toContain("Resets");
    // The percentage still renders.
    expect(block.textContent).toContain("17.7% used");
  });

  it("creates no empty UI for absent on-demand or cursor-plan metadata", () => {
    renderList(
      grokUsage(
        observation({
          cursorPlanName: undefined,
          hasAvailableUsage: undefined,
          onDemandEnabled: undefined,
        }),
      ),
    );
    const block = screen.getByRole("group", { name: "Grok Bot · X Premium+" });
    // No "$0 / $0" style placeholders, no empty lines.
    expect(block.textContent).not.toContain("$");
    expect(block.textContent).not.toContain("—");
    // The diagnostics-grade plan id never renders.
    expect(block.textContent).not.toContain("x-premium-plus");
  });

  it("renders nothing for a malformed percentage", () => {
    const malformed = observation();
    (malformed as unknown as { usedPercent: unknown }).usedPercent = Number.NaN;
    renderList(grokUsage(malformed));
    expect(screen.queryByRole("group", { name: /^Grok Bot/ })).toBeNull();
  });

  it("renders nothing for another provider carrying the field", () => {
    const zai: ProviderUsage = {
      id: "zai",
      name: "Z.ai",
      status: "ok",
      health: "live",
      checkedAt: "2026-10-06T11:59:00.000Z",
      limits: [{ label: "5-hour", usedPercent: 12 }],
      grokBot: observation(),
    };
    render(
      <GrokBotPanel usage={zai} now={NOW} />,
    );
    expect(screen.queryByRole("group", { name: /^Grok Bot/ })).toBeNull();
  });
});

describe("narrow-width layout contract", () => {
  it("reuses the responsive window-row vocabulary instead of fixed widths", () => {
    renderList(grokUsage(observation()));
    const block = document.querySelector(".grok-bot");
    expect(block).toBeTruthy();
    // The block is built from the same flexible row vocabulary as the xAI
    // windows (accepted at narrow widths); no inline widths, no fixed
    // pixel sizing.
    expect(block?.querySelector(".limit-row")).toBeTruthy();
    expect(block?.querySelector(".limit-label")).toBeTruthy();
    expect(block?.querySelector(".limit-percent")).toBeTruthy();
    expect(block?.querySelector(".limit-reset")).toBeTruthy();
    for (const element of block?.querySelectorAll<HTMLElement>("*") ?? []) {
      expect(element.style.width).toBe("");
    }
  });
});

describe("display contract helpers", () => {
  it("formats one-decimal percentages without inventing precision", () => {
    expect(formatGrokBotPercent(17.66)).toBe("17.7% used");
    expect(formatGrokBotPercent(0)).toBe("0% used");
    expect(formatGrokBotPercent(100)).toBe("100% used");
    expect(formatGrokBotPercent(54)).toBe("54% used");
  });

  it("labels the block from the server-reported plan name", () => {
    expect(grokBotLabel(observation())).toBe("Grok Bot · X Premium+");
    expect(grokBotLabel(observation({ planName: undefined }))).toBe("Grok Bot");
    expect(grokBotLabel(observation({ planName: "   " }))).toBe("Grok Bot");
  });

  it("gates the observation to the Grok entry and a finite percentage", () => {
    expect(grokBotFor(grokUsage(observation()))).not.toBeNull();
    expect(grokBotFor({ ...grokUsage(observation()), id: "zai" })).toBeNull();
    expect(grokBotFor(grokUsage())).toBeNull();
    const nonFinite = observation();
    (nonFinite as unknown as { usedPercent: unknown }).usedPercent = "17.7";
    expect(grokBotFor(grokUsage(nonFinite))).toBeNull();
  });
});
