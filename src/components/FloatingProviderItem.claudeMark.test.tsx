// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderUsage } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { FloatingProviderItem } from "./FloatingProviderItem";
import { FloatingProviderPopover } from "./FloatingProviderPopover";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const now = new Date("2026-11-17T12:00:00Z");

const claude: ProviderUsage = {
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
  ],
};

/** The same provider under an account-scoped id. */
const scopedClaude: ProviderUsage = { ...claude, id: "claude-code:secondary" };

const segment = (item: ReturnType<typeof floatingQuotaItems>[number]) => (
  <FloatingProviderItem
    item={item}
    expanded={false}
    onHover={() => {}}
    onHoverEnd={() => {}}
    onOpen={() => {}}
    onContextMenu={() => {}}
  />
);

const card = (item: ReturnType<typeof floatingQuotaItems>[number]) => (
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

/**
 * The published Claude mark: Anthropic's claude-logo.svg geometry, at the
 * published viewBox and in the published brand colour. The monogram that
 * stood in for it is gone.
 */
const claudeMark = (view: { container: HTMLElement }) => {
  const svg = view.container.querySelector(".fq-mark svg");
  expect(svg).not.toBeNull();
  expect(svg?.getAttribute("viewBox")).toBe("0 0 24 24");
  const path = svg?.querySelector("path");
  expect(path?.getAttribute("fill")).toBe("#D97757");
  const d = path?.getAttribute("d") ?? "";
  expect(d.startsWith("M11.376 24")).toBe(true);
  expect(d.endsWith("L11.376 24Z")).toBe(true);
  return d;
};

afterEach(cleanup);

describe("Claude Code provider mark", () => {
  it("draws the published Claude mark on the collapsed segment", () => {
    const [item] = floatingQuotaItems([claude], now);
    const view = render(segment(item));

    claudeMark(view);

    // The "CC" monogram is gone: no letter fallback stands in for the mark.
    expect(view.container.querySelector(".fq-mark-fallback")).toBeNull();
    expect(view.container.textContent).not.toContain("CC");
  });

  it("draws the same mark for an account-scoped Claude id", () => {
    const [item] = floatingQuotaItems([scopedClaude], now);
    const view = render(segment(item));

    claudeMark(view);
    expect(view.container.querySelector(".fq-mark-fallback")).toBeNull();
  });

  it("shares one mark between the collapsed segment and the expanded card", () => {
    const [scoped] = floatingQuotaItems([scopedClaude], now);

    const segmentView = render(segment(scoped));
    const segmentPath = claudeMark(segmentView);

    const cardView = render(card(scoped));
    const cardSvg = cardView.container.querySelector(".fq-popover-mark svg");
    expect(cardSvg?.getAttribute("viewBox")).toBe("0 0 24 24");
    const cardPath = cardSvg?.querySelector("path");
    expect(cardPath?.getAttribute("fill")).toBe("#D97757");

    // One identity, two places: the expanded card wears the mark of the
    // segment it expands, character for character.
    expect(cardPath?.getAttribute("d")).toBe(segmentPath);
  });
});
