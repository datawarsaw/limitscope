// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderUsage } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { FloatingProviderPopover } from "./FloatingProviderPopover";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const now = new Date("2026-11-17T12:00:00Z");

/**
 * A healthy Claude Code reading: the 5h window changed two minutes ago,
 * inside the 15-minute freshness budget, so the runtime calls it Recent
 * (status class "ok"). The card must not wear that word: a healthy card
 * is quiet for every provider.
 */
const healthyClaude: ProviderUsage = {
  id: "claude-code",
  name: "Claude Code",
  health: "live",
  status: "ok",
  checkedAt: "2026-11-17T11:58:00.000Z",
  sourceUpdatedAt: "2026-11-17T11:58:00.000Z",
  dataFreshness: "fresh",
  claudeStatuslineTimes: {
    receiptAt: "2026-11-17T11:59:00Z",
    fiveHourChangedAt: "2026-11-17T11:58:00.000Z",
    sevenDayChangedAt: "2026-11-17T08:00:00.000Z",
  },
  limits: [
    { label: "5-hour", usedPercent: 23.5, resetAt: "2026-11-17T15:00:00.000Z" },
    { label: "7-day", usedPercent: 41.2, resetAt: "2026-11-20T00:00:00.000Z" },
  ],
};

/** The same provider, but the 5h window last changed 30 minutes ago. */
const lastKnownClaude: ProviderUsage = {
  ...healthyClaude,
  claudeStatuslineTimes: {
    receiptAt: "2026-11-17T11:31:00Z",
    fiveHourChangedAt: "2026-11-17T11:30:00.000Z",
    sevenDayChangedAt: "2026-11-17T08:00:00.000Z",
  },
};

/** No local reading at all: the provider's own source is unavailable. */
const unavailableClaude: ProviderUsage = {
  ...healthyClaude,
  health: "unavailable",
  status: "error",
  dataFreshness: "stale",
  limits: [],
  claudeStatuslineTimes: undefined,
};

/** The source could not be classified at all. */
const unknownClaude: ProviderUsage = {
  ...healthyClaude,
  health: "unknown",
  status: "unknown",
  dataFreshness: "stale",
  limits: [],
  claudeStatuslineTimes: undefined,
};

/** A healthy card of a different provider, for the parity check. */
const healthyOpenDesign: ProviderUsage = {
  id: "opendesign",
  name: "OpenDesign",
  health: "live",
  status: "ok",
  checkedAt: "2026-11-17T11:58:00.000Z",
  sourceUpdatedAt: "2026-11-17T11:58:00.000Z",
  dataFreshness: "fresh",
  limits: [
    { label: "5-hour", usedPercent: 73, resetText: "Resets in 39 min" },
    { label: "7-day", usedPercent: 9, resetText: "Resets in 6 days 14 hr" },
  ],
};

function card(item: ReturnType<typeof floatingQuotaItems>[number]) {
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

const statusChips = (view: { container: HTMLElement }) =>
  Array.from(view.container.querySelectorAll(".fq-status")).map(
    (el) => el.textContent,
  );

afterEach(cleanup);

describe("Claude Code status label on the detail card", () => {
  it("shows no Recent label on a healthy Claude card", () => {
    const [item] = floatingQuotaItems([healthyClaude], now);

    // The runtime still classifies the reading as Recent (status class
    // "ok"); the card is what stays quiet.
    expect(item.statusLabel).toBe("Recent");
    expect(item.statusClass).toBe("ok");

    const view = render(card(item));
    expect(statusChips(view)).toEqual([]);
    expect(view.container.textContent).not.toContain("Recent");
    expect(view.container.querySelector(".fq-status")).toBeNull();
  });

  it("keeps a healthy card of another provider identical", () => {
    const [other] = floatingQuotaItems([healthyOpenDesign], now);
    expect(other.statusClass).toBe("ok");

    const view = render(card(other));
    expect(statusChips(view)).toEqual([]);
    expect(view.container.textContent).not.toContain("Cached");

    // Same rule, same silence: the provider name never decides it.
    const [claude] = floatingQuotaItems([healthyClaude], now);
    const claudeView = render(card(claude));
    expect(statusChips(claudeView)).toEqual(statusChips(view));
  });

  it("still names a degraded Claude state", () => {
    const [lastKnown] = floatingQuotaItems([lastKnownClaude], now);
    expect(lastKnown.statusLabel).toBe("Last known");
    expect(lastKnown.statusClass).toBe("stale");
    const lastKnownView = render(card(lastKnown));
    expect(statusChips(lastKnownView)).toEqual(["Last known"]);
    expect(
      lastKnownView.container.querySelector(".fq-status.status-stale")
        ?.textContent,
    ).toBe("Last known");

    const [unavailable] = floatingQuotaItems([unavailableClaude], now);
    expect(unavailable.statusLabel).toBe("Unavailable");
    expect(unavailable.statusClass).toBe("error");
    const unavailableView = render(card(unavailable));
    expect(statusChips(unavailableView)).toEqual(["Unavailable"]);

    const [unknown] = floatingQuotaItems([unknownClaude], now);
    expect(unknown.statusLabel).toBe("Unknown");
    expect(unknown.statusClass).toBe("unknown");
    const unknownView = render(card(unknown));
    expect(statusChips(unknownView)).toEqual(["Unknown"]);
  });

  it("keeps the statusLine bottom note on the Claude card", () => {
    const [item] = floatingQuotaItems([healthyClaude], now);
    const view = render(card(item));

    const note = view.container.querySelector(".fq-popover-note");
    expect(note?.textContent).toContain("Last statusLine event:");
    expect(note?.textContent).toContain("5h usage changed:");
    expect(note?.textContent).toContain("7d usage changed:");
    expect(note?.textContent).toContain(
      "Local event does not confirm a quota refresh",
    );
  });
});
