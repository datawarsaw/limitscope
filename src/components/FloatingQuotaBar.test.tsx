// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { FloatingQuotaBar } from "./FloatingQuotaBar";
import type { FloatingQuotaItem } from "../lib/floatingQuota";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = new Date("2026-09-28T10:00:00.000Z");
const RESET_AT = "2026-09-28T18:24:00.000Z";

function item(extra: Partial<FloatingQuotaItem>): FloatingQuotaItem {
  return {
    providerId: "zai",
    name: "Z.ai",
    state: "healthy",
    percent: 22,
    windowLabel: "Weekly",
    resetAt: RESET_AT,
    windows: [{ label: "Weekly", usedPercent: 22, resetAt: RESET_AT }],
    primaryReset: { label: "Weekly", resetAt: RESET_AT },
    stale: false,
    statusLabel: "Live",
    statusClass: "ok",
    ...extra,
  };
}

const HEALTHY = item({});
const UNAVAILABLE = item({
  providerId: "grok",
  name: "Grok",
  state: "unavailable",
  percent: null,
  windowLabel: null,
  resetAt: null,
  windows: [],
  primaryReset: null,
  statusLabel: "Unknown",
  statusClass: "unknown",
});
const STALE = item({
  providerId: "opencode-go",
  name: "OpenCode Go",
  state: "stale",
  percent: 41,
  windowLabel: "5-hour",
  resetAt: null,
  windows: [{ label: "5-hour", usedPercent: 41, resetAt: null }],
  primaryReset: null,
  statusLabel: "Stale",
  statusClass: "stale",
  stale: true,
});
const ERROR = item({
  providerId: "antigravity",
  name: "Google Antigravity",
  state: "error",
  percent: 12,
  windowLabel: "Weekly",
  resetAt: null,
  windows: [{ label: "Weekly", usedPercent: 12, resetAt: null }],
  primaryReset: null,
  statusLabel: "Refresh failed",
  statusClass: "error",
  error: "refresh failed",
});

/** Two-window provider: the readout window is not the countdown window. */
const TWO_WINDOWS = item({
  providerId: "openai-codex",
  name: "OpenAI / Codex",
  percent: 90,
  usedPercent: 90,
  windowLabel: "5-hour",
  resetAt: "2026-09-28T14:00:00.000Z",
  windows: [
    { label: "5-hour", usedPercent: 90, resetAt: "2026-09-28T14:00:00.000Z" },
    { label: "Weekly", usedPercent: 30, resetAt: "2026-10-04T18:24:00.000Z" },
  ],
  primaryReset: { label: "Weekly", resetAt: "2026-10-04T18:24:00.000Z" },
});

type BarProps = Parameters<typeof FloatingQuotaBar>[0];

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

async function renderBar(overrides: Partial<BarProps> = {}) {
  const props: BarProps = {
    items: [HEALTHY],
    loading: false,
    now: NOW,
    detailId: null,
    pinned: false,
    menuOpen: false,
    alwaysOnTop: true,
    note: null,
    exiting: false,
    onHover: vi.fn(),
    onHoverEnd: vi.fn(),
    onHoverCancel: vi.fn(),
    onOpen: vi.fn(),
    onContextMenu: vi.fn(),
    onDismiss: vi.fn(),
    onOpenMain: vi.fn(),
    onRefresh: vi.fn(),
    onTogglePin: vi.fn(),
    onHide: vi.fn(),
    onDetailCardHeight: vi.fn(),
    ...overrides,
  };
  await act(async () => root.render(<FloatingQuotaBar {...props} />));
  return props;
}

async function click(element: Element) {
  await act(async () => {
    element.dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true }),
    );
  });
}

describe("FloatingQuotaBar shared detail card", () => {
  it("opens the detail card on click", async () => {
    const props = await renderBar({ items: [HEALTHY, UNAVAILABLE] });
    const provider = container.querySelector<HTMLElement>(
      '[data-provider-id="zai"]',
    );
    expect(provider).not.toBeNull();
    await click(provider!);
    expect(props.onOpen).toHaveBeenCalledWith("zai");
  });

  it("shows the remaining readout on the pinned card", async () => {
    await renderBar({ detailId: "zai", pinned: true, note: "Showing last good data" });
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog!.getAttribute("aria-label")).toBe("Z.ai");
    expect(dialog!.textContent).toContain("Z.ai");
    expect(dialog!.querySelector(".fq-value-num")!.textContent).toBe("78%");
    expect(dialog!.querySelector(".fq-value-unit")!.textContent).toBe(
      "remaining",
    );
    expect(dialog!.querySelector(".fq-meter-primary")).not.toBeNull();
    expect(dialog!.textContent).toContain("Weekly reset in");
    expect(dialog!.textContent).toContain("8h 24m");
    expect(dialog!.textContent).toContain("Showing last good data");
  });

  it("always shows remaining, with the meter filled by remaining capacity", async () => {
    const highUse = item({
      providerId: "openai-codex",
      name: "OpenAI / Codex",
      percent: 95,
      usedPercent: 95,
    });

    await renderBar({ items: [highUse], detailId: highUse.providerId, pinned: true });
    expect(
      container.querySelector('[data-provider-id="openai-codex"]')!
        .getAttribute("aria-label"),
    ).toContain("Critical · 5% remaining");
    const meter = container.querySelector(
      '[role="dialog"] .fq-meter-primary',
    )!;
    expect(meter.getAttribute("aria-valuenow")).toBe("5");
    expect(meter.getAttribute("aria-valuetext")).toBe("5% remaining");
    expect(meter.querySelector(".fq-meter-fill")!.getAttribute("style")).toContain(
      "width: 5%",
    );
    expect(container.querySelector(".fq-value-num")!.textContent).toBe("5%");
    expect(container.querySelector(".fq-value-unit")!.textContent).toBe(
      "remaining",
    );
  });

  it("renders the primary reset line above the meter, then the window inventory", async () => {
    await renderBar({
      items: [TWO_WINDOWS],
      detailId: TWO_WINDOWS.providerId,
      pinned: true,
    });
    const dialog = container.querySelector('[role="dialog"]')!;
    // Primary reset line first, in the approved "label reset in countdown"
    // shape, then every usable window as an inventory row with its own meter.
    expect(dialog.querySelector(".fq-primary-reset")!.textContent).toBe(
      "Weekly reset in 6d 8h",
    );
    const rows = Array.from(dialog.querySelectorAll(".fq-window-row"));
    expect(rows).toHaveLength(2);
    const rowText = rows.map((row) => row.textContent);
    expect(rowText[0]).toBe("5-hour10% remaining · resets in 4h 0m");
    expect(rowText[1]).toBe("Weekly70% remaining · resets in 6d 8h");
    expect(dialog.querySelectorAll(".fq-meter-row")).toHaveLength(2);
    // Row meters carry the semantic level of their own window.
    expect(
      rows[1].querySelector(".fq-meter-row")!.getAttribute("data-quota"),
    ).toBe("good");
  });

  it("keeps the account identity on a muted bottom line", async () => {
    const attributed = item({ accountLabel: "key ··3456" });
    await renderBar({
      items: [attributed],
      detailId: attributed.providerId,
      pinned: true,
    });
    const footer = container.querySelector(".fq-footer")!;
    expect(footer).not.toBeNull();
    const account = footer.querySelector(".fq-popover-account")!;
    expect(account).not.toBeNull();
    expect(account.textContent).toBe("account: key ··3456");
    const dialog = container.querySelector('[role="dialog"]')!;
    // Muted and last: nothing renders below the footer except actions.
    expect(footer.nextElementSibling?.className).toBe("fq-actions");
    expect(dialog.textContent).not.toContain("· account");
  });

  it("renders the identical card for hover preview and pinned click", async () => {
    const full = { ...TWO_WINDOWS, accountLabel: "key ··3456" };
    await renderBar({
      items: [full],
      detailId: full.providerId,
      pinned: false,
      note: "Showing last good data",
    });
    const hoverCard = container.querySelector(".fq-popover")!.outerHTML;
    await renderBar({
      items: [full],
      detailId: full.providerId,
      pinned: true,
      note: "Showing last good data",
    });
    const pinnedCard = container.querySelector(".fq-popover")!.outerHTML;
    // Same component: same markup, data, meters, reset line, note, and
    // account footer — no separate quick-view variant.
    expect(hoverCard).toBe(pinnedCard);
    expect(hoverCard).toContain('role="dialog"');
    expect(hoverCard).toContain(">Open</button>");
    expect(hoverCard).toContain("account: key ··3456");
    expect(hoverCard).toContain("Weekly reset in <strong>6d 8h</strong>");
    expect(hoverCard).toContain("Showing last good data");
  });

  it("keeps exactly one command action, Open, on the hover preview too", async () => {
    await renderBar({ detailId: "zai", pinned: false });
    const buttons = container.querySelectorAll('[role="dialog"] button');
    expect(buttons).toHaveLength(1);
    expect(buttons[0].textContent).toBe("Open");
  });

  it("no longer duplicates the menu commands in the detail card", async () => {
    await renderBar({ detailId: "zai", pinned: true });
    const text = container.querySelector('[role="dialog"]')!.textContent;
    expect(text).not.toContain("Refresh");
    expect(text).not.toContain("Pin");
    expect(text).not.toContain("Unpin");
    expect(text).not.toContain("Hide");
  });

  it("reports the measured card height and clears it when the card closes", async () => {
    const onDetailCardHeight = vi.fn();
    const rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockReturnValue({
        height: 320,
        width: 300,
        top: 0,
        left: 0,
        bottom: 320,
        right: 300,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);
    try {
      await renderBar({ detailId: "zai", pinned: true, onDetailCardHeight });
      // The window reserves exactly the rendered card, so no provider's card
      // can overflow — the overflow that scrolled the pill away.
      expect(onDetailCardHeight).toHaveBeenCalledWith(320);
      await renderBar({ detailId: null, pinned: false, onDetailCardHeight });
      expect(onDetailCardHeight).toHaveBeenCalledWith(null);
    } finally {
      rect.mockRestore();
    }
  });

  it("keeps the pill and the card composed in the same window on click", async () => {
    await renderBar({ items: [HEALTHY, UNAVAILABLE], detailId: "zai", pinned: true });
    const shell = container.querySelector(".fq-shell")!;
    const card = container.querySelector(".fq-popover")!;
    expect(shell).not.toBeNull();
    expect(card).not.toBeNull();
    // The card renders below the pill in the same document — a clicked
    // provider never swaps the bar for a detail-only surface.
    expect(
      card.compareDocumentPosition(shell) & Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy();
  });

  it("moves the active selector with the detail and rests when closed", async () => {
    const props = await renderBar({ items: [HEALTHY, UNAVAILABLE], detailId: "zai" });
    // The selector lands first and fades in on the next tick — it appears
    // where it stops, never sweeping in from a stale position.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    const selector = container.querySelector<HTMLElement>(".fq-active")!;
    expect(selector).not.toBeNull();
    expect(selector.getAttribute("data-on")).toBe("true");
    expect(selector.getAttribute("aria-hidden")).toBe("true");

    await renderBar({ ...props, detailId: null });
    expect(container.querySelector(".fq-active")!.getAttribute("data-on")).toBe(
      "false",
    );
  });

  it("re-keys the card body per provider so a switch can crossfade", async () => {
    const props = await renderBar({ items: [HEALTHY, STALE], detailId: "zai" });
    const first = container.querySelector(".fq-card");
    expect(first).not.toBeNull();

    await renderBar({ ...props, detailId: "opencode-go" });
    expect(container.querySelector(".fq-card")).not.toBe(first);
  });

  it("flags the closing card so it fades out without taking input", async () => {
    await renderBar({ detailId: "zai", exiting: true });
    const card = container.querySelector(".fq-popover")!;
    expect(card.className).toContain("is-exiting");

    await renderBar({ detailId: "zai", exiting: false });
    expect(container.querySelector(".fq-popover")!.className).not.toContain(
      "is-exiting",
    );
  });
});

describe("Grok detail card Grok Bot section", () => {
  /** Grok Weekly credits stay the primary quota; the Bot read is separate. */
  const GROK = item({
    providerId: "grok",
    name: "Grok",
    percent: 22,
    windowLabel: "Weekly credits",
    resetAt: RESET_AT,
    windows: [{ label: "On-demand", usedPercent: 9, resetAt: RESET_AT }],
    primaryReset: { label: "Weekly credits", resetAt: RESET_AT },
  });

  it("adds a Grok Bot section below the quota windows, before the actions", async () => {
    await renderBar({ items: [GROK], detailId: "grok", pinned: true });
    const dialog = container.querySelector('[role="dialog"]')!;
    const windows = dialog.querySelector(".fq-windows")!;
    const grokbot = dialog.querySelector(".fq-grokbot")!;
    const actions = dialog.querySelector(".fq-actions")!;
    expect(grokbot).not.toBeNull();
    expect(
      grokbot.compareDocumentPosition(windows) & Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy();
    expect(
      actions.compareDocumentPosition(grokbot) & Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy();
  });

  it("keeps Grok Weekly credits as the untouched primary quota", async () => {
    await renderBar({ items: [GROK], detailId: "grok", pinned: true });
    const dialog = container.querySelector('[role="dialog"]')!;
    expect(dialog.querySelector(".fq-value-num")!.textContent).toBe("78%");
    expect(dialog.querySelector(".fq-primary-reset")!.textContent).toBe(
      "Weekly credits reset in 8h 24m",
    );
    const rows = Array.from(dialog.querySelectorAll(".fq-window-row"));
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toBe("On-demand91% remaining · resets in 8h 24m");
    // The section carries its own meter, separate from the window meters.
    expect(dialog.querySelectorAll(".fq-meter-row")).toHaveLength(1);
  });

  it("renders the Grok Bot section for Grok only — other providers are untouched", async () => {
    await renderBar({ items: [HEALTHY, UNAVAILABLE], detailId: "zai", pinned: true });
    const dialog = container.querySelector('[role="dialog"]')!;
    expect(dialog.querySelector(".fq-grokbot")).toBeNull();
    // The non-Grok card keeps exactly its one Open action.
    const buttons = dialog.querySelectorAll("button");
    expect(buttons).toHaveLength(1);
    expect(buttons[0].textContent).toBe("Open");
    expect(dialog.textContent).not.toContain("Grok Bot");
  });

  it("still reports the measured card height with the section present", async () => {
    const onDetailCardHeight = vi.fn();
    const rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockReturnValue({
        height: 360,
        width: 300,
        top: 0,
        left: 0,
        bottom: 360,
        right: 300,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);
    try {
      await renderBar({ items: [GROK], detailId: "grok", pinned: true, onDetailCardHeight });
      expect(onDetailCardHeight).toHaveBeenCalledWith(360);
    } finally {
      rect.mockRestore();
    }
  });
});

describe("FloatingQuotaBar right-click menu", () => {
  it("dismisses on an outside left press and browser blur, but not inside or right press", async () => {
    const props = await renderBar({ menuOpen: true });
    await act(async () => {
      container.querySelector(".fq-menu button")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 2 }));
    });
    expect(props.onDismiss).not.toHaveBeenCalled();
    await act(async () => {
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
      window.dispatchEvent(new Event("blur"));
    });
    expect(props.onDismiss).toHaveBeenCalledTimes(2);
  });

  it("prevents the native WebView menu on overlays and opens provider menus once", async () => {
    const props = await renderBar({ menuOpen: true });
    const overlayEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    await act(async () => container.querySelector(".fq-menu button")!.dispatchEvent(overlayEvent));
    expect(overlayEvent.defaultPrevented).toBe(true);
    expect(props.onContextMenu).not.toHaveBeenCalled();
    await act(async () => container.querySelector(".fq-item")!.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
    ));
    expect(props.onContextMenu).toHaveBeenCalledTimes(1);
  });

  it("keeps the four menu commands", async () => {
    await renderBar({ menuOpen: true });
    const menu = container.querySelector('[role="group"]');
    expect(menu).not.toBeNull();
    const labels = Array.from(menu!.querySelectorAll("button")).map(
      (button) => button.textContent,
    );
    expect(labels).toEqual([
      "Refresh",
      "Open main window",
      "Always on top",
      "Hide floating bar",
    ]);
  });

  it("keeps the menu callbacks", async () => {
    const props = await renderBar({ menuOpen: true });
    const menu = container.querySelector('[role="group"]')!;
    const [refresh, openMain, alwaysOnTop, hide] = menu.querySelectorAll(
      "button",
    );
    await click(refresh);
    await click(openMain);
    await click(alwaysOnTop);
    await click(hide);
    expect(props.onRefresh).toHaveBeenCalledTimes(1);
    expect(props.onOpenMain).toHaveBeenCalledTimes(1);
    expect(props.onTogglePin).toHaveBeenCalledTimes(1);
    expect(props.onDismiss).toHaveBeenCalledTimes(2);
    expect(props.onHide).toHaveBeenCalledTimes(1);
  });

  it("marks always on top as a pressed toggle, not a menu checkbox", async () => {
    await renderBar({ menuOpen: true, alwaysOnTop: true });
    const toggle = Array.from(
      container.querySelectorAll('[role="group"] button'),
    ).find((button) => button.textContent === "Always on top");
    expect(toggle).toBeDefined();
    expect(toggle!.getAttribute("aria-pressed")).toBe("true");
    expect(toggle!.getAttribute("role")).toBeNull();
  });

  it("gains only the dock action while the dock feature is active", async () => {
    const props = await renderBar({ menuOpen: true, onDockToggle: vi.fn() });
    const menu = container.querySelector('[role="group"]')!;
    const labels = Array.from(menu.querySelectorAll("button")).map(
      (button) => button.textContent,
    );
    // Inserted before Hide; the four production commands stay untouched.
    expect(labels).toEqual([
      "Refresh",
      "Open main window",
      "Always on top",
      "Dock to top edge",
      "Hide floating bar",
    ]);

    await click(
      Array.from(menu.querySelectorAll("button")).find(
        (button) => button.textContent === "Dock to top edge",
      )!,
    );
    expect(props.onDockToggle).toHaveBeenCalledTimes(1);
    expect(props.onDismiss).toHaveBeenCalledTimes(1);
  });

  it("labels the dock action as undock while docked", async () => {
    await renderBar({ menuOpen: true, onDockToggle: vi.fn(), docked: true });
    const labels = Array.from(
      container.querySelectorAll('[role="group"] button'),
    ).map((button) => button.textContent);
    expect(labels).toContain("Undock from top edge");
    expect(labels).not.toContain("Dock to top edge");
  });
});

describe("FloatingQuotaBar drag surface", () => {
  it("exposes the root, shell, and grip as drag regions", async () => {
    await renderBar({ items: [HEALTHY] });
    expect(
      container.querySelector(".fq-root")!.hasAttribute("data-tauri-drag-region"),
    ).toBe(true);
    expect(
      container.querySelector(".fq-shell")!.hasAttribute("data-tauri-drag-region"),
    ).toBe(true);
    const grip = container.querySelector(".fq-grip");
    expect(grip).not.toBeNull();
    expect(grip!.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(grip!.getAttribute("aria-hidden")).toBe("true");
    expect(grip!.tagName).not.toBe("BUTTON");
  });

  it("keeps provider buttons out of the drag region", async () => {
    await renderBar({ items: [HEALTHY, UNAVAILABLE, STALE, ERROR] });
    const providers = container.querySelectorAll(".fq-item");
    expect(providers.length).toBe(4);
    for (const provider of providers) {
      expect(provider.hasAttribute("data-tauri-drag-region")).toBe(false);
    }
  });

  it("renders the dock strip inside the drag root, under the shell", async () => {
    await renderBar({
      items: [HEALTHY],
      dockSlot: <div className="fq-dock-strip" />,
      dockState: "resting",
    });
    const root = container.querySelector(".fq-root")!;
    const slot = root.querySelector(".fq-dock-strip");
    expect(slot).not.toBeNull();
    // Painted before the shell so the revealed pill crossfades in above it.
    expect(
      slot!.compareDocumentPosition(root.querySelector(".fq-shell")!) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(root.getAttribute("data-dock-state")).toBe("resting");
  });

  it("reports the pointer entering and leaving the whole window", async () => {
    const onPointerEnterWindow = vi.fn();
    const onPointerLeaveWindow = vi.fn();
    const props = await renderBar({
      items: [HEALTHY],
      onPointerEnterWindow,
      onPointerLeaveWindow,
    });
    const root = container.querySelector(".fq-root")!;
    // React derives enter/leave from delegated mouseover/mouseout; a null
    // relatedTarget means the window edge.
    await act(async () => {
      root.dispatchEvent(
        new MouseEvent("mouseover", { bubbles: true, relatedTarget: null }),
      );
    });
    await act(async () => {
      root.dispatchEvent(
        new MouseEvent("mouseout", { bubbles: true, relatedTarget: null }),
      );
    });
    expect(props.onPointerEnterWindow).toHaveBeenCalledTimes(1);
    expect(props.onPointerLeaveWindow).toHaveBeenCalledTimes(1);
  });
});

describe("FloatingQuotaBar persistent bar", () => {
  it("keeps the compact readout per state and stays untouched by the detail card", async () => {
    await renderBar({
      items: [HEALTHY, UNAVAILABLE, STALE, ERROR],
      detailId: "zai",
      pinned: true,
    });
    const percent = (id: string) =>
      container.querySelector(`[data-provider-id="${id}"] .fq-percent`)!
        .textContent;
    expect(percent("zai")).toBe("78%");
    expect(percent("grok")).toBe("—");
    expect(
      container
        .querySelector('[data-provider-id="grok"] .fq-meter')!
        .getAttribute("aria-valuetext"),
    ).toBe("unavailable");
    expect(
      container.querySelector('[data-provider-id="opencode-go"] .fq-stale-dot'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-provider-id="antigravity"] .fq-warn'),
    ).not.toBeNull();
  });

  it("keeps the collapsed segment countdown-free; reset timing stays on the card", async () => {
    await renderBar({ items: [HEALTHY, TWO_WINDOWS, STALE] });
    expect(container.querySelector(".fq-reset")).toBeNull();
    await renderBar({
      items: [TWO_WINDOWS],
      detailId: "openai-codex",
      pinned: false,
    });
    expect(
      container.querySelector('[role="dialog"]')!.textContent,
    ).toContain("Weekly reset in 6d 8h");
  });

  it("keeps degraded statuses badged in the details header", async () => {
    await renderBar({
      items: [HEALTHY, STALE, ERROR],
      detailId: "opencode-go",
      pinned: false,
    });
    expect(
      container.querySelector(".fq-popover .fq-status")!.textContent,
    ).toBe("Stale");
    await renderBar({
      items: [HEALTHY, STALE, ERROR],
      detailId: "antigravity",
      pinned: false,
    });
    expect(
      container.querySelector(".fq-popover .fq-status")!.textContent,
    ).toBe("Refresh failed");
  });

  it("lists every usable window as an inventory row in the hover preview", async () => {
    await renderBar({
      items: [TWO_WINDOWS],
      detailId: "openai-codex",
      pinned: false,
    });
    const card = container.querySelector('[role="dialog"]')!;
    // Primary reset line, then the full window inventory with per-window
    // meters — including the window the reset line names.
    expect(card.querySelector(".fq-primary-reset")!.textContent).toBe(
      "Weekly reset in 6d 8h",
    );
    const rows = Array.from(card.querySelectorAll(".fq-window-row"));
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toBe("5-hour10% remaining · resets in 4h 0m");
    expect(rows[1].textContent).toBe("Weekly70% remaining · resets in 6d 8h");
  });

  it("keeps a single-window card structured like the multi-window one", async () => {
    await renderBar({ detailId: "zai", pinned: false });
    const card = container.querySelector('[role="dialog"]')!;
    expect(card.querySelector(".fq-primary-reset")!.textContent).toBe(
      "Weekly reset in 8h 24m",
    );
    const rows = Array.from(card.querySelectorAll(".fq-window-row"));
    expect(rows).toHaveLength(1);
    expect(rows[0].querySelector(".fq-window-label")!.textContent).toBe(
      "Weekly",
    );
    expect(rows[0].querySelector(".fq-meter-row")).not.toBeNull();
  });

  it("colors quota by semantic level, not provider", async () => {
    const low = item({
      providerId: "opencode-go",
      name: "OpenCode Go",
      percent: 80,
      usedPercent: 80,
    });
    const critical = item({
      providerId: "antigravity",
      name: "Google Antigravity",
      percent: 100,
      usedPercent: 100,
      windows: [{ label: "Weekly", usedPercent: 100, resetAt: RESET_AT }],
    });
    await renderBar({ items: [HEALTHY, low, critical, UNAVAILABLE] });
    const quota = (id: string) =>
      container
        .querySelector(`[data-provider-id="${id}"]`)!
        .getAttribute("data-quota");
    expect(quota("zai")).toBe("good"); // HEALTHY: 22 used → 78% remaining
    expect(quota("opencode-go")).toBe("low"); // 20% remaining
    expect(quota("antigravity")).toBe("critical"); // 0% remaining
    expect(quota("grok")).toBe("unavailable");

    await renderBar({
      items: [item({ percent: 95, usedPercent: 95 })],
      detailId: "zai",
      pinned: true,
    });
    expect(
      container.querySelector('[role="dialog"]')!.getAttribute("data-quota"),
    ).toBe("critical"); // 5% remaining
  });
});
