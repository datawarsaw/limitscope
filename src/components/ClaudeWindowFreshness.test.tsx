// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderUsage } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { providerStatusPresentation } from "../lib/v03Integration";
import { QuotaWindowList } from "./dashboard/QuotaWindowList";
import { FloatingProviderPopover } from "./FloatingProviderPopover";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const NOW = new Date("2026-11-17T12:00:00.000Z");

function makeClaudeUsage(
  overrides: Partial<ProviderUsage> = {},
): ProviderUsage {
  return {
    id: "claude-code",
    name: "Claude Code",
    health: "stale",
    status: "stale",
    checkedAt: "2026-11-17T10:00:00.000Z",
    sourceUpdatedAt: "2026-11-17T10:00:00.000Z",
    dataFreshness: "stale",
    claudeStatuslineTimes: {
      receiptAt: "2026-11-17T11:59:30.000Z",
      fiveHourChangedAt: "2026-11-17T11:59:30.000Z",
      sevenDayChangedAt: "2026-11-17T10:00:00.000Z",
    },
    limits: [
      { label: "5-hour", usedPercent: 18, resetAt: "2026-11-17T15:00:00.000Z" },
      { label: "7-day", usedPercent: 41, resetAt: "2026-11-20T00:00:00.000Z" },
    ],
    ...overrides,
  };
}

function renderBoth(usage: ProviderUsage) {
  const main = render(
    <QuotaWindowList
      usage={usage}
      now={NOW}
      predictionFor={() => undefined}
      perspective="remaining"
    />,
  );
  const [item] = floatingQuotaItems([usage]);
  const floating = render(
    <FloatingProviderPopover
      item={item}
      pinned={false}
      now={NOW}
      note={null}
      align={0}
      exiting={false}
      onMouseEnter={() => {}}
      onMouseLeave={() => {}}
      onFocus={() => {}}
      onBlur={() => {}}
    />,
  );
  return { main, floating, item };
}

function badges(container: HTMLElement) {
  return Array.from(
    container.querySelectorAll(".window-freshness-badge"),
  ).map((el) => ({
    text: el.textContent,
    ariaLabel: el.getAttribute("aria-label"),
    className: el.className,
  }));
}

afterEach(cleanup);

describe("Claude per-window freshness parity", () => {
  it("matches screenshot case (5h recent, 7d 2h stale) while overall remains Last known", () => {
    const claude = makeClaudeUsage();
    expect(providerStatusPresentation(claude)).toEqual({
      label: "Last known",
      className: "stale",
    });

    const { main, floating, item } = renderBoth(claude);
    expect(item.statusLabel).toBe("Last known");

    const mainBadges = badges(main.container);
    const floatingBadges = badges(floating.container);
    expect(mainBadges.map((b) => b.text)).toEqual(["Recent", "Last known · 2h"]);
    expect(floatingBadges.map((b) => b.text)).toEqual(["Recent", "Last known · 2h"]);
    expect(mainBadges[0].ariaLabel).toBe("5-hour freshness: Recent");
    expect(mainBadges[1].ariaLabel).toBe("7-day freshness: Last known · 2h");
    expect(floatingBadges[0].ariaLabel).toBe("5-hour freshness: Recent");
    expect(floatingBadges[1].ariaLabel).toBe("Weekly freshness: Last known · 2h");
    expect(mainBadges[0].className).toContain("window-freshness-ok");
    expect(mainBadges[1].className).toContain("window-freshness-stale");
    expect(floatingBadges[0].className).toContain("window-freshness-ok");
    expect(floatingBadges[1].className).toContain("window-freshness-stale");

    const floatingLabels = Array.from(
      floating.container.querySelectorAll(".fq-window-label"),
    ).map((el) => el.textContent);
    expect(floatingLabels).toEqual(["5-hour", "Weekly"]);
    expect(floating.container.textContent).not.toContain("7-day");
  });

  it("handles reverse case (5h stale, 7d recent) with parity", () => {
    const claude = makeClaudeUsage({
      claudeStatuslineTimes: {
        receiptAt: "2026-11-17T11:59:00.000Z",
        fiveHourChangedAt: "2026-11-17T11:35:00.000Z",
        sevenDayChangedAt: "2026-11-17T11:55:00.000Z",
      },
    });
    const { main, floating } = renderBoth(claude);
    expect(badges(main.container).map((b) => b.text)).toEqual([
      "Last known · 25m",
      "Recent",
    ]);
    expect(badges(floating.container).map((b) => b.text)).toEqual([
      "Last known · 25m",
      "Recent",
    ]);
  });

  it("handles both recent and both stale with parity", () => {
    const bothRecent = makeClaudeUsage({
      health: "live",
      status: "ok",
      dataFreshness: "fresh",
      sourceUpdatedAt: "2026-11-17T11:50:00.000Z",
      claudeStatuslineTimes: {
        receiptAt: "2026-11-17T11:58:00.000Z",
        fiveHourChangedAt: "2026-11-17T11:58:00.000Z",
        sevenDayChangedAt: "2026-11-17T11:45:00.000Z",
      },
    });
    const recentRender = renderBoth(bothRecent);
    expect(badges(recentRender.main.container).map((b) => b.text)).toEqual([
      "Recent",
      "Recent",
    ]);
    expect(badges(recentRender.floating.container).map((b) => b.text)).toEqual([
      "Recent",
      "Recent",
    ]);
    cleanup();

    const bothStale = makeClaudeUsage({
      claudeStatuslineTimes: {
        receiptAt: "2026-11-17T11:58:00.000Z",
        fiveHourChangedAt: "2026-11-17T11:00:00.000Z",
        sevenDayChangedAt: "2026-11-15T12:00:00.000Z",
      },
    });
    const staleRender = renderBoth(bothStale);
    expect(badges(staleRender.main.container).map((b) => b.text)).toEqual([
      "Last known · 1h",
      "Last known · 2d",
    ]);
    expect(badges(staleRender.floating.container).map((b) => b.text)).toEqual([
      "Last known · 1h",
      "Last known · 2d",
    ]);
  });

  it("treats missing, invalid, and future timestamps as Unknown, never fresh", () => {
    const badTimes = makeClaudeUsage({
      claudeStatuslineTimes: {
        receiptAt: "2026-11-17T11:59:00.000Z",
        fiveHourChangedAt: "not-a-date",
        sevenDayChangedAt: "2026-11-17T13:00:00.000Z",
      },
    });
    const first = renderBoth(badTimes);
    expect(badges(first.main.container).map((b) => [b.text, b.className])).toEqual([
      ["Unknown", expect.stringContaining("window-freshness-unknown")],
      ["Unknown", expect.stringContaining("window-freshness-unknown")],
    ]);
    expect(badges(first.floating.container).map((b) => [b.text, b.className])).toEqual([
      ["Unknown", expect.stringContaining("window-freshness-unknown")],
      ["Unknown", expect.stringContaining("window-freshness-unknown")],
    ]);
    cleanup();

    const missingTimes = makeClaudeUsage({
      claudeStatuslineTimes: undefined,
    });
    const second = renderBoth(missingTimes);
    expect(badges(second.main.container).map((b) => b.text)).toEqual([
      "Unknown",
      "Unknown",
    ]);
    expect(badges(second.floating.container).map((b) => b.text)).toEqual([
      "Unknown",
      "Unknown",
    ]);
  });

  it("renders no freshness badges for non-Claude providers", () => {
    const opendesign: ProviderUsage = {
      id: "opendesign",
      name: "OpenDesign",
      health: "live",
      status: "ok",
      checkedAt: "2026-11-17T11:58:00.000Z",
      sourceUpdatedAt: "2026-11-17T11:58:00.000Z",
      dataFreshness: "fresh",
      limits: [
        { label: "5-hour", usedPercent: 20, resetText: "Resets in 39 min" },
        { label: "7-day", usedPercent: 10, resetText: "Resets in 6 days" },
      ],
    };
    const { main, floating } = renderBoth(opendesign);
    expect(badges(main.container)).toEqual([]);
    expect(badges(floating.container)).toEqual([]);
  });
});
