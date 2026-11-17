// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ProviderUsage } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { FloatingProviderPopover } from "./FloatingProviderPopover";

const now = new Date("2026-09-30T12:00:00Z");
const credits = (bankedCredits: number, currentlyApplicable?: number): NonNullable<ProviderUsage["resetCredits"]> => ({
  bankedCredits, currentlyApplicable, checkedAt: "2026-09-30T11:59:00Z", source: "codex-wham-usage",
});
function usage(id: string, alias: string, resetCredits?: ProviderUsage["resetCredits"]): ProviderUsage {
  return { id, name: "Codex", accountAlias: alias, status: "ok", health: "live", checkedAt: now.toISOString(), limits: [{ label: "Weekly", usedPercent: 20 }], resetCredits };
}
function card(item: ReturnType<typeof floatingQuotaItems>[number]) {
  return <FloatingProviderPopover item={item} pinned={false} now={now} note={null} align={0} exiting={false} onMouseEnter={() => {}} onMouseLeave={() => {}} onFocus={() => {}} onBlur={() => {}} />;
}
afterEach(cleanup);

describe("floating Codex reset bank", () => {
  it("keeps independent balances and identities when switching and swapping accounts", () => {
    const [primary, secondary] = floatingQuotaItems([
      usage("openai-codex", "Personal", credits(3, 0)),
      usage("openai-codex:secondary", "CoWork", credits(1, 1)),
    ]);
    const view = render(card(primary));
    expect(view.container.textContent).toContain("Personal");
    expect(view.container.textContent).toContain("3 banked reset credits");
    expect(view.container.textContent).toContain("0 credits currently applicable");
    expect(view.container.textContent).toContain("Expiry not reported");
    expect(view.container.textContent).not.toMatch(/resets? (available|left)/i);
    view.rerender(card(secondary));
    expect(view.container.textContent).toContain("CoWork");
    expect(view.container.textContent).toContain("1 banked reset credit");
    expect(view.container.textContent).not.toContain("3 banked reset credits");
    const [swapped] = floatingQuotaItems([usage("openai-codex:secondary", "New identity", credits(0))]);
    view.rerender(card(swapped));
    expect(view.container.textContent).toContain("New identity");
    expect(view.container.textContent).toContain("0 banked reset credits");
    expect(view.container.textContent).toContain("No credit applicability information");
    expect(view.container.textContent).not.toContain("CoWork");
    expect(view.container.textContent).not.toContain("currently applicable");
  });

  it("does not borrow the primary observation for a missing secondary", () => {
    const items = floatingQuotaItems([usage("openai-codex", "Personal", credits(3, 0)), usage("openai-codex:secondary", "CoWork")]);
    render(card(items[1]));
    expect(screen.getByText("Reset credits unavailable")).toBeTruthy();
    expect(document.body.textContent).not.toContain("banked reset credit");
  });

  it("does not show stale or malformed balances", () => {
    for (const value of [
      { ...credits(3), checkedAt: "2026-09-30T11:44:59Z" },
      { ...credits(3), bankedCredits: -1 },
      { ...credits(3), checkedAt: "invalid" },
    ]) {
      const view = render(card(floatingQuotaItems([usage("openai-codex", "Personal", value)])[0]));
      expect(view.container.textContent).toContain("Reset credits unavailable");
      expect(view.container.textContent).not.toContain("banked reset credit");
      view.unmount();
    }
  });

  it("never renders reset credits for another provider", () => {
    render(card(floatingQuotaItems([usage("zai", "Other", credits(3, 0))])[0]));
    expect(screen.queryByRole("region", { name: "Codex reset credits" })).toBeNull();
  });
});
