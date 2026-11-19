// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderUsage } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { providerStatusPresentation } from "../lib/v03Integration";
import { PrimaryQuotaPanel } from "./dashboard/PrimaryQuotaPanel";
import { ProviderRail } from "./dashboard/ProviderRail";
import { QuotaWindowList } from "./dashboard/QuotaWindowList";
import { FloatingProviderItem } from "./FloatingProviderItem";
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
      fiveHourChangedAt: "2026-11-17T10:00:00.000Z",
      sevenDayChangedAt: "2026-11-17T09:00:00.000Z",
      fiveHourConfirmedAt: "2026-11-17T11:59:30.000Z",
    },
    limits: [
      { label: "5-hour", usedPercent: 18, resetAt: "2026-11-17T15:00:00.000Z" },
      { label: "7-day", usedPercent: 41, resetAt: "2026-11-20T00:00:00.000Z" },
    ],
    ...overrides,
  };
}

function renderSurfaces(usage: ProviderUsage) {
  invoke.mockResolvedValue({ command: null, powershellCommand: null, settingsSnippet: null });
  const primary = render(
    <PrimaryQuotaPanel
      usage={usage}
      now={NOW}
      predictionFor={() => undefined}
      perspective="remaining"
    />,
  );
  const rail = render(
    <ProviderRail
      usages={[usage]}
      selectedId={usage.id}
      onSelect={() => {}}
      perspective="remaining"
      now={NOW}
    />,
  );
  const windows = render(
    <QuotaWindowList
      usage={usage}
      now={NOW}
      predictionFor={() => undefined}
      perspective="remaining"
    />,
  );
  const [item] = floatingQuotaItems([usage], NOW);
  const pill = render(
    <FloatingProviderItem
      item={item}
      expanded={false}
      onHover={() => {}}
      onHoverEnd={() => {}}
      onOpen={() => {}}
      onContextMenu={() => {}}
    />,
  );
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
  return { primary, rail, windows, pill, floating, item };
}

afterEach(cleanup);

describe("Claude 5h-only header/rail/floating status without per-row badges", () => {
  it("shows single green Recent header, rail dot, and healthy floating pill when equal 5h is freshly confirmed and 7d is hours old, with zero row badges", () => {
    const claude = makeClaudeUsage();
    expect(providerStatusPresentation(claude, NOW)).toEqual({
      label: "Recent",
      className: "ok",
    });

    const { primary, rail, windows, pill, floating, item } = renderSurfaces(claude);

    // Zero per-row freshness badges in either surface
    expect(windows.container.querySelectorAll(".window-freshness-badge")).toHaveLength(0);
    expect(floating.container.querySelectorAll(".window-freshness-badge")).toHaveLength(0);

    // Main header: exactly one status label, green dot, Recent
    const headerStatus = primary.container.querySelectorAll(".status-label");
    expect(headerStatus).toHaveLength(1);
    expect(headerStatus[0].textContent).toBe("Recent");
    expect(headerStatus[0].querySelector(".status-dot")?.className).toContain("status-ok");

    // Diagnostic text preserves accurate changed/event timestamps
    const note = primary.container.querySelector(".primary-source")?.textContent ?? "";
    expect(note).toContain("Last statusLine event: just now");
    expect(note).toContain("5h usage changed: 2h ago");
    expect(note).toContain("7d usage changed: 3h ago");

    // Rail dot: green status-ok
    expect(rail.container.querySelector(".rail-item .status-dot")?.className).toContain("status-ok");

    // Floating pill & popover: Recent, status-ok, no stale dot
    expect(item.statusLabel).toBe("Recent");
    expect(item.statusClass).toBe("ok");
    expect(item.stale).toBe(false);
    expect(pill.container.querySelector(".fq-stale-dot")).toBeNull();
    const floatingStatus = floating.container.querySelectorAll(".fq-status");
    expect(floatingStatus).toHaveLength(1);
    expect(floatingStatus[0].textContent).toBe("Recent");
    expect(floatingStatus[0].className).toContain("status-ok");
  });

  it("shows Last known when only generic hook or weekly-only event is recent, or when 5h is stale and 7d is recent", () => {
    const genericOnly = makeClaudeUsage({
      claudeStatuslineTimes: {
        receiptAt: "2026-11-17T11:59:50.000Z",
        fiveHourChangedAt: "2026-11-17T10:00:00.000Z",
        sevenDayChangedAt: "2026-11-17T09:00:00.000Z",
      },
    });
    const first = renderSurfaces(genericOnly);
    expect(first.primary.container.querySelector(".status-label")?.textContent).toBe("Last known");
    expect(first.primary.container.querySelector(".status-dot")?.className).toContain("status-stale");
    expect(first.rail.container.querySelector(".rail-item .status-dot")?.className).toContain("status-stale");
    expect(first.pill.container.querySelector(".fq-stale-dot")).not.toBeNull();
    expect(first.floating.container.querySelector(".fq-status")?.textContent).toBe("Last known");
    expect(first.floating.container.querySelector(".fq-status")?.className).toContain("status-stale");
    cleanup();

    const fiveStaleSevenRecent = makeClaudeUsage({
      claudeStatuslineTimes: {
        receiptAt: "2026-11-17T11:59:30.000Z",
        fiveHourChangedAt: "2026-11-17T11:40:00.000Z",
        sevenDayChangedAt: "2026-11-17T11:59:30.000Z",
      },
    });
    const second = renderSurfaces(fiveStaleSevenRecent);
    expect(second.primary.container.querySelector(".status-label")?.textContent).toBe("Last known");
    expect(second.floating.container.querySelector(".fq-status")?.textContent).toBe("Last known");
    cleanup();

    const weeklyOnly = makeClaudeUsage({
      health: "live",
      status: "ok",
      dataFreshness: "fresh",
      sourceUpdatedAt: "2026-11-17T11:59:30.000Z",
      claudeStatuslineTimes: {
        receiptAt: "2026-11-17T11:59:30.000Z",
        sevenDayChangedAt: "2026-11-17T11:59:30.000Z",
      },
      limits: [{ label: "7-day", usedPercent: 41, resetAt: "2026-11-20T00:00:00.000Z" }],
    });
    const third = renderSurfaces(weeklyOnly);
    expect(third.primary.container.querySelector(".status-label")?.textContent).toBe("Last known");
    expect(third.floating.container.querySelector(".fq-status")?.textContent).toBe("Last known");
  });

  it("keeps non-Claude providers unchanged and renders zero row freshness badges", () => {
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
    const { primary, rail, windows, floating } = renderSurfaces(opendesign);
    expect(primary.container.querySelector(".status-label")?.textContent).toBe("Cached");
    expect(rail.container.querySelector(".rail-item .status-dot")?.className).toContain("status-ok");
    expect(windows.container.querySelectorAll(".window-freshness-badge")).toHaveLength(0);
    expect(floating.container.querySelectorAll(".window-freshness-badge")).toHaveLength(0);
  });
});
