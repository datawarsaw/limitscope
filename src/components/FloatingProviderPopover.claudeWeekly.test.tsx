// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderUsage } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { FloatingProviderPopover } from "./FloatingProviderPopover";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const now = new Date("2026-11-17T12:00:00Z");

// The runtime reports Claude Code's weekly allowance under the raw "7-day"
// label; the card's detail row names it Weekly.
const claudeLimits = [
  { label: "5-hour", usedPercent: 23.5, resetAt: "2026-11-17T15:00:00.000Z" },
  { label: "7-day", usedPercent: 41.2, resetAt: "2026-11-20T00:00:00.000Z" },
];
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
    fiveHourChangedAt: "2026-11-17T11:58:00Z",
    sevenDayChangedAt: "2026-11-17T08:00:00Z",
  },
  limits: claudeLimits.map((limit) => ({ ...limit })),
};

// OpenDesign keeps its own 7-day spelling: nothing of its card may change.
const opendesign: ProviderUsage = {
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

const rowLabels = (view: { container: HTMLElement }) =>
  Array.from(view.container.querySelectorAll(".fq-window-label")).map(
    (el) => el.textContent,
  );
const rowValues = (view: { container: HTMLElement }) =>
  Array.from(view.container.querySelectorAll(".fq-window-value")).map(
    (el) => el.textContent,
  );

afterEach(cleanup);

describe("Claude Code weekly detail row", () => {
  it("names the detail row Weekly while every raw metric stays put", () => {
    const [item] = floatingQuotaItems([claude]);

    // The stored window metadata keeps the raw label the runtime reported:
    // primary-window selection, history and metrics all key on it.
    expect(item.windowLabel).toBe("7-day");
    expect(item.primaryReset?.label).toBe("7-day");
    expect(item.windows.map((w) => [w.label, w.usedPercent, w.resetAt])).toEqual([
      ["5-hour", 23.5, "2026-11-17T15:00:00.000Z"],
      ["7-day", 41.2, "2026-11-20T00:00:00.000Z"],
    ]);

    const view = render(card(item));

    // The detail row names the window Weekly, and the raw spelling does not
    // reach the card.
    expect(rowLabels(view)).toEqual(["5-hour", "Weekly"]);
    expect(view.container.textContent).not.toContain("7-day");

    // The headline and the countdown are unchanged.
    expect(view.container.querySelector(".fq-value-num")?.textContent).toBe(
      "59%",
    );
    expect(view.container.querySelector(".fq-value-unit")?.textContent).toBe(
      "remaining",
    );
    expect(
      view.container.querySelector(".fq-primary-reset")?.textContent,
    ).toBe("Weekly reset in 2d 12h");

    // The quota percentages and the server reset timestamps are unchanged.
    expect(item.percent).toBe(41);
    expect(item.usedPercent).toBe(41.2);
    expect(item.resetAt).toBe("2026-11-20T00:00:00.000Z");
    expect(item.primaryReset?.resetAt).toBe("2026-11-20T00:00:00.000Z");
    expect(rowValues(view)).toEqual(["77%", "59%"]);
    expect(
      view.container.querySelectorAll(".fq-window-row")[1].textContent,
    ).toContain("resets in 2d 12h");

    // The provider identity is unchanged, and the raw source the runtime
    // records (and the history store keys on) is never rewritten by a render.
    expect(item.providerId).toBe("claude-code");
    expect(item.name).toBe("Claude Code");
    expect(
      view.container.firstElementChild?.getAttribute("data-provider"),
    ).toBe("claude-code");
    expect(claude.limits).toEqual(claudeLimits);
    expect(claude.limits.map((limit) => limit.label)).toEqual([
      "5-hour",
      "7-day",
    ]);
  });

  it("leaves another provider's 7-day label untouched", () => {
    const [item] = floatingQuotaItems([opendesign]);
    expect(item.windowLabel).toBe("7-day");
    expect(item.windows.map((w) => w.label)).toEqual(["5-hour", "7-day"]);

    const view = render(card(item));
    expect(rowLabels(view)).toEqual(["5-hour", "7-day"]);
    expect(view.container.textContent).toContain("7-day");
    expect(
      view.container.querySelector(".fq-primary-reset")?.textContent,
    ).toContain("7-day");
  });
});
