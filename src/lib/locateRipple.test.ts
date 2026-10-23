// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  LOCATE_RIPPLE_MIN_STAGE,
  LOCATE_RIPPLE_RING_MS,
  LOCATE_RIPPLE_RING_STAGGER_MS,
  LOCATE_RIPPLE_TOTAL_MS,
  LOCATE_RIPPLE_URL,
  LOCATE_RIPPLE_WINDOW_PREFIX,
  closeLocateRipple,
  prefersReducedMotion,
  rippleOverlayGeometry,
  spawnLocateRipple,
} from "./locateRipple";

/**
 * The ripple overlay is the only window the floating webview ever creates,
 * so its lifecycle is pinned here: click-through and focusless on screen,
 * replaced (never stacked) by a repeated locate, destroyed by its own timer,
 * and inert when the user asked for reduced motion.
 */

const mocks = vi.hoisted(() => {
  class FakeOverlay {
    static instances: FakeOverlay[] = [];
    static autoCreated = true;
    static failOn: string | null = null;
    label: string;
    options: Record<string, unknown>;
    destroyed = false;
    size: { width: number; height: number } | null = null;
    position: { x: number; y: number } | null = null;
    ignoreCursor: boolean | null = null;
    focusable: boolean | null = null;
    shown = false;
    private handlers = new Map<string, Array<() => void>>();
    constructor(label: string, options: Record<string, unknown>) {
      this.label = label;
      this.options = options;
      FakeOverlay.instances.push(this);
      if (FakeOverlay.autoCreated) {
        queueMicrotask(() => this.emit("tauri://created"));
      }
    }
    emit(event: string): void {
      for (const handler of this.handlers.get(event) ?? []) handler();
    }
    once(event: string, handler: () => void): Promise<() => void> {
      const list = this.handlers.get(event) ?? [];
      list.push(handler);
      this.handlers.set(event, list);
      return Promise.resolve(() => {});
    }
    async setFocusable(value: boolean): Promise<void> {
      if (FakeOverlay.failOn === "setFocusable") throw new Error("denied");
      this.focusable = value;
    }
    async setSize(size: { width: number; height: number }): Promise<void> {
      if (FakeOverlay.failOn === "setSize") throw new Error("denied");
      this.size = { width: size.width, height: size.height };
    }
    async setPosition(position: { x: number; y: number }): Promise<void> {
      if (FakeOverlay.failOn === "setPosition") throw new Error("denied");
      this.position = { x: position.x, y: position.y };
    }
    async setIgnoreCursorEvents(value: boolean): Promise<void> {
      if (FakeOverlay.failOn === "setIgnoreCursorEvents") throw new Error("denied");
      this.ignoreCursor = value;
    }
    async show(): Promise<void> {
      if (FakeOverlay.failOn === "show") throw new Error("denied");
      this.shown = true;
    }
    async destroy(): Promise<void> {
      this.destroyed = true;
      this.emit("tauri://destroyed");
    }
  }
  return { FakeOverlay };
});

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: mocks.FakeOverlay,
}));

let reducedMotion = false;

beforeEach(() => {
  mocks.FakeOverlay.instances.length = 0;
  mocks.FakeOverlay.autoCreated = true;
  mocks.FakeOverlay.failOn = null;
  reducedMotion = false;
  window.matchMedia = ((query: string) =>
    ({ matches: reducedMotion, media: query })) as unknown as typeof window.matchMedia;
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("locate ripple geometry", () => {
  it("centers the overlay on the bar's center, in physical px", () => {
    const bar = { x: 100, y: 200, width: 520, height: 64 };
    const overlay = rippleOverlayGeometry(bar, 1);
    expect(Math.abs(overlay.x + overlay.width / 2 - (bar.x + bar.width / 2))).toBeLessThanOrEqual(1);
    expect(Math.abs(overlay.y + overlay.height / 2 - (bar.y + bar.height / 2))).toBeLessThanOrEqual(1);
  });

  it("keeps the physical stage size across monitor scale factors", () => {
    const bar = { x: 100, y: 200, width: 520, height: 64 };
    const one = rippleOverlayGeometry(bar, 1);
    const hidpi = rippleOverlayGeometry(bar, 2);
    // The bar occupies the same physical pixels on either monitor, so the
    // waves must span the same physical extent; only the logical size that
    // produces it changes.
    expect(Math.abs(hidpi.width - one.width)).toBeLessThanOrEqual(2);
    expect(Math.abs(hidpi.height - one.height)).toBeLessThanOrEqual(2);
    expect(Math.abs(hidpi.x + hidpi.width / 2 - (one.x + one.width / 2))).toBeLessThanOrEqual(1);
  });

  it("gives the docked resting strip a full round stage", () => {
    const strip = rippleOverlayGeometry(
      { x: 400, y: 0, width: 520, height: 16 },
      1,
    );
    expect(strip.height).toBeGreaterThanOrEqual(LOCATE_RIPPLE_MIN_STAGE);
  });

  it("caps the wave reach for a very wide bar", () => {
    const wide = rippleOverlayGeometry(
      { x: 0, y: 0, width: 1600, height: 64 },
      1,
    );
    // reach caps at 1000 logical px, so the stage stays bounded: bar + 2×480.
    expect(wide.width).toBe(1600 + 960);
  });

  it("treats a missing scale factor as 1", () => {
    const bar = { x: 0, y: 0, width: 520, height: 64 };
    const fallback = rippleOverlayGeometry(bar, Number.NaN);
    const one = rippleOverlayGeometry(bar, 1);
    expect(fallback).toEqual(one);
  });
});

describe("spawnLocateRipple lifecycle", () => {
  it("creates one click-through, focusless, topmost overlay and destroys it after the last crest", async () => {
    const spawn = spawnLocateRipple({ x: 80, y: 40, width: 520, height: 64 }, 1);
    const shown = await spawn;
    expect(shown).toBe(true);
    const [overlay] = mocks.FakeOverlay.instances;
    expect(overlay.label).toMatch(new RegExp(`^${LOCATE_RIPPLE_WINDOW_PREFIX}-\\d+$`));
    expect(overlay.options.url).toBe(LOCATE_RIPPLE_URL);
    expect(overlay.options.visible).toBe(false);
    expect(overlay.options.transparent).toBe(true);
    expect(overlay.options.focus).toBe(false);
    expect(overlay.options.alwaysOnTop).toBe(true);
    expect(overlay.options.decorations).toBe(false);
    expect(overlay.options.skipTaskbar).toBe(true);
    expect(overlay.focusable).toBe(false);
    expect(overlay.ignoreCursor).toBe(true);
    expect(overlay.shown).toBe(true);
    // The stage is positioned from the bar's physical rect, centered on it.
    const bounds = rippleOverlayGeometry({ x: 80, y: 40, width: 520, height: 64 }, 1);
    expect(overlay.size).toEqual({ width: bounds.width, height: bounds.height });
    expect(overlay.position).toEqual({ x: bounds.x, y: bounds.y });
    // Nothing is destroyed while the waves play.
    await vi.advanceTimersByTimeAsync(LOCATE_RIPPLE_TOTAL_MS - 1);
    expect(overlay.destroyed).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(overlay.destroyed).toBe(true);
  });

  it("replaces a running ripple instead of stacking windows", async () => {
    await spawnLocateRipple({ x: 0, y: 0, width: 520, height: 64 }, 1);
    const first = mocks.FakeOverlay.instances[0];
    await spawnLocateRipple({ x: 0, y: 0, width: 520, height: 64 }, 1);
    expect(mocks.FakeOverlay.instances).toHaveLength(2);
    expect(first.destroyed).toBe(true);
    expect(mocks.FakeOverlay.instances[1].destroyed).toBe(false);
  });

  it("destroys everything and reports failure when the overlay cannot be staged", async () => {
    mocks.FakeOverlay.failOn = "setSize";
    const shown = await spawnLocateRipple({ x: 0, y: 0, width: 520, height: 64 }, 1);
    expect(shown).toBe(false);
    expect(mocks.FakeOverlay.instances[0].destroyed).toBe(true);
  });

  it("destroys a window whose creation wedges instead of holding the locate flow", async () => {
    mocks.FakeOverlay.autoCreated = false;
    const spawn = spawnLocateRipple({ x: 0, y: 0, width: 520, height: 64 }, 1);
    await vi.advanceTimersByTimeAsync(1500);
    const shown = await spawn;
    expect(shown).toBe(false);
    // The late created event must not resurrect the stale session: the
    // window is destroyed on the timeout and stays destroyed.
    expect(mocks.FakeOverlay.instances[0].destroyed).toBe(true);
  });

  it("skips the overlay entirely under prefers-reduced-motion", async () => {
    reducedMotion = true;
    expect(prefersReducedMotion()).toBe(true);
    const shown = await spawnLocateRipple({ x: 0, y: 0, width: 520, height: 64 }, 1);
    expect(shown).toBe(false);
    expect(mocks.FakeOverlay.instances).toHaveLength(0);
  });

  it("clears the session when the window dies, so a later close is a no-op", async () => {
    await spawnLocateRipple({ x: 0, y: 0, width: 520, height: 64 }, 1);
    const overlay = mocks.FakeOverlay.instances[0];
    const destroy = vi.spyOn(overlay, "destroy");
    await overlay.destroy();
    closeLocateRipple();
    expect(destroy).toHaveBeenCalledTimes(1);
  });

  it("stops the close timer when the ripple is closed early", async () => {
    await spawnLocateRipple({ x: 0, y: 0, width: 520, height: 64 }, 1);
    const overlay = mocks.FakeOverlay.instances[0];
    closeLocateRipple();
    expect(overlay.destroyed).toBe(true);
    await vi.advanceTimersByTimeAsync(LOCATE_RIPPLE_TOTAL_MS);
    expect(mocks.FakeOverlay.instances).toHaveLength(1);
  });

  it("paces three staggered crests inside the total window", () => {
    const lastCrest =
      LOCATE_RIPPLE_RING_STAGGER_MS * 2 + LOCATE_RIPPLE_RING_MS;
    expect(LOCATE_RIPPLE_TOTAL_MS).toBeGreaterThanOrEqual(lastCrest);
  });
});
