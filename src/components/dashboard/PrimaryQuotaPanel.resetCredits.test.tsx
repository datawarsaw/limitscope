// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { PrimaryQuotaPanel } from "./PrimaryQuotaPanel";
import type { ProviderUsage } from "../../types";

const NOW = new Date("2026-09-30T12:00:00.000Z");

function codexUsage(
  resetCredits: ProviderUsage["resetCredits"],
): ProviderUsage {
  return {
    id: "openai-codex",
    name: "OpenAI / Codex",
    status: "ok",
    health: "live",
    checkedAt: "2026-09-30T11:59:00.000Z",
    limits: [{ label: "Weekly", usedPercent: 20 }],
    resetCredits,
  };
}

function renderPanel(usage: ProviderUsage) {
  return render(
    <PrimaryQuotaPanel
      usage={usage}
      now={NOW}
      predictionFor={() => undefined}
      perspective="used"
    />,
  );
}

afterEach(cleanup);

describe("Codex reset-credit display", () => {
  it("keeps banked credits distinct from current applicability", () => {
    renderPanel(
      codexUsage({
        bankedCredits: 3,
        currentlyApplicable: 0,
        checkedAt: "2026-09-30T11:59:00.000Z",
        source: "codex-wham-usage",
      }),
    );

    const group = screen.getByRole("group", { name: "Codex reset credits" });
    expect(group.textContent).toContain("3 banked reset credits");
    expect(group.textContent).toContain("0 credits currently applicable");
    expect(group.textContent).not.toMatch(/resets? (available|left)/i);
  });

  it("reports missing applicability as unknown instead of zero", () => {
    renderPanel(
      codexUsage({
        bankedCredits: 3,
        checkedAt: "2026-09-30T11:59:00.000Z",
        source: "codex-wham-usage",
      }),
    );

    expect(screen.queryByText("No credit applicability information")).not.toBeNull();
  });
});
