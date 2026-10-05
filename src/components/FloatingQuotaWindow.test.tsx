// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

vi.mock("../hooks/useNow", () => ({
  useNow: () => Date.parse("2026-09-28T10:00:00.000Z"),
}));

vi.mock("../hooks/useSettings", () => ({
  useSettings: () => ({
    settings: {
      launchAtStartup: false,
      refreshIntervalMinutes: 5,
      theme: "graphite",
      quotaNotifications: false,
      providerPreferences: mocks.prefs,
    },
  }),
}));

vi.mock("../hooks/useProviderUsage", () => ({
  useProviderUsage: () => ({
    usages: [
      {
        id: "zai",
        name: "Z.ai",
        status: "ok",
        health: "live",
        checkedAt: "2026-09-28T09:50:00.000Z",
        limits: [
          { label: "Weekly", usedPercent: 22, resetAt: "2026-09-28T18:24:00.000Z" },
        ],
      },
      {
        id: "grok",
        name: "Grok",
        status: "unknown",
        health: "unknown",
        checkedAt: "2026-09-28T09:50:00.000Z",
        limits: [],
      },
    ],
    loading: false,
    refresh: vi.fn(),
  }),
}));

vi.mock("../lib/floatingWindowChrome", () => ({
  isTauriRuntime: () => false,
  attachFloatingChrome: (
    _overlayOpen: () => boolean,
    onDockChange?: (dock: "none" | "top") => void,
    onBlur?: () => void,
    _signal?: AbortSignal,
    onGesturePreview?: (gesture: "snap" | "snap-ready" | "unsnap" | null) => void,
  ) => {
    mocks.onDockChange = onDockChange;
    mocks.onBlur = onBlur;
    mocks.onGesturePreview = onGesturePreview;
    return Promise.resolve(() => {});
  },
  attachDockReanchor: (_onReanchor: () => void) => () => {},
  resizeFloatingWindow: (
    width: number,
    height: number,
    overlay: boolean,
    docked: boolean,
  ) => {
    mocks.resizeCalls.push([width, height, overlay, docked]);
  },
  setFloatingDock: () => Promise.resolve(),
  setFloatingDockConstraints: () => Promise.resolve(),
  openMainWindow: () => Promise.resolve(),
  requestGlobalRefresh: () => Promise.resolve(),
  hideFloatingWindow: () => Promise.resolve(),
  setFloatingPinned: () => Promise.resolve(),
}));

import { dockMeterRects } from "../lib/floatingQuota";
import { FLOATING_PREFS_STORAGE_KEY } from "../lib/floatingWindowPrefs";
import { DOCK_REVEAL_MS, FloatingQuotaWindow } from "./FloatingQuotaWindow";

const mocks = vi.hoisted(() => ({
  prefs: { order: [] as string[], hidden: [] as string[] },
  resizeCalls: [] as Array<[number, number, boolean, boolean]>,
  onDockChange: undefined as undefined | ((dock: "none" | "top") => void),
  onBlur: undefined as undefined | (() => void),
  onGesturePreview: undefined as
    | undefined
    | ((gesture: "snap" | "snap-ready" | "unsnap" | null) => void),
}));

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  mocks.resizeCalls.length = 0;
  mocks.onDockChange = undefined;
  mocks.onGesturePreview = undefined;
  window.localStorage.clear();
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

async function renderWindow() {
  await act(async () => root.render(<FloatingQuotaWindow />));
}

async function click(element: Element) {
  await act(async () => {
    element.dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true }),
    );
  });
}

// React derives onMouseEnter/onMouseLeave from delegated mouseover/mouseout,
// so the pointer helpers dispatch those with a relatedTarget.
async function hoverIn(element: Element, from: Element | null = null) {
  await act(async () => {
    element.dispatchEvent(
      new MouseEvent("mouseover", { bubbles: true, relatedTarget: from }),
    );
  });
}

async function hoverOut(element: Element, to: Element | null = null) {
  await act(async () => {
    element.dispatchEvent(
      new MouseEvent("mouseout", { bubbles: true, relatedTarget: to }),
    );
  });
}

function card() {
  return document.querySelector<HTMLElement>(".fq-popover");
}

async function advance(ms: number) {
  await act(async () => {
    vi.advanceTimersByTime(ms);
  });
}

function withFakeTimers(run: () => Promise<void>) {
  return async () => {
    vi.useFakeTimers();
    try {
      await run();
    } finally {
      vi.useRealTimers();
    }
  };
}

describe("FloatingQuotaWindow detail card interaction", () => {
  it("dismisses the menu on native blur and releases overlay state, while preserving a pinned card", async () => {
    await renderWindow();
    const provider = document.querySelector('[data-provider-id="zai"]')!;
    await act(async () => provider.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true })));
    expect(document.querySelector(".fq-menu")).not.toBeNull();
    await act(async () => mocks.onBlur?.());
    expect(document.querySelector(".fq-menu")).toBeNull();
    expect(mocks.resizeCalls.at(-1)?.[2]).toBe(false);
    await click(provider);
    await act(async () => mocks.onBlur?.());
    expect(card()).not.toBeNull();
    expect(card()?.classList.contains("is-exiting")).toBe(false);
  });

  it(
    "opens the detail card on hover and closes it after the pointer leaves",
    withFakeTimers(async () => {
      await renderWindow();
      const provider = document.querySelector<HTMLElement>(
        '[data-provider-id="zai"]',
      );
      expect(provider).not.toBeNull();

      await hoverIn(provider!);
      expect(card()).not.toBeNull();
      expect(card()!.getAttribute("data-provider")).toBe("zai");

      await hoverOut(provider!);
      expect(card()).not.toBeNull();
      // The close is a 140ms grace, then a 120ms exit fade — still on screen
      // mid-fade, gone after it completes.
      await advance(200);
      expect(card()!.className).toContain("is-exiting");
      await advance(150);
      expect(card()).toBeNull();
    }),
  );

  it(
    "reopening during the close fade cancels the exit",
    withFakeTimers(async () => {
      await renderWindow();
      const provider = document.querySelector<HTMLElement>(
        '[data-provider-id="zai"]',
      )!;

      await hoverIn(provider);
      await hoverOut(provider);
      await advance(200);
      expect(card()!.className).toContain("is-exiting");

      await hoverIn(provider);
      const reopened = card()!;
      expect(reopened.className).not.toContain("is-exiting");
      expect(reopened.getAttribute("data-provider")).toBe("zai");
      await advance(300);
      expect(card()).not.toBeNull();
    }),
  );

  it(
    "keeps the pinned card open when the pointer leaves provider and card",
    withFakeTimers(async () => {
      await renderWindow();
      const provider = document.querySelector<HTMLElement>(
        '[data-provider-id="zai"]',
      )!;

      await click(provider);
      const pinned = card()!;
      expect(pinned.getAttribute("data-provider")).toBe("zai");

      // The pointer leaves the provider, crosses the card, then leaves it:
      // none of that closes a pinned card.
      await hoverOut(provider, pinned);
      await hoverIn(pinned, provider);
      await hoverOut(pinned);
      await advance(400);
      expect(card()).toBe(pinned);
    }),
  );

  it(
    "closes the pinned card on a second click of the same provider",
    withFakeTimers(async () => {
      await renderWindow();
      const provider = document.querySelector<HTMLElement>(
        '[data-provider-id="zai"]',
      )!;

      await click(provider);
      expect(card()).not.toBeNull();
      await click(provider);
      // The close fades briefly instead of popping the card away.
      expect(card()).not.toBeNull();
      expect(card()!.className).toContain("is-exiting");
      await advance(200);
      expect(card()).toBeNull();
    }),
  );

  it("switches the pinned card to another provider on click", async () => {
    await renderWindow();
    await click(document.querySelector('[data-provider-id="zai"]')!);
    expect(card()!.getAttribute("data-provider")).toBe("zai");

    await click(document.querySelector('[data-provider-id="grok"]')!);
    const switched = card()!;
    expect(switched).not.toBeNull();
    expect(switched.getAttribute("data-provider")).toBe("grok");
    expect(switched.getAttribute("aria-label")).toBe("Grok");
    // A switch keeps the card mounted — it glides and crossfades, no exit.
    expect(switched.className).not.toContain("is-exiting");
  });

  it(
    "does not move or close the pinned card when the pointer hovers elsewhere",
    withFakeTimers(async () => {
      await renderWindow();
      const zai = document.querySelector<HTMLElement>('[data-provider-id="zai"]')!;
      const grok = document.querySelector<HTMLElement>(
        '[data-provider-id="grok"]',
      )!;

      await click(zai);
      await hoverIn(grok);
      expect(card()!.getAttribute("data-provider")).toBe("zai");
      await hoverOut(grok);
      await advance(400);
      expect(card()!.getAttribute("data-provider")).toBe("zai");
    }),
  );

  it(
    "closes the pinned card on an outside mousedown",
    withFakeTimers(async () => {
      await renderWindow();
      await click(document.querySelector('[data-provider-id="zai"]')!);
      expect(card()).not.toBeNull();

      await act(async () => {
        document
          .querySelector(".fq-root")!
          .dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
      });
      expect(card()).not.toBeNull();
      expect(card()!.className).toContain("is-exiting");
      await advance(200);
      expect(card()).toBeNull();
    }),
  );

  it("keeps the bar visible when a provider is clicked", async () => {
    await renderWindow();
    mocks.resizeCalls.length = 0;
    await click(document.querySelector('[data-provider-id="zai"]')!);

    // Every detail-open resize still carries the full 64px pill band — the
    // card is reserved above it by the measured height, never instead of it.
    const detailCalls = mocks.resizeCalls.filter(([, , overlay]) => overlay);
    expect(detailCalls.length).toBeGreaterThan(0);
    for (const [, height] of detailCalls) {
      expect(height).toBeGreaterThanOrEqual(64 + 292);
    }

    // Composition: the pill and the card live in the same document, pill on
    // top, and nothing scrolled the pill out of the viewport.
    const shell = document.querySelector(".fq-shell")!;
    const card = document.querySelector(".fq-popover")!;
    expect(
      card.compareDocumentPosition(shell) & Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy();
    expect(document.documentElement.scrollTop).toBe(0);
    expect(document.body.scrollTop).toBe(0);
  });

  it("pins the card without focus-scrolling the pill out of view", async () => {
    const focusSpy = vi.spyOn(HTMLButtonElement.prototype, "focus");
    try {
      await renderWindow();
      await click(document.querySelector('[data-provider-id="zai"]')!);
      expect(focusSpy).toHaveBeenCalledWith({ preventScroll: true });
      expect(document.activeElement).toBe(
        document.querySelector('[role="dialog"] button'),
      );
    } finally {
      focusSpy.mockRestore();
    }
  });

  it(
    "opens the pinned card on click, focuses Open, and Escape returns focus to the provider",
    withFakeTimers(async () => {
      await renderWindow();
      const provider = document.querySelector<HTMLElement>(
        '[data-provider-id="zai"]',
      );
      expect(provider).not.toBeNull();

      await click(provider!);
      const dialog = document.querySelector('[role="dialog"]');
      expect(dialog).not.toBeNull();
      expect(document.querySelector('[role="dialog"] button')!.textContent).toBe(
        "Open",
      );
      expect(document.activeElement).toBe(
        dialog!.querySelector("button"),
      );

      await act(async () => {
        window.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
        );
      });
      // Focus returns to the provider immediately; the card itself fades out.
      expect(document.activeElement).toBe(provider);
      await advance(200);
      expect(document.querySelector('[role="dialog"]')).toBeNull();
    }),
  );

  it("keeps the unavailable provider on the bar while the healthy one is open", async () => {
    await renderWindow();
    expect(document.body.textContent).toContain("—");
    await click(
      document.querySelector<HTMLElement>('[data-provider-id="zai"]')!,
    );
    expect(document.querySelector('[role="dialog"]')!.textContent).toContain(
      "Weekly",
    );
  });
});

describe("FloatingQuotaWindow provider preferences", () => {
  afterEach(() => {
    mocks.prefs.order = [];
    mocks.prefs.hidden = [];
  });

  it("removes hidden providers from the glanceable bar", async () => {
    mocks.prefs.hidden = ["zai"];
    await renderWindow();
    expect(document.querySelector('[data-provider-id="zai"]')).toBeNull();
    expect(document.querySelector('[data-provider-id="grok"]')).not.toBeNull();
  });

  it("orders the bar by the saved preference order", async () => {
    mocks.prefs.order = ["grok", "zai"];
    await renderWindow();
    const ids = Array.from(document.querySelectorAll("[data-provider-id]")).map(
      (element) => element.getAttribute("data-provider-id"),
    );
    expect(ids).toEqual(["grok", "zai"]);
  });
});

describe("FloatingQuotaWindow resting dock", () => {
  function dockStrip() {
    return document.querySelector<HTMLElement>(".fq-dock-strip");
  }

  function dockPref() {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top" }),
    );
  }

  it("shows a snap preview without leaving the floating dock state", async () => {
    await renderWindow();
    expect(document.querySelector(".fq-root")!.hasAttribute("data-dock-gesture")).toBe(false);
    await act(async () => {
      mocks.onGesturePreview?.("snap-ready");
    });
    expect(document.querySelector(".fq-root")!.getAttribute("data-dock-gesture")).toBe(
      "snap-ready",
    );
    expect(document.querySelector(".fq-root")!.getAttribute("data-dock-state")).toBe("none");
    await act(async () => {
      mocks.onGesturePreview?.(null);
    });
    expect(document.querySelector(".fq-root")!.hasAttribute("data-dock-gesture")).toBe(false);
  });

  function menuButton(label: string) {
    return Array.from(document.querySelectorAll(".fq-menu button")).find(
      (button) => button.textContent === label,
    );
  }

  async function openContextMenu() {
    await act(async () => {
      document
        .querySelector(".fq-root")!
        .dispatchEvent(
          new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
        );
    });
  }

  it(
    "rests 600×16 on startup when the dock pref is top, meters only",
    async () => {
      dockPref();
      await renderWindow();

      // Startup restore: the first native resize is the resting strip.
      expect(mocks.resizeCalls[0]).toEqual([600, 16, false, true]);
      expect(document.querySelector(".fq-root")!.getAttribute("data-dock-state")).toBe(
        "resting",
      );
      // The fixture shows two providers; the strip mirrors them with meters
      // on the computed dock geometry and nothing else — no readouts, no
      // marks, no labels.
      const meters = Array.from(
        dockStrip()!.querySelectorAll<HTMLElement>(".fq-dock-meter"),
      );
      expect(meters).toHaveLength(2);
      const rects = dockMeterRects(2);
      expect(meters[0].style.left).toBe(`${rects[0].left}px`);
      expect(meters[0].style.width).toBe(`${rects[0].width}px`);
      expect(meters[1].style.left).toBe(`${rects[1].left}px`);
      expect(dockStrip()!.querySelector(".fq-percent, .fq-mark, .fq-readout")).toBeNull();
    },
  );

  it(
    "reveals the production bar on hover and re-rests after the pointer leaves",
    withFakeTimers(async () => {
      dockPref();
      await renderWindow();
      mocks.resizeCalls.length = 0;

      await hoverIn(dockStrip()!);
      // One native resize to the production size (fixture: 2 providers →
      // 300×64), then content motion only: the resting surface fades out
      // over the bar for the whole reveal window.
      expect(mocks.resizeCalls.at(-1)).toEqual([300, 64, false, true]);
      expect(dockStrip()!.className).toContain("is-exiting");
      expect(document.querySelector(".fq-shell")).not.toBeNull();
      expect(
        document.querySelector(".fq-root")!.getAttribute("data-dock-state"),
      ).toBe("revealing");
      await advance(DOCK_REVEAL_MS);
      expect(dockStrip()).toBeNull();
      expect(
        document.querySelector(".fq-root")!.getAttribute("data-dock-state"),
      ).toBe("revealed");

      await hoverOut(document.querySelector(".fq-root")!);
      await advance(150);
      expect(mocks.resizeCalls.at(-1)).toEqual([600, 16, false, true]);
      expect(dockStrip()).not.toBeNull();
      expect(
        document.querySelector(".fq-root")!.getAttribute("data-dock-state"),
      ).toBe("resting");
    }),
  );

  it(
    "settles straight back to a full-strength strip when the pointer leaves mid-reveal",
    withFakeTimers(async () => {
      dockPref();
      await renderWindow();

      // A graze: the pointer leaves well inside the reveal window, so the
      // leave grace fires while the strip is still fading out. The settle
      // must cancel that exit — the resting surface is back immediately,
      // not left fading (or held invisible by the exit's fill) until the
      // reveal timer runs out.
      await hoverIn(dockStrip()!);
      await hoverOut(document.querySelector(".fq-root")!);
      await advance(150);
      expect(
        document.querySelector(".fq-root")!.getAttribute("data-dock-state"),
      ).toBe("resting");
      expect(dockStrip()).not.toBeNull();
      expect(dockStrip()!.className).not.toContain("is-exiting");

      // The cancelled reveal timer must not come back to bite: the dock
      // stays resting with the strip mounted.
      await advance(DOCK_REVEAL_MS);
      expect(
        document.querySelector(".fq-root")!.getAttribute("data-dock-state"),
      ).toBe("resting");
      expect(dockStrip()).not.toBeNull();
      expect(dockStrip()!.className).not.toContain("is-exiting");
    }),
  );

  it(
    "keeps the dock revealed while a pinned card holds the bar open",
    withFakeTimers(async () => {
      dockPref();
      await renderWindow();
      mocks.resizeCalls.length = 0;

      await hoverIn(dockStrip()!);
      await advance(DOCK_REVEAL_MS);
      await click(document.querySelector('[data-provider-id="zai"]')!);
      expect(card()).not.toBeNull();

      // The pointer leaves the window entirely: the pinned card keeps the
      // bar open, and the dock must not snap back to the strip under it.
      await hoverOut(document.querySelector(".fq-root")!);
      await advance(400);
      expect(card()).not.toBeNull();
      expect(mocks.resizeCalls.some(([width, height]) => width === 600 && height === 16)).toBe(
        false,
      );

      // Escape closes the pinned card; with the pointer gone the dock then
      // settles back into the strip through the same close path. The close
      // fade commits first, and the leave grace starts only after it.
      await act(async () => {
        window.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
        );
      });
      await advance(200);
      await advance(200);
      expect(mocks.resizeCalls.at(-1)).toEqual([600, 16, false, true]);
      expect(dockStrip()).not.toBeNull();
    }),
  );

  it(
    "undocks from the menu action and returns to the free production bar",
    withFakeTimers(async () => {
      dockPref();
      await renderWindow();
      await hoverIn(dockStrip()!);
      await advance(DOCK_REVEAL_MS);
      mocks.resizeCalls.length = 0;

      await openContextMenu();
      const undock = menuButton("Undock from top edge");
      expect(undock).toBeDefined();
      await click(undock!);

      // Free bar again: production width, no dock anchoring, strip gone.
      expect(mocks.resizeCalls.at(-1)).toEqual([300, 64, false, false]);
      expect(document.querySelector(".fq-menu")).toBeNull();
      expect(dockStrip()).toBeNull();
      expect(
        document.querySelector(".fq-root")!.getAttribute("data-dock-state"),
      ).toBe("none");
    }),
  );

  it(
    "docks from the menu action and settles into the resting strip",
    withFakeTimers(async () => {
      await renderWindow();
      mocks.resizeCalls.length = 0;

      await openContextMenu();
      const dock = menuButton("Dock to top edge");
      expect(dock).toBeDefined();
      await click(dock!);

      // Docking dismisses the menu: top-anchored at the production size —
      // 300 wide for the fixture's two visible providers, only the resting
      // strip is fixed at 600.
      expect(mocks.resizeCalls.at(-1)).toEqual([300, 64, false, true]);
      expect(document.querySelector(".fq-menu")).toBeNull();
      // No outside click is needed for the dock to settle.
      await advance(150);
      expect(mocks.resizeCalls.at(-1)).toEqual([600, 16, false, true]);
      expect(dockStrip()).not.toBeNull();
    }),
  );

  it("follows a drag-away undock reported by the chrome", async () => {
    dockPref();
    await renderWindow();
    expect(mocks.resizeCalls.at(-1)).toEqual([600, 16, false, true]);

    await act(async () => {
      mocks.onDockChange?.("none");
    });
    expect(mocks.resizeCalls.at(-1)).toEqual([300, 64, false, false]);
    expect(dockStrip()).toBeNull();
  });
});
