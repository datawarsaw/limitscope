// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * W01 main-window bounds: the clamp math itself is covered in
 * windowBounds.test.ts. These tests pin the application path — the physical
 * px conversion including the decoration delta, the primary-monitor
 * fallback, and the attach listeners (scale change, resize, focus regain)
 * that re-check bounds after DPI journeys and monitor removal.
 */

const mocks = vi.hoisted(() => {
  type Size = { width: number; height: number };
  type Pos = { x: number; y: number };
  let current: { workArea: { position: Pos; size: Size } } | null;
  let primary: { workArea: { position: Pos; size: Size } } | null;
  let currentWindow: Record<string, unknown>;
  const setSize = vi.fn(
    async (_size?: { width: number; height: number }) => undefined,
  );
  const setPosition = vi.fn(
    async (_position?: { x: number; y: number }) => undefined,
  );
  const listeners = new Map<string, (event: unknown) => void>();
  const subscribe = (name: string) =>
    vi.fn(async (handler: (event: unknown) => void) => {
      listeners.set(name, handler);
      return () => listeners.delete(name);
    });
  const reset = (windowState: Record<string, unknown>) => {
    current = null;
    primary = null;
    listeners.clear();
    setSize.mockClear();
    setPosition.mockClear();
    currentWindow = windowState;
  };
  return {
    PhysicalSize: class {
      constructor(public width: number, public height: number) {}
    },
    PhysicalPosition: class {
      constructor(public x: number, public y: number) {}
    },
    getCurrentWindow: vi.fn(() => currentWindow),
    currentMonitor: vi.fn(async () => current),
    primaryMonitor: vi.fn(async () => primary),
    setMonitors: (c: typeof current, p: typeof primary) => {
      current = c;
      primary = p;
    },
    setSize,
    setPosition,
    listeners,
    subscribe,
    reset,
  };
});

vi.mock("@tauri-apps/api/window", () => ({
  PhysicalSize: mocks.PhysicalSize,
  PhysicalPosition: mocks.PhysicalPosition,
  getCurrentWindow: mocks.getCurrentWindow,
  currentMonitor: mocks.currentMonitor,
  primaryMonitor: mocks.primaryMonitor,
}));

import {
  attachMainWindowBounds,
  clampMainWindowToWorkArea,
  type ClampableWindow,
} from "./mainWindowBounds";

const AREA_1080P = {
  workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1032 } },
};
const AREA_200_DPI = {
  // 200% on 1080p: the physical work area is half the logical resolution.
  workArea: { position: { x: 0, y: 0 }, size: { width: 960, height: 516 } },
};

function fakeWindow(outer: { x: number; y: number; width: number; height: number }) {
  const win = {
    outerPosition: async () => ({ x: outer.x, y: outer.y }),
    outerSize: async () => ({ width: outer.width, height: outer.height }),
    // A decorated window's client area is smaller than its outer bounds.
    innerSize: async () => ({ width: outer.width, height: outer.height - 32 }),
    setSize: mocks.setSize,
    setPosition: mocks.setPosition,
  };
  return win as unknown as ClampableWindow;
}

beforeEach(() => {
  mocks.reset({});
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
});

afterEach(() => {
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

describe("clampMainWindowToWorkArea", () => {
  it("leaves a placement that fits the work area untouched", async () => {
    mocks.setMonitors(AREA_1080P, AREA_1080P);
    await clampMainWindowToWorkArea(
      fakeWindow({ x: 120, y: 0, width: 1400, height: 1016 }),
    );
    expect(mocks.setSize).not.toHaveBeenCalled();
    expect(mocks.setPosition).not.toHaveBeenCalled();
  });

  it("shrinks the 200% default so the outer bounds fit the physical work area", async () => {
    // 700×760 logical at 200% is 1400×1520 physical — both axes exceed the
    // 960×516 physical work area, so both clamp, and the size call carries
    // the 32px decoration delta on the height.
    mocks.setMonitors(AREA_200_DPI, AREA_200_DPI);
    await clampMainWindowToWorkArea(
      fakeWindow({ x: 100, y: 0, width: 1400, height: 1520 }),
    );
    expect(mocks.setSize).toHaveBeenCalledTimes(1);
    expect(mocks.setSize.mock.calls[0][0]).toEqual({
      width: 960,
      height: 516 - 32,
    });
    expect(mocks.setPosition.mock.calls[0][0]).toEqual({ x: 0, y: 0 });
  });

  it("repositions a window stranded beyond the work area", async () => {
    mocks.setMonitors(AREA_1080P, AREA_1080P);
    await clampMainWindowToWorkArea(
      fakeWindow({ x: 4000, y: 0, width: 1400, height: 1016 }),
    );
    expect(mocks.setPosition.mock.calls[0][0]).toEqual({ x: 520, y: 0 });
    // Size re-applied in client px: outer intent minus the decoration delta.
    expect(mocks.setSize.mock.calls[0][0]).toEqual({
      width: 1400,
      height: 1016 - 32,
    });
  });

  it("falls back to the primary monitor when the current one is gone", async () => {
    mocks.setMonitors(null, AREA_1080P);
    await clampMainWindowToWorkArea(
      fakeWindow({ x: 100, y: 0, width: 1400, height: 1520 }),
    );
    expect(mocks.setSize.mock.calls[0][0]).toEqual({
      width: 1400,
      height: 1032 - 32,
    });
    expect(mocks.setPosition.mock.calls[0][0]).toEqual({ x: 100, y: 0 });
  });

  it("does nothing when no monitor is available", async () => {
    mocks.setMonitors(null, null);
    await clampMainWindowToWorkArea(
      fakeWindow({ x: 0, y: 0, width: 1400, height: 1520 }),
    );
    expect(mocks.setSize).not.toHaveBeenCalled();
    expect(mocks.setPosition).not.toHaveBeenCalled();
  });
});

describe("attachMainWindowBounds", () => {
  it("is inert outside the Tauri runtime", () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    const cleanup = attachMainWindowBounds();
    expect(typeof cleanup).toBe("function");
    expect(mocks.getCurrentWindow).not.toHaveBeenCalled();
    cleanup();
  });

  it("applies the clamp at attach and re-checks on events until cleanup", async () => {
    const onScaleChanged = mocks.subscribe("scale");
    const onResized = mocks.subscribe("resized");
    const onFocusChanged = mocks.subscribe("focus");
    mocks.reset({
      outerPosition: async () => ({ x: 0, y: 0 }),
      outerSize: async () => ({ width: 1400, height: 1520 }),
      innerSize: async () => ({ width: 1400, height: 1488 }),
      setSize: mocks.setSize,
      setPosition: mocks.setPosition,
      onScaleChanged,
      onResized,
      onFocusChanged,
    });
    mocks.setMonitors(AREA_200_DPI, AREA_200_DPI);

    const cleanup = attachMainWindowBounds();
    await vi.waitFor(() => expect(mocks.setSize).toHaveBeenCalledTimes(1));

    // DPI change and focus regain each re-run the clamp; a focus loss does not.
    mocks.setSize.mockClear();
    mocks.listeners.get("scale")?.(undefined);
    await vi.waitFor(() => expect(mocks.setSize).toHaveBeenCalledTimes(1));
    mocks.setSize.mockClear();
    mocks.listeners.get("focus")?.({ payload: false });
    mocks.listeners.get("focus")?.({ payload: true });
    await vi.waitFor(() => expect(mocks.setSize).toHaveBeenCalledTimes(1));
    mocks.setSize.mockClear();

    cleanup();
    await vi.waitFor(() => expect(mocks.listeners.size).toBe(0));
    expect(onScaleChanged).toHaveBeenCalledTimes(1);
    expect(onResized).toHaveBeenCalledTimes(1);
    expect(onFocusChanged).toHaveBeenCalledTimes(1);
    expect(mocks.setSize).not.toHaveBeenCalled();
  });

  it("subscribes and unsubscribes exactly once per listener kind", async () => {
    const onScaleChanged = mocks.subscribe("scale");
    const onResized = mocks.subscribe("resized");
    const onFocusChanged = mocks.subscribe("focus");
    mocks.reset({
      outerPosition: async () => ({ x: 0, y: 0 }),
      outerSize: async () => ({ width: 1400, height: 1016 }),
      innerSize: async () => ({ width: 1400, height: 984 }),
      setSize: mocks.setSize,
      setPosition: mocks.setPosition,
      onScaleChanged,
      onResized,
      onFocusChanged,
    });
    mocks.setMonitors(AREA_1080P, AREA_1080P);

    const cleanup = attachMainWindowBounds();
    await vi.waitFor(() => expect(mocks.listeners.size).toBe(3));
    // A fitting placement still registers listeners but never resizes.
    expect(mocks.setSize).not.toHaveBeenCalled();
    cleanup();
    await vi.waitFor(() => expect(mocks.listeners.size).toBe(0));
  });
});
