// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { availableMonitors, currentMonitor } from "@tauri-apps/api/window";

/**
 * The floating window's Refresh must reach the shared Rust runtime through
 * the same `request_refresh` command the main window and the tray use —
 * never by re-emitting the old per-webview `tray://refresh` event.
 *
 * v0.8 Lane C additions pinned here: the tray Show/Hide listener pair with
 * visibility announcements (A10), the conditional main-window focus restore
 * on self-dismissal (A09), and the both-axis overlay containment (A11).
 */

const mocks = vi.hoisted(() => {
  const state = {
    listeners: new Map<string, (event: unknown) => void>(),
    emitted: [] as Array<{ event: string; payload: unknown }>,
    position: { x: 80, y: 40 },
    movedHandler: undefined as
      | undefined
      | ((event: { payload: { x: number; y: number } }) => void),
    monitor: null as null | {
      scaleFactor: number;
      workArea: { position: { x: number; y: number }; size: { width: number; height: number } };
    },
    mainVisible: true,
    mainWindowExists: true,
  };
  const listen = vi.fn(async (event: string, handler: (event: unknown) => void) => {
    state.listeners.set(event, handler);
    return () => state.listeners.delete(event);
  });
  const emit = vi.fn(async (event: string, payload?: unknown) => {
    state.emitted.push({ event, payload });
  });
  const invoke = vi.fn<(command: string, args?: unknown) => Promise<unknown>>(
    async () => undefined,
  );
  const winFns = {
    hide: vi.fn(async () => undefined),
    show: vi.fn(async () => undefined),
    unminimize: vi.fn(async () => undefined),
    setAlwaysOnTop: vi.fn(async () => undefined),
    setPosition: vi.fn(async (position: { x: number; y: number }) => {
      state.position = { x: position.x, y: position.y };
    }),
    setSize: vi.fn(async () => undefined),
    outerPosition: vi.fn(async () => ({ ...state.position })),
    outerSize: vi.fn(async () => ({ width: 520, height: 64 })),
    setMinSize: vi.fn(async () => undefined),
    onFocusChanged: vi.fn(async (handler: (event: { payload: boolean }) => void) => {
      state.listeners.set("focus-changed", handler as (event: unknown) => void);
      return () => state.listeners.delete("focus-changed");
    }),
    onMoved: vi.fn(
      async (handler: (event: { payload: { x: number; y: number } }) => void) => {
        state.movedHandler = handler;
        return () => {
          state.movedHandler = undefined;
        };
      },
    ),
    onCloseRequested: vi.fn(async (handler: () => void) => {
      state.listeners.set("close-requested", handler);
      return () => state.listeners.delete("close-requested");
    }),
  };
  const mainFns = {
    isVisible: vi.fn(async () => state.mainVisible),
    setFocus: vi.fn(async () => undefined),
  };
  const webviewWindowMock = {
    getByLabel: vi.fn(async (label: string) =>
      label === "main" && state.mainWindowExists ? mainFns : null,
    ),
  };
  return {
    state,
    listen,
    emit,
    invoke,
    winFns,
    mainFns,
    WebviewWindow: webviewWindowMock,
    reset: () => {
      state.listeners.clear();
      state.emitted.length = 0;
      state.position = { x: 80, y: 40 };
      state.movedHandler = undefined;
      state.monitor = null;
      state.mainVisible = true;
      state.mainWindowExists = true;
      for (const fn of [
        ...Object.values(winFns),
        ...Object.values(mainFns),
        listen,
        emit,
        invoke,
        webviewWindowMock.getByLabel,
      ]) {
        (fn as ReturnType<typeof vi.fn>).mockClear();
      }
    },
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
  emit: mocks.emit,
}));
vi.mock("@tauri-apps/api/window", () => ({
  availableMonitors: vi.fn(async () => []),
  currentMonitor: vi.fn(async () => mocks.state.monitor),
  primaryMonitor: vi.fn(async () => null),
  getCurrentWindow: vi.fn(() => mocks.winFns),
  LogicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
  PhysicalPosition: class {
    constructor(
      public x: number,
      public y: number,
    ) {}
  },
  PhysicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: mocks.WebviewWindow,
}));

import {
  FLOATING_VISIBILITY_EVENT,
  attachDockReanchor,
  attachFloatingChrome,
  dockAnchorCenterX,
  dockAnchorFromCenter,
  dockCenterForAnchor,
  hideFloatingWindow,
  requestGlobalRefresh,
  resizeFloatingWindow,
  setFloatingBarEnabled,
  setFloatingDock,
  setFloatingDockConstraints,
  shouldUndockFromTop,
} from "./floatingWindowChrome";
import {
  FLOATING_PREFS_STORAGE_KEY,
  loadFloatingQuotaPrefs,
  saveFloatingQuotaPrefs,
} from "./floatingWindowPrefs";
import { FLOATING_DRAG_LIFECYCLE_EVENT } from "./floatingDragLifecycle";

beforeEach(async () => {
  mocks.reset();
  mocks.invoke.mockImplementation(async (command: string) =>
    command === "attach_floating_drag_lifecycle" ? 0 : undefined,
  );
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  await setFloatingDock("top");
  await setFloatingDock("none");
});

afterEach(() => {
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  window.localStorage.clear();
});

describe("requestGlobalRefresh (floating window)", () => {
  it("requests a runtime refresh through the shared command", async () => {
    await requestGlobalRefresh();
    expect(mocks.invoke).toHaveBeenCalledWith("request_refresh");
  });

  it("is a no-op outside the Tauri runtime", async () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    await requestGlobalRefresh();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});

describe("setFloatingBarEnabled", () => {
  it("persists disable without erasing geometry or pin", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: false,
        floatingBarEnabled: true,
        alwaysOnTop: true,
        x: 320,
        y: 180,
      }),
    );

    await setFloatingBarEnabled(false);

    expect(loadFloatingQuotaPrefs()).toEqual({
      visible: false,
      floatingBarEnabled: false,
      alwaysOnTop: true,
      dock: "none",
      dockAnchor: null,
      x: 320,
      y: 180,
    });
  });

  it("re-enabling restores the prior geometry and pin", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({
        visible: false,
        floatingBarEnabled: false,
        alwaysOnTop: false,
        x: 320,
        y: 180,
      }),
    );

    await setFloatingBarEnabled(true);

    expect(loadFloatingQuotaPrefs()).toEqual({
      visible: false,
      floatingBarEnabled: true,
      alwaysOnTop: false,
      dock: "none",
      dockAnchor: null,
      x: 320,
      y: 180,
    });
  });
});

describe("hideFloatingWindow (A09/A10)", () => {
  it("hides, announces the hidden state, and restores focus to a visible main window", async () => {
    await hideFloatingWindow();

    expect(mocks.winFns.hide).toHaveBeenCalledTimes(1);
    expect(mocks.emit).toHaveBeenCalledWith(FLOATING_VISIBILITY_EVENT, false);
    expect(loadFloatingQuotaPrefs().visible).toBe(false);
    expect(mocks.mainFns.setFocus).toHaveBeenCalledTimes(1);
  });

  it("does not steal focus when the main window is hidden", async () => {
    mocks.state.mainVisible = false;

    await hideFloatingWindow();

    expect(mocks.winFns.hide).toHaveBeenCalledTimes(1);
    expect(mocks.emit).toHaveBeenCalledWith(FLOATING_VISIBILITY_EVENT, false);
    expect(mocks.mainFns.setFocus).not.toHaveBeenCalled();
  });

  it("survives a missing main window without hiding failure", async () => {
    mocks.state.mainWindowExists = false;

    await hideFloatingWindow();

    expect(mocks.winFns.hide).toHaveBeenCalledTimes(1);
    expect(mocks.mainFns.setFocus).not.toHaveBeenCalled();
  });

  it("updates only the persisted pref outside the Tauri runtime", async () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    await hideFloatingWindow();

    expect(loadFloatingQuotaPrefs().visible).toBe(false);
    expect(mocks.winFns.hide).not.toHaveBeenCalled();
    expect(mocks.emit).not.toHaveBeenCalled();
    expect(mocks.mainFns.setFocus).not.toHaveBeenCalled();
  });
});

describe("attachFloatingChrome tray symmetry (A10)", () => {
  it("announces the restored visibility once at attach", async () => {
    await attachFloatingChrome(() => false);

    expect(mocks.emit).toHaveBeenCalledWith(FLOATING_VISIBILITY_EVENT, true);
  });

  it("flips the pref and announces on tray show and tray hide events", async () => {
    const cleanup = await attachFloatingChrome(() => false);

    mocks.state.listeners.get("tray://show-floating")?.(undefined);
    await Promise.resolve();
    expect(loadFloatingQuotaPrefs().visible).toBe(true);
    expect(mocks.state.emitted).toContainEqual({
      event: FLOATING_VISIBILITY_EVENT,
      payload: true,
    });

    mocks.state.listeners.get("tray://hide-floating")?.(undefined);
    await Promise.resolve();
    expect(loadFloatingQuotaPrefs().visible).toBe(false);
    expect(mocks.state.emitted).toContainEqual({
      event: FLOATING_VISIBILITY_EVENT,
      payload: false,
    });

    cleanup();
    expect(mocks.state.listeners.size).toBe(0);
  });

  it("announces hidden on a close request", async () => {
    await attachFloatingChrome(() => false);

    mocks.state.listeners.get("close-requested")?.(undefined);
    await Promise.resolve();

    expect(loadFloatingQuotaPrefs().visible).toBe(false);
    expect(mocks.state.emitted).toContainEqual({
      event: FLOATING_VISIBILITY_EVENT,
      payload: false,
    });
  });

  it("does not announce outside the Tauri runtime", async () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    const cleanup = await attachFloatingChrome(() => false);
    cleanup();
    expect(mocks.emit).not.toHaveBeenCalled();
  });
});

describe("resizeFloatingWindow overlay containment (A11)", () => {
  const WORK_AREA_1080P = {
    scaleFactor: 1,
    workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1032 } },
  };

  it("shifts an overlay-parked bar up at the bottom edge, as before", async () => {
    mocks.state.monitor = WORK_AREA_1080P;
    mocks.state.position = { x: 100, y: 1000 };

    await resizeFloatingWindow(520, 64, true);

    expect(mocks.winFns.setPosition).toHaveBeenCalledWith(
      expect.objectContaining({ x: 100, y: 1032 - 8 - 64 }),
    );

    await resizeFloatingWindow(520, 64, false);
    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: 100, y: 1000 }),
    );
  });

  it("also contains the bar horizontally at the right edge", async () => {
    mocks.state.monitor = WORK_AREA_1080P;
    mocks.state.position = { x: 1800, y: 990 };

    await resizeFloatingWindow(520, 64, true);

    expect(mocks.winFns.setPosition).toHaveBeenCalledWith(
      expect.objectContaining({ x: 1920 - 520, y: 1032 - 8 - 64 }),
    );

    await resizeFloatingWindow(520, 64, false);
    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: 1800, y: 990 }),
    );
  });

  it("leaves a fully visible bar unmoved", async () => {
    mocks.state.monitor = WORK_AREA_1080P;
    mocks.state.position = { x: 100, y: 100 };

    await resizeFloatingWindow(520, 64, true);
    await resizeFloatingWindow(520, 64, false);

    expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
  });

  it("does nothing outside the Tauri runtime", async () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    await resizeFloatingWindow(520, 64, true);
    expect(mocks.winFns.setSize).not.toHaveBeenCalled();
  });
});

describe("resizeFloatingWindow docked (resting dock)", () => {
  beforeEach(async () => { await setFloatingDock("top"); });
  // Monitor 1 of the accepted setup: 3440×1440, taskbar at the top edge.
  const ULTRAWIDE = {
    scaleFactor: 1,
    workArea: { position: { x: 3520, y: 0 }, size: { width: 3440, height: 1400 } },
  };
  // Monitor 3: 1440×2560 portrait, negative work-area x.
  const PORTRAIT = {
    scaleFactor: 1,
    workArea: { position: { x: -1440, y: 0 }, size: { width: 1440, height: 2552 } },
  };
  // 150% scaling on the ultrawide: work-area coordinates stay physical.
  const ULTRAWIDE_150 = {
    scaleFactor: 1.5,
    workArea: { position: { x: 3520, y: 0 }, size: { width: 5160, height: 2100 } },
  };

  it("rests 600×16 anchored to the top center of the work area", async () => {
    mocks.state.monitor = ULTRAWIDE;

    await resizeFloatingWindow(600, 16, false, true);

    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(
      expect.objectContaining({ width: 600, height: 16 }),
    );
    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: 3520 + (3440 - 600) / 2, y: 0 }),
    );
  });

  it("keeps the top edge fixed and the horizontal center when revealing", async () => {
    mocks.state.monitor = ULTRAWIDE;

    await resizeFloatingWindow(600, 16, false, true);
    await resizeFloatingWindow(600, 64, false, true);

    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(
      expect.objectContaining({ width: 600, height: 64 }),
    );
    // The same anchor on both sizes: the window grows downward in place.
    expect(mocks.winFns.setPosition).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({ x: 3520 + 1420, y: 0 }),
    );
    expect(mocks.winFns.setPosition).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ x: 3520 + 1420, y: 0 }),
    );
  });

  it("centers a narrower revealed bar so docked meters keep their screen x", async () => {
    // Two visible providers reveal at 300 wide; the reveal re-centers.
    mocks.state.monitor = ULTRAWIDE;

    await resizeFloatingWindow(300, 64, false, true);

    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: 3520 + (3440 - 300) / 2, y: 0 }),
    );
  });

  it("stays correct on the portrait monitor and negative work-area x", async () => {
    mocks.state.monitor = PORTRAIT;

    await resizeFloatingWindow(600, 16, false, true);

    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: -1440 + (1440 - 600) / 2, y: 0 }),
    );
  });

  it("scales logical dock geometry by the monitor factor at 150%", async () => {
    mocks.state.monitor = ULTRAWIDE_150;

    await resizeFloatingWindow(600, 16, false, true);

    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(
      expect.objectContaining({ width: 900, height: 24 }),
    );
    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: 3520 + (5160 - 900) / 2, y: 0 }),
    );
  });

  it("clamps the whole window as one unit when the work area is too narrow", async () => {
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 500, height: 1600 } },
    };

    await resizeFloatingWindow(600, 16, false, true);

    // Shrunk as one unit and pulled inside; no separate resting-only width.
    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(
      expect.objectContaining({ width: 500, height: 16 }),
    );
    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: 0, y: 0 }),
    );
  });

  it("relaxes the min-size constraint only while docked", async () => {
    await setFloatingDockConstraints(true);
    expect(mocks.winFns.setMinSize).toHaveBeenLastCalledWith(null);

    await setFloatingDockConstraints(false);
    expect(mocks.winFns.setMinSize).toHaveBeenLastCalledWith(
      expect.objectContaining({ width: 300, height: 64 }),
    );
  });

  it("places the resting strip at a saved off-center anchor", async () => {
    mocks.state.monitor = ULTRAWIDE;
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", dockAnchor: 0.25 }),
    );

    await resizeFloatingWindow(600, 16, false, true);

    // anchor 0.25 → shared center = 3520 + 0.75 × 3440.
    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({
        x: 3520 + Math.round(0.75 * 3440) - 300,
        y: 0,
      }),
    );
  });

  it("rests and reveals on the same anchor center so the reveal never slides", async () => {
    mocks.state.monitor = ULTRAWIDE;
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", dockAnchor: 0.25 }),
    );

    await resizeFloatingWindow(600, 16, false, true);
    await resizeFloatingWindow(300, 64, false, true);

    const center = 3520 + 0.75 * 3440;
    expect(mocks.winFns.setPosition).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({ x: Math.round(center) - 300 }),
    );
    expect(mocks.winFns.setPosition).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ x: Math.round(center) - 150 }),
    );
  });

  it("re-anchors proportionally on another monitor and clamps at the edge", async () => {
    mocks.state.monitor = PORTRAIT;
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", dockAnchor: 0.25 }),
    );

    await resizeFloatingWindow(600, 16, false, true);

    // The same fraction of the portrait work area, not the same absolute x.
    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({
        x: -1440 + Math.round(0.75 * 1440) - 300,
        y: 0,
      }),
    );

    // An out-of-range stored anchor parses to 0.5 and clamps to the far
    // edge the strip still fits: its right edge sits on the work area's.
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", dockAnchor: 0.9 }),
    );
    await resizeFloatingWindow(600, 16, false, true);

    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({ x: -1440 + 1440 - 600, y: 0 }),
    );
  });

  it("keeps the anchor fraction across a 150% scale factor", async () => {
    mocks.state.monitor = ULTRAWIDE_150;
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", dockAnchor: 0.25 }),
    );

    await resizeFloatingWindow(600, 16, false, true);

    expect(mocks.winFns.setPosition).toHaveBeenLastCalledWith(
      expect.objectContaining({
        x: 3520 + Math.round(0.75 * 5160) - 450,
        y: 0,
      }),
    );
  });

  it("does nothing outside the Tauri runtime while docked", async () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    await resizeFloatingWindow(600, 16, false, true);
    expect(mocks.winFns.setSize).not.toHaveBeenCalled();
    expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
  });
});

describe("dock anchor math", () => {
  const WA = { x: 100, width: 2000 };

  it("converts between anchor fractions and center x symmetrically", () => {
    expect(dockAnchorCenterX(0, WA)).toBe(1100);
    expect(dockAnchorCenterX(0.5, WA)).toBe(2100);
    expect(dockAnchorCenterX(-0.5, WA)).toBe(100);
    expect(dockAnchorFromCenter(1100, WA)).toBe(0);
    expect(dockAnchorFromCenter(2100, WA)).toBe(0.5);
    expect(dockAnchorFromCenter(100, WA)).toBe(-0.5);
  });

  it("keeps negative work-area origins correct", () => {
    const NEG = { x: -1440, width: 1440 };
    expect(dockAnchorCenterX(0, NEG)).toBe(-720);
    expect(dockAnchorFromCenter(-720, NEG)).toBe(0);
    expect(dockAnchorFromCenter(-1440, NEG)).toBe(-0.5);
  });

  it("reads degenerate inputs as centered", () => {
    expect(dockAnchorFromCenter(Number.NaN, WA)).toBe(0);
    expect(dockAnchorFromCenter(1100, { x: 0, width: 0 })).toBe(0);
  });

  it("confines the shared center to where the widest placement fits", () => {
    // widestHalf 300 on the 2000-wide work area: legal centers 400..1800.
    expect(dockCenterForAnchor(0.25, WA, 300)).toBe(1600);
    expect(dockCenterForAnchor(0.5, WA, 300)).toBe(1800);
    expect(dockCenterForAnchor(-0.5, WA, 300)).toBe(400);
    // A work area narrower than the widest placement falls back to the
    // middle and lets the per-placement clamp own the edges.
    expect(dockCenterForAnchor(0.5, { x: 0, width: 500 }, 300)).toBe(250);
  });
});

describe("native dock interaction races", () => {
  const monitor = {
    scaleFactor: 1,
    workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
  };

  function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason: unknown) => void;
    const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
  }

  it("drops a pending dock placement after drag-away commits free coordinates", async () => {
    mocks.state.monitor = monitor;
    await setFloatingDock("top");
    const detach = await attachFloatingChrome(() => false);
    const constraint = deferred<undefined>();
    const started = deferred<undefined>();
    mocks.winFns.setMinSize.mockImplementationOnce(() => {
      started.resolve(undefined);
      return constraint.promise;
    });
    const dock = resizeFloatingWindow(600, 16, false, true);
    await started.promise;
    mocks.state.position = { x: 1000, y: 60 };
    mocks.state.movedHandler?.({ payload: { ...mocks.state.position } });
    await vi.waitFor(() => expect(loadFloatingQuotaPrefs().dock).toBe("none"));
    const free = resizeFloatingWindow(520, 64, false, false);
    constraint.resolve(undefined);
    await Promise.all([dock, free]);
    expect(mocks.state.position).toEqual({ x: 1000, y: 60 });
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 1000, y: 60 });
    expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
    expect(mocks.winFns.setSize).toHaveBeenCalledOnce();
    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 520, height: 64 }));
    detach();
  });

  it("drops a pending free overlay placement before a newer dock intent", async () => {
    mocks.state.monitor = monitor;
    await setFloatingDock("none");
    mocks.state.position = { x: 3200, y: 1300 };
    const lookup = deferred<Awaited<ReturnType<typeof currentMonitor>>>();
    const started = deferred<undefined>();
    vi.mocked(currentMonitor).mockImplementationOnce(() => {
      started.resolve(undefined);
      return lookup.promise;
    });
    const free = resizeFloatingWindow(520, 300, true, false);
    await started.promise;
    await setFloatingDock("top");
    const dock = resizeFloatingWindow(600, 16, false, true);
    lookup.resolve(monitor as Awaited<ReturnType<typeof currentMonitor>>);
    await Promise.all([free, dock]);
    expect(mocks.winFns.setPosition).toHaveBeenCalledOnce();
    expect(mocks.state.position).toEqual({ x: 1420, y: 0 });
    expect(mocks.winFns.setSize).toHaveBeenCalledOnce();
    expect(loadFloatingQuotaPrefs().dock).toBe("top");
  });

  it("explicit Dock then Undock drops the queued dock and restores the free point", async () => {
    mocks.state.monitor = monitor;
    window.localStorage.setItem(FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "none", x: 1000, y: 60, dockAnchor: 0.25 }));
    mocks.state.position = { x: 1000, y: 60 };
    await setFloatingDock("top");
    const dock = resizeFloatingWindow(600, 16, false, true);
    await setFloatingDock("none");
    const free = resizeFloatingWindow(520, 64, false, false);
    await Promise.all([dock, free]);
    expect(mocks.state.position).toEqual({ x: 1000, y: 60 });
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 1000, y: 60, dockAnchor: 0.25 });
    expect(mocks.winFns.setMinSize).toHaveBeenCalledOnce();
    expect(mocks.winFns.setSize).toHaveBeenCalledOnce();
    expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
  });

  it("explicit Undock then Dock drops the queued free restore and keeps the saved anchor", async () => {
    mocks.state.monitor = monitor;
    window.localStorage.setItem(FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", x: 1000, y: 60, dockAnchor: 0.25 }));
    mocks.state.position = { x: 2280, y: 0 };
    await setFloatingDock("none");
    const free = resizeFloatingWindow(520, 64, false, false);
    await setFloatingDock("top");
    const dock = resizeFloatingWindow(600, 16, false, true);
    await Promise.all([free, dock]);
    expect(mocks.state.position).toEqual({ x: 2280, y: 0 });
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "top", x: 1000, y: 60, dockAnchor: 0.25 });
    expect(mocks.winFns.setPosition).toHaveBeenCalledOnce();
    expect(mocks.winFns.setSize).toHaveBeenCalledOnce();
  });

  it("does not position a dock whose size command finishes after Undock", async () => {
    mocks.state.monitor = monitor;
    await setFloatingDock("top");
    const size = deferred<undefined>();
    const started = deferred<undefined>();
    mocks.winFns.setSize.mockImplementationOnce(() => {
      started.resolve(undefined);
      return size.promise;
    });
    const dock = resizeFloatingWindow(600, 16, false, true);
    await started.promise;
    await setFloatingDock("none");
    const free = resizeFloatingWindow(520, 64, false, false);
    size.resolve(undefined);
    await Promise.all([dock, free]);
    expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 520, height: 64 }));
  });

  it.each([true, false])("reconciles an already submitted dock move after Undock (saved free point: %s)", async (saved) => {
    mocks.state.monitor = monitor;
    window.localStorage.setItem(FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "none", x: saved ? 1000 : null, y: saved ? 60 : null }));
    mocks.state.position = { x: 1000, y: 60 };
    await setFloatingDock("top");
    const move = deferred<undefined>();
    const started = deferred<undefined>();
    mocks.winFns.setPosition.mockImplementationOnce(async (position) => {
      started.resolve(undefined);
      await move.promise;
      mocks.state.position = { x: position.x, y: position.y };
    });
    const dock = resizeFloatingWindow(600, 16, false, true);
    await started.promise;
    await setFloatingDock("none");
    const free = resizeFloatingWindow(520, 64, false, false);
    move.resolve(undefined);
    await Promise.all([dock, free]);
    expect(mocks.state.position).toEqual({ x: 1000, y: 60 });
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: saved ? 1000 : null, y: saved ? 60 : null });
  });

  it("drops startup restoration when Dock changes intent during monitor lookup", async () => {
    mocks.state.monitor = monitor;
    window.localStorage.setItem(FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "none", x: 1000, y: 60, dockAnchor: 0.25 }));
    const lookup = deferred<Awaited<ReturnType<typeof availableMonitors>>>();
    const started = deferred<undefined>();
    vi.mocked(availableMonitors).mockImplementationOnce(() => {
      started.resolve(undefined);
      return lookup.promise;
    });
    const attach = attachFloatingChrome(() => false);
    await started.promise;
    await setFloatingDock("top");
    const dock = resizeFloatingWindow(600, 16, false, true);
    lookup.resolve([monitor] as Awaited<ReturnType<typeof availableMonitors>>);
    const detach = await attach;
    await dock;
    expect(mocks.state.position).toEqual({ x: 2280, y: 0 });
    expect(mocks.winFns.setPosition).toHaveBeenCalledOnce();
    detach();
  });

  it("retains a newer free drag point while an undock correction is in flight", async () => {
    mocks.state.monitor = monitor;
    window.localStorage.setItem(FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "none", x: 1000, y: 60 }));
    mocks.state.position = { x: 1000, y: 60 };
    const detach = await attachFloatingChrome(() => false);
    await setFloatingDock("top");
    await resizeFloatingWindow(600, 16, false, true);
    mocks.winFns.setPosition.mockClear();
    const move = deferred<undefined>();
    const started = deferred<undefined>();
    mocks.winFns.setPosition.mockImplementationOnce(async (position) => {
      started.resolve(undefined);
      await move.promise;
      mocks.state.position = { x: position.x, y: position.y };
    });
    await setFloatingDock("none");
    const free = resizeFloatingWindow(520, 64, false, false);
    await started.promise;
    mocks.state.position = { x: 1200, y: 90 };
    mocks.state.movedHandler?.({ payload: { ...mocks.state.position } });
    move.resolve(undefined);
    await free;
    expect(mocks.state.position).toEqual({ x: 1200, y: 90 });
    expect(mocks.winFns.setPosition).toHaveBeenCalledTimes(2);
    detach(); // flush the unchanged trailing free-position saver
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 1200, y: 90 });
  });

  it("serializes an already submitted startup move before the newer dock placement", async () => {
    mocks.state.monitor = monitor;
    window.localStorage.setItem(FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "none", x: 1000, y: 60, dockAnchor: 0.25 }));
    vi.mocked(availableMonitors).mockResolvedValueOnce([monitor] as Awaited<ReturnType<typeof availableMonitors>>);
    const move = deferred<undefined>();
    const started = deferred<undefined>();
    mocks.winFns.setPosition.mockImplementationOnce(async (position) => {
      started.resolve(undefined);
      await move.promise;
      mocks.state.position = { x: position.x, y: position.y };
    });
    const attach = attachFloatingChrome(() => false);
    await started.promise;
    await setFloatingDock("top");
    const dock = resizeFloatingWindow(600, 16, false, true);
    move.resolve(undefined);
    const detach = await attach;
    await dock;
    expect(mocks.state.position).toEqual({ x: 2280, y: 0 });
    expect(mocks.winFns.setPosition).toHaveBeenCalledTimes(2);
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "top", x: 1000, y: 60, dockAnchor: 0.25 });
    detach();
  });

  it("a rejected obsolete size command neither blocks the queue nor restores stale state", async () => {
    mocks.state.monitor = monitor;
    await setFloatingDock("top");
    const size = deferred<undefined>();
    const started = deferred<undefined>();
    mocks.winFns.setSize.mockImplementationOnce(() => {
      started.resolve(undefined);
      return size.promise;
    });
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    const dock = resizeFloatingWindow(600, 16, false, true);
    await started.promise;
    await setFloatingDock("none");
    const free = resizeFloatingWindow(520, 64, false, false);
    size.reject(new Error("native size failed"));
    await Promise.all([dock, free]);
    expect(loadFloatingQuotaPrefs().dock).toBe("none");
    expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 520, height: 64 }));
    expect(warning).toHaveBeenCalled();
    warning.mockRestore();
  });

  it("rejects a stale dock request submitted after Undock without touching native constraints", async () => {
    mocks.state.monitor = monitor;
    await setFloatingDock("top");
    await setFloatingDock("none");
    await resizeFloatingWindow(600, 16, false, true);
    expect(mocks.winFns.setMinSize).not.toHaveBeenCalled();
    expect(mocks.winFns.setSize).not.toHaveBeenCalled();
    expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
  });

  it("waits for a cross-monitor move before collapsing to a different width", async () => {
    mocks.state.monitor = monitor;
    await setFloatingDock("top");
    const detach = await attachFloatingChrome(() => false);
    await resizeFloatingWindow(600, 64, false, true);
    mocks.winFns.setSize.mockClear();
    const nextMonitor = {
      scaleFactor: 1,
      workArea: { position: { x: 3440, y: 0 }, size: { width: 2560, height: 1400 } },
    };
    let release!: () => void;
    vi.mocked(currentMonitor).mockImplementationOnce(() => new Promise((resolve) => {
      release = () => resolve(nextMonitor as Awaited<ReturnType<typeof currentMonitor>>);
    }));
    mocks.state.movedHandler?.({ payload: { x: 3900, y: 0 } });
    mocks.state.monitor = nextMonitor;
    const collapse = resizeFloatingWindow(300, 16, false, true);
    await Promise.resolve();
    await Promise.resolve();
    expect(mocks.winFns.setSize).not.toHaveBeenCalled();
    release();
    await collapse;
    expect(loadFloatingQuotaPrefs().dockAnchor).toBeCloseTo((4200 - 4720) / 2560);
    expect(mocks.state.position).toEqual({ x: 4050, y: 0 });
    detach();
  });

  it("accepts an immediate real drag after resize and preserves its center on collapse/reveal/re-dock", async () => {
    mocks.state.monitor = monitor;
    await setFloatingDock("top");
    const detach = await attachFloatingChrome(() => false);
    await resizeFloatingWindow(600, 64, false, true);
    mocks.state.movedHandler?.({ payload: { ...mocks.state.position } }); // native echo
    mocks.state.movedHandler?.({ payload: { x: 2020, y: 0 } }); // no suppression wait
    expect(loadFloatingQuotaPrefs().dockAnchor).toBeCloseTo(600 / 3440);
    await resizeFloatingWindow(600, 16, false, true);
    expect(mocks.state.position).toEqual({ x: 2020, y: 0 });
    await resizeFloatingWindow(520, 64, false, true);
    expect(mocks.state.position).toEqual({ x: 2060, y: 0 });
    await setFloatingDock("none");
    await resizeFloatingWindow(520, 232, true, false);
    await setFloatingDock("top");
    await resizeFloatingWindow(600, 16, false, true);
    expect(mocks.state.position).toEqual({ x: 2020, y: 0 });
    detach();
    const restart = await attachFloatingChrome(() => false);
    await resizeFloatingWindow(600, 16, false, true);
    expect(mocks.state.position).toEqual({ x: 2020, y: 0 });
    restart();
  });

  it("waits for the min-size constraint before sizing and serializes transitions", async () => {
    mocks.state.monitor = monitor;
    await setFloatingDock("top");
    let release!: () => void;
    mocks.winFns.setMinSize.mockImplementationOnce(() => new Promise<undefined>((resolve) => { release = () => resolve(undefined); }));
    const rest = resizeFloatingWindow(600, 16, false, true);
    const reveal = resizeFloatingWindow(520, 64, false, true);
    await Promise.resolve();
    await Promise.resolve();
    expect(mocks.winFns.setSize).not.toHaveBeenCalled();
    release();
    await Promise.all([rest, reveal]);
    expect(mocks.winFns.setSize).toHaveBeenNthCalledWith(1, expect.objectContaining({ width: 600, height: 16 }));
    expect(mocks.winFns.setSize).toHaveBeenNthCalledWith(2, expect.objectContaining({ width: 520, height: 64 }));
    expect(mocks.state.position).toEqual({ x: 1460, y: 0 });
  });

  it("subscribes native blur and removes it on detach", async () => {
    const blur = vi.fn();
    const detach = await attachFloatingChrome(() => false, undefined, blur);
    mocks.state.listeners.get("focus-changed")?.({ payload: true });
    expect(blur).not.toHaveBeenCalled();
    mocks.state.listeners.get("focus-changed")?.({ payload: false });
    expect(blur).toHaveBeenCalledOnce();
    detach();
    expect(mocks.state.listeners.has("focus-changed")).toBe(false);
  });
});

describe("floating dock pref and drag-away undock", () => {
  // The moved-event path is fire-and-forget async in production; a macrotask
  // flush lets its microtask chain settle deterministically.
  const flushTasks = () => new Promise((resolve) => setTimeout(resolve, 0));

  it("persists the dock mode without touching geometry", async () => {
    await setFloatingDock("top");
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "top", x: null, y: null });

    await setFloatingDock("none");
    expect(loadFloatingQuotaPrefs().dock).toBe("none");
  });

  it("skips the stored-position restore while docked so the anchor owns placement", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", x: 200, y: 150 }),
    );

    const cleanup = await attachFloatingChrome(() => false);
    cleanup();

    expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
  });

  it("clears the dock and reports it when a drag crosses the top-edge threshold", async () => {
    const onDockChange = vi.fn();
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top" }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false, onDockChange);

    // Dragged further down than the strip's own 16px height.
    mocks.state.movedHandler?.({ payload: { x: 1400, y: 40 } });
    await flushTasks();

    expect(loadFloatingQuotaPrefs().dock).toBe("none");
    expect(onDockChange).toHaveBeenCalledWith("none");
    cleanup();
  });

  it("keeps the dock for jitter within the threshold and stores no position", async () => {
    const onDockChange = vi.fn();
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top" }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false, onDockChange);

    mocks.state.movedHandler?.({ payload: { x: 1400, y: 6 } });
    await flushTasks();

    expect(loadFloatingQuotaPrefs().dock).toBe("top");
    expect(onDockChange).not.toHaveBeenCalled();
    // While docked the position is derived, so jitter is never stored as a
    // free-bar position.
    expect(mocks.state.position).toEqual({ x: 80, y: 40 });
    cleanup();
  });

  it("threshold scales with the monitor factor", () => {
    // 16 logical px at 100% and 150%; degenerate scales floor at 1.
    expect(shouldUndockFromTop(16, 0, 1)).toBe(false);
    expect(shouldUndockFromTop(17, 0, 1)).toBe(true);
    expect(shouldUndockFromTop(24, 0, 1.5)).toBe(false);
    expect(shouldUndockFromTop(25, 0, 1.5)).toBe(true);
    expect(shouldUndockFromTop(0, 0, 0)).toBe(false);
  });

  it("persists a settled horizontal drag as the anchor without touching free x/y", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top" }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false);

    // Dragged right within the top threshold; the resting strip is 600px.
    await resizeFloatingWindow(600, 16, false, true);
    mocks.state.movedHandler?.({ payload: { x: 2020, y: 0 } });
    await new Promise((resolve) => setTimeout(resolve, 260));

    const prefs = loadFloatingQuotaPrefs();
    // Center 2020 + 300 = 2320; fraction = (2320 − 1720) / 3440 = 600/3440.
    expect(prefs.dockAnchor).toBeCloseTo(600 / 3440, 5);
    expect(prefs.dock).toBe("top");
    // The free bar's position is never written while docked.
    expect(prefs.x).toBeNull();
    expect(prefs.y).toBeNull();
    cleanup();
  });

  it("does not store an anchor for moves suppressed by a programmatic re-anchor", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top", dockAnchor: 0.25 }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false);

    // Only the exact programmatic position is an echo; real moves work now.
    await resizeFloatingWindow(600, 16, false, true);
    mocks.state.movedHandler?.({ payload: { ...mocks.state.position } });
    await new Promise((resolve) => setTimeout(resolve, 260));

    expect(loadFloatingQuotaPrefs().dockAnchor).toBe(0.25);
    cleanup();
  });

  it("drops a stale anchor write when the same drag undocked", async () => {
    const onDockChange = vi.fn();
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true, dock: "top" }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false, onDockChange);

    // The undocking move is claimed by the anchor saver before the async
    // undock lands; the write must be dropped, never re-dock, and the drop
    // position must not leak into the anchor.
    mocks.winFns.outerSize.mockResolvedValueOnce({ width: 600, height: 16 });
    mocks.state.movedHandler?.({ payload: { x: 1400, y: 40 } });
    await new Promise((resolve) => setTimeout(resolve, 260));

    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.dock).toBe("none");
    expect(onDockChange).toHaveBeenCalledWith("none");
    expect(prefs.dockAnchor).toBeNull();
    expect(prefs.x).toBe(1400);
    expect(prefs.y).toBe(40);
    cleanup();
  });
});

describe("native floating drag lifecycle placement", () => {
  const flushTasks = () => new Promise((resolve) => setTimeout(resolve, 0));
  const dragEvent = (
    phase: "begin" | "move" | "end",
    sequence: number,
    position: { x: number; y: number },
    reason: "release" | "cancel" | "unknown" | null = phase === "end" ? "release" : null,
    overrides: Partial<{ generation: number; gesture: number }> = {},
  ) => ({ generation: 41, gesture: 9, sequence, phase, reason, position, ...overrides });

  function emitDrag(event: ReturnType<typeof dragEvent>) {
    mocks.state.listeners.get(FLOATING_DRAG_LIFECYCLE_EVENT)?.({ payload: event });
  }

  beforeEach(() => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "attach_floating_drag_lifecycle") return 41;
      if (command === "restore_floating_drag_origin") return true;
      return undefined;
    });
  });

  afterEach(() => {
    mocks.winFns.outerSize.mockReset();
    mocks.winFns.outerSize.mockImplementation(async () => ({ width: 520, height: 64 }));
    vi.mocked(availableMonitors).mockReset();
    vi.mocked(availableMonitors).mockImplementation(async () => []);
  });

  it("listens before attach, buffers a fast release, and detaches its generation once", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "attach_floating_drag_lifecycle") {
        emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
        emitDrag(dragEvent("end", 2, { x: 120, y: 90 }));
        return 41;
      }
      return undefined;
    });

    const cleanup = await attachFloatingChrome(() => false);
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ x: 120, y: 90 });
    cleanup();
    cleanup();
    expect(mocks.invoke).toHaveBeenCalledWith("detach_floating_drag_lifecycle", {
      generation: 41,
    });
    expect(mocks.invoke.mock.calls.filter(([command]) => command === "detach_floating_drag_lifecycle"))
      .toHaveLength(1);
  });

  it("shows only after the lifecycle listener and native attachment are ready", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ visible: true }),
    );
    let resolveAttach!: (generation: unknown) => void;
    mocks.invoke.mockImplementation((command: string) => {
      if (command !== "attach_floating_drag_lifecycle") return Promise.resolve(undefined);
      return new Promise<unknown>((resolve) => { resolveAttach = resolve; });
    });

    const attaching = attachFloatingChrome(() => false);
    await flushTasks();
    expect(mocks.state.listeners.has(FLOATING_DRAG_LIFECYCLE_EVENT)).toBe(true);
    expect(mocks.winFns.show).not.toHaveBeenCalled();
    resolveAttach(41);
    const cleanup = await attaching;

    expect(mocks.winFns.show).toHaveBeenCalledOnce();
    cleanup();
  });

  it("aborts a pending native attach without accepting late events and detaches late generation once", async () => {
    let resolveAttach!: (generation: unknown) => void;
    mocks.invoke.mockImplementation((command: string) => {
      if (command !== "attach_floating_drag_lifecycle") return Promise.resolve(undefined);
      return new Promise<unknown>((resolve) => { resolveAttach = resolve; });
    });
    const controller = new AbortController();
    const attaching = attachFloatingChrome(() => false, undefined, undefined, controller.signal);
    await flushTasks();
    const staleListener = mocks.state.listeners.get(FLOATING_DRAG_LIFECYCLE_EVENT);
    expect(staleListener).toBeDefined();

    controller.abort();
    staleListener?.({ payload: dragEvent("begin", 1, { x: 300, y: 200 }) });
    staleListener?.({ payload: dragEvent("end", 2, { x: 400, y: 300 }) });
    resolveAttach(41);
    const cleanup = await attaching;
    await flushTasks();

    expect(loadFloatingQuotaPrefs()).toMatchObject({ x: null, y: null });
    expect(mocks.invoke.mock.calls.filter(([command]) => command === "detach_floating_drag_lifecycle"))
      .toHaveLength(1);
    cleanup();
    expect(mocks.invoke.mock.calls.filter(([command]) => command === "detach_floating_drag_lifecycle"))
      .toHaveLength(1);
  });

  it("defers native user movement until release and ignores duplicate tauri moved events", async () => {
    const cleanup = await attachFloatingChrome(() => false);
    emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
    emitDrag(dragEvent("move", 2, { x: 200, y: 120 }));
    mocks.state.movedHandler?.({ payload: { x: 999, y: 999 } });
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ x: null, y: null });

    emitDrag(dragEvent("end", 3, { x: 240, y: 160 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ x: 240, y: 160 });
    // A late Tao notification after terminal is not a second user drag.
    mocks.state.movedHandler?.({ payload: { x: 998, y: 998 } });
    await new Promise((resolve) => setTimeout(resolve, 260));
    expect(loadFloatingQuotaPrefs()).toMatchObject({ x: 240, y: 160 });
    cleanup();
  });

  it("does not treat a Tao moved notification without a native begin as user input", async () => {
    const cleanup = await attachFloatingChrome(() => false);
    mocks.state.movedHandler?.({ payload: { x: 700, y: 500 } });
    await new Promise((resolve) => setTimeout(resolve, 260));

    expect(loadFloatingQuotaPrefs()).toMatchObject({ x: null, y: null });
    cleanup();
  });

  it("reports a native adapter failure and disables rather than approximating user drag persistence", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "attach_floating_drag_lifecycle") throw new Error("native hook unavailable");
      return undefined;
    });
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const cleanup = await attachFloatingChrome(() => false);
      mocks.state.movedHandler?.({ payload: { x: 700, y: 500 } });
      await new Promise((resolve) => setTimeout(resolve, 260));

      expect(loadFloatingQuotaPrefs()).toMatchObject({ x: null, y: null });
      expect(error).toHaveBeenCalledWith(
        expect.stringContaining("adapter failed; user drag persistence is disabled"),
        expect.any(Error),
      );
      cleanup();
    } finally {
      error.mockRestore();
    }
  });

  it("keeps dock mode through an unsnap preview and commits the free point on release", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0, x: 22, y: 33 }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false);
    emitDrag(dragEvent("begin", 1, { x: 80, y: 0 }));
    emitDrag(dragEvent("move", 2, { x: 1400, y: 40 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "top", dockAnchor: 0, x: 22, y: 33 });

    emitDrag(dragEvent("end", 3, { x: 1450, y: 55 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 1450, y: 55 });
    cleanup();
  });

  it("coalesces held native moves while monitor lookup is delayed and commits the terminal point", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0 }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false);
    vi.mocked(currentMonitor).mockClear();
    const releases: Array<(monitor: Awaited<ReturnType<typeof currentMonitor>>) => void> = [];
    vi.mocked(currentMonitor).mockImplementation(() => new Promise((resolve) => {
      releases.push(resolve);
    }));

    try {
      emitDrag(dragEvent("begin", 1, { x: 80, y: 0 }));
      for (let sequence = 2; sequence <= 101; sequence++) {
        emitDrag(dragEvent("move", sequence, { x: 100 + sequence, y: 0 }));
      }
      emitDrag(dragEvent("end", 102, { x: 2400, y: 0 }));
      await Promise.resolve();
      expect(currentMonitor).toHaveBeenCalledTimes(1);

      const monitor = {
        scaleFactor: 1,
        workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
      } as Awaited<ReturnType<typeof currentMonitor>>;
      releases.shift()?.(monitor);
      await Promise.resolve();
      await Promise.resolve();
      releases.shift()?.(monitor);
      await flushTasks();

      expect(vi.mocked(currentMonitor).mock.calls.length).toBeLessThanOrEqual(3);
      expect(loadFloatingQuotaPrefs().dock).toBe("top");
      expect(loadFloatingQuotaPrefs().dockAnchor).toBeCloseTo((2400 + 260 - 1720) / 3440, 8);
    } finally {
      cleanup();
      vi.mocked(currentMonitor).mockImplementation(async () =>
        mocks.state.monitor as Awaited<ReturnType<typeof currentMonitor>>,
      );
    }
  });

  it("settles a released stream when explicit Dock invalidates a blocked monitor move", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0 }),
    );
    const cleanup = await attachFloatingChrome(() => false);
    let release!: (monitor: Awaited<ReturnType<typeof currentMonitor>>) => void;
    vi.mocked(currentMonitor).mockImplementationOnce(() => new Promise((resolve) => {
      release = resolve;
    }));

    emitDrag(dragEvent("begin", 1, { x: 80, y: 0 }));
    emitDrag(dragEvent("move", 2, { x: 1400, y: 40 }));
    await Promise.resolve();
    await setFloatingDock("none");
    emitDrag(dragEvent("end", 3, { x: 1400, y: 40 }));
    release({
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    } as Awaited<ReturnType<typeof currentMonitor>>);
    await flushTasks();

    expect(loadFloatingQuotaPrefs().dock).toBe("none");
    cleanup();
  });

  it("defers the latest resize intent until native pointer capture ends", async () => {
    const cleanup = await attachFloatingChrome(() => false);
    emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
    await resizeFloatingWindow(320, 64, false, false);
    await resizeFloatingWindow(480, 64, false, false);
    expect(mocks.winFns.setSize).not.toHaveBeenCalled();

    emitDrag(dragEvent("end", 2, { x: 80, y: 40 }));
    await flushTasks();
    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(
      expect.objectContaining({ width: 480, height: 64 }),
    );
    cleanup();
  });

  it("cancels a pending monitor lookup and restores the complete placement origin", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.25, x: 22, y: 33 }),
    );
    const onDockChange = vi.fn();
    const cleanup = await attachFloatingChrome(() => false, onDockChange);
    let release!: (value: Awaited<ReturnType<typeof currentMonitor>>) => void;
    vi.mocked(currentMonitor).mockImplementationOnce(() => new Promise((resolve) => {
      release = resolve;
    }));
    emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
    emitDrag(dragEvent("move", 2, { x: 1400, y: 40 }));
    await Promise.resolve();
    emitDrag(dragEvent("end", 3, { x: 1400, y: 40 }, "cancel"));
    release({
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    } as Awaited<ReturnType<typeof currentMonitor>>);
    await flushTasks();

    expect(loadFloatingQuotaPrefs()).toMatchObject({
      dock: "top", dockAnchor: 0.25, x: 22, y: 33,
    });
    expect(onDockChange).toHaveBeenCalledWith("top");
    expect(mocks.invoke).toHaveBeenCalledWith("restore_floating_drag_origin", {
      generation: 41,
      gesture: 9,
      x: 80,
      y: 40,
    });
    cleanup();
  });

  it("does not let a cancelled stale session overwrite an explicit new dock mode", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.25, x: 22, y: 33 }),
    );
    const cleanup = await attachFloatingChrome(() => false);
    let release!: (value: Awaited<ReturnType<typeof currentMonitor>>) => void;
    vi.mocked(currentMonitor).mockImplementationOnce(() => new Promise((resolve) => {
      release = resolve;
    }));
    emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
    emitDrag(dragEvent("move", 2, { x: 1400, y: 40 }));
    await Promise.resolve();
    await setFloatingDock("none");
    emitDrag(dragEvent("end", 3, { x: 1400, y: 40 }, "unknown"));
    release({
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    } as Awaited<ReturnType<typeof currentMonitor>>);
    await flushTasks();

    expect(loadFloatingQuotaPrefs().dock).toBe("none");
    cleanup();
  });

  it("drops a queued cancellation restore when a newer native begin owns placement", async () => {
    let releaseSize!: () => void;
    mocks.winFns.setMinSize.mockImplementationOnce(() => new Promise<undefined>((resolve) => {
      releaseSize = () => resolve(undefined);
    }));
    const queuedResize = resizeFloatingWindow(320, 64, false, false);
    await flushTasks();
    expect(mocks.winFns.setMinSize).toHaveBeenCalledOnce();
    const cleanup = await attachFloatingChrome(() => false);

    emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
    emitDrag(dragEvent("end", 2, { x: 80, y: 40 }, "cancel"));
    emitDrag(dragEvent("begin", 1, { x: 500, y: 300 }, null, { gesture: 10 }));
    releaseSize();
    await queuedResize;
    await flushTasks();

    expect(mocks.winFns.setPosition).not.toHaveBeenCalledWith(
      expect.objectContaining({ x: 80, y: 40 }),
    );
    emitDrag(dragEvent("end", 2, { x: 500, y: 300 }, "release", { gesture: 10 }));
    await flushTasks();
    cleanup();
  });

  it("restores cancelled drag-away prefs before a later begin snapshots its origin", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.25, x: 22, y: 33 }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false);

    emitDrag(dragEvent("begin", 1, { x: 80, y: 0 }));
    emitDrag(dragEvent("move", 2, { x: 1400, y: 40 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs().dock).toBe("top");

    emitDrag(dragEvent("end", 3, { x: 1400, y: 40 }, "cancel"));
    expect(loadFloatingQuotaPrefs()).toMatchObject({
      dock: "top", dockAnchor: 0.25, x: 22, y: 33,
    });
    emitDrag(dragEvent("begin", 1, { x: 500, y: 0 }, null, { gesture: 10 }));
    emitDrag(dragEvent("end", 2, { x: 500, y: 0 }, "cancel", { gesture: 10 }));
    await flushTasks();

    expect(loadFloatingQuotaPrefs()).toMatchObject({
      dock: "top", dockAnchor: 0.25, x: 22, y: 33,
    });
    cleanup();
  });

  it("drops a deferred free-size intent when cancel restores the top dock", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.25, x: 22, y: 33 }),
    );
    mocks.state.monitor = {
      scaleFactor: 1,
      workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
    };
    const cleanup = await attachFloatingChrome(() => false);
    emitDrag(dragEvent("begin", 1, { x: 80, y: 0 }));
    emitDrag(dragEvent("move", 2, { x: 1400, y: 40 }));
    await flushTasks();
    await resizeFloatingWindow(320, 64, false, false);
    emitDrag(dragEvent("end", 3, { x: 1400, y: 40 }, "cancel"));
    await flushTasks();

    expect(loadFloatingQuotaPrefs().dock).toBe("top");
    expect(mocks.winFns.setSize).not.toHaveBeenCalledWith(
      expect.objectContaining({ width: 320, height: 64 }),
    );
    cleanup();
  });

  it("does not flush deferred placement after a submitted restore loses ownership", async () => {
    let releaseRestore!: (restored: boolean) => void;
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "attach_floating_drag_lifecycle") return Promise.resolve(41);
      if (command === "restore_floating_drag_origin") {
        return new Promise<boolean>((resolve) => { releaseRestore = resolve; });
      }
      return Promise.resolve(undefined);
    });
    const cleanup = await attachFloatingChrome(() => false);
    emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
    await resizeFloatingWindow(480, 64, false, false);
    emitDrag(dragEvent("end", 2, { x: 80, y: 40 }, "cancel"));
    await Promise.resolve();
    emitDrag(dragEvent("begin", 1, { x: 500, y: 300 }, null, { gesture: 10 }));
    releaseRestore(true);
    await flushTasks();

    expect(mocks.winFns.setSize).not.toHaveBeenCalled();
    emitDrag(dragEvent("end", 2, { x: 500, y: 300 }, "release", { gesture: 10 }));
    await flushTasks();
    cleanup();
  });

  it("does not flush a pending cancellation restore rejected after a newer native begin", async () => {
    let releaseRestore!: (restored: boolean) => void;
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "attach_floating_drag_lifecycle") return Promise.resolve(41);
      if (command === "restore_floating_drag_origin") {
        return new Promise<boolean>((resolve) => { releaseRestore = resolve; });
      }
      return Promise.resolve(undefined);
    });
    const cleanup = await attachFloatingChrome(() => false);
    emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
    await resizeFloatingWindow(600, 16, false, true);
    emitDrag(dragEvent("end", 2, { x: 80, y: 40 }, "cancel"));
    await Promise.resolve();
    expect(mocks.invoke).toHaveBeenCalledWith("restore_floating_drag_origin", {
      generation: 41,
      gesture: 9,
      x: 80,
      y: 40,
    });

    emitDrag(dragEvent("begin", 1, { x: 500, y: 300 }, null, { gesture: 10 }));
    releaseRestore(false);
    await flushTasks();

    expect(mocks.winFns.setSize).not.toHaveBeenCalled();
    emitDrag(dragEvent("end", 2, { x: 500, y: 300 }, "release", { gesture: 10 }));
    await flushTasks();
    cleanup();
  });

  it("drops old deferred size on explicit Dock/Undock but applies the new intent", async () => {
    const cleanup = await attachFloatingChrome(() => false);
    emitDrag(dragEvent("begin", 1, { x: 80, y: 40 }));
    await resizeFloatingWindow(600, 16, false, true);
    await setFloatingDock("none");
    await resizeFloatingWindow(320, 64, false, false);

    emitDrag(dragEvent("end", 2, { x: 80, y: 40 }, "unknown"));
    await flushTasks();
    expect(mocks.winFns.setSize).toHaveBeenLastCalledWith(
      expect.objectContaining({ width: 320, height: 64 }),
    );
    expect(mocks.winFns.setSize).not.toHaveBeenCalledWith(
      expect.objectContaining({ width: 600, height: 16 }),
    );
    cleanup();
  });

  function monitor(
    x: number,
    y: number,
    width: number,
    height: number,
    scaleFactor = 1,
  ) {
    return {
      scaleFactor,
      workArea: { position: { x, y }, size: { width, height } },
    };
  }

  async function gesture(
    positions: Array<{ x: number; y: number }>,
    reason: "release" | "cancel" | "unknown" = "release",
  ) {
    emitDrag(dragEvent("begin", 1, positions[0]));
    positions.slice(1, -1).forEach((position, index) => {
      emitDrag(dragEvent("move", index + 2, position));
    });
    const end = positions[positions.length - 1];
    emitDrag(dragEvent("end", positions.length === 1 ? 2 : positions.length, end, reason));
    await flushTasks();
  }

  it("does not dock a floating release outside the top band", async () => {
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    const preview = vi.fn();
    const cleanup = await attachFloatingChrome(() => false, undefined, undefined, undefined, preview);
    emitDrag(dragEvent("begin", 1, { x: 400, y: 400 }));
    emitDrag(dragEvent("move", 2, { x: 420, y: 300 }));
    await flushTasks();
    expect(preview).not.toHaveBeenCalled();
    expect(loadFloatingQuotaPrefs().dock).toBe("none");
    emitDrag(dragEvent("end", 3, { x: 430, y: 280 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 430, y: 280 });
    cleanup();
  });

  it("previews inside the onset band and docks only on an eligible release", async () => {
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    mocks.winFns.outerSize.mockResolvedValue({ width: 480, height: 64 });
    const preview = vi.fn();
    const onDockChange = vi.fn();
    const cleanup = await attachFloatingChrome(
      () => false,
      onDockChange,
      undefined,
      undefined,
      preview,
    );
    emitDrag(dragEvent("begin", 1, { x: 2000, y: 400 }));
    emitDrag(dragEvent("move", 2, { x: 2000, y: 80 }));
    await flushTasks();
    expect(preview).toHaveBeenCalledWith("snap");
    expect(loadFloatingQuotaPrefs().dock).toBe("none");

    emitDrag(dragEvent("end", 3, { x: 2000, y: 80 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 2000, y: 80 });
    expect(onDockChange).not.toHaveBeenCalled();
    cleanup();
  });

  it("commits an off-center dock on an eligible release and keeps the old free point", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "none", x: 90, y: 700 }),
    );
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    mocks.winFns.outerSize.mockResolvedValue({ width: 480, height: 64 });
    const onDockChange = vi.fn();
    const cleanup = await attachFloatingChrome(() => false, onDockChange);
    await gesture([{ x: 2000, y: 400 }, { x: 2000, y: 20 }]);
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.dock).toBe("top");
    expect(prefs.x).toBe(90);
    expect(prefs.y).toBe(700);
    expect(prefs.dockAnchor).toBeCloseTo((2240 - 1720) / 3440, 8);
    expect(onDockChange).toHaveBeenCalledWith("top");
    cleanup();
  });

  it("docks against the portrait monitor that contains the drag, not the primary", async () => {
    const primary = monitor(0, 0, 3440, 1400);
    const portrait = monitor(3440, 0, 1440, 2560);
    mocks.state.monitor = primary;
    vi.mocked(availableMonitors).mockResolvedValue([primary, portrait] as Awaited<
      ReturnType<typeof availableMonitors>
    >);
    mocks.winFns.outerSize.mockResolvedValue({ width: 480, height: 64 });
    const cleanup = await attachFloatingChrome(() => false);
    await gesture([{ x: 3600, y: 400 }, { x: 3800, y: 12 }]);
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.dock).toBe("top");
    expect(prefs.dockAnchor).toBeCloseTo((4040 - 4160) / 1440, 8);
    expect(prefs.dockAnchor).not.toBeCloseTo(0.5, 2);
    cleanup();
  });

  it("stays docked below the detach threshold and floats after a release past it", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.2, x: 11, y: 22 }),
    );
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    const preview = vi.fn();
    const cleanup = await attachFloatingChrome(() => false, undefined, undefined, undefined, preview);
    emitDrag(dragEvent("begin", 1, { x: 800, y: 0 }));
    emitDrag(dragEvent("move", 2, { x: 900, y: 28 }));
    await flushTasks();
    expect(preview).not.toHaveBeenCalledWith("unsnap");
    expect(loadFloatingQuotaPrefs().dock).toBe("top");
    emitDrag(dragEvent("end", 3, { x: 900, y: 28 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs().dock).toBe("top");
    expect(loadFloatingQuotaPrefs().x).toBe(11);
    cleanup();
  });

  it("enters unsnap preview without committing, then floats on release", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.2, x: 11, y: 22 }),
    );
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    const preview = vi.fn();
    const onDockChange = vi.fn();
    const cleanup = await attachFloatingChrome(
      () => false,
      onDockChange,
      undefined,
      undefined,
      preview,
    );
    emitDrag(dragEvent("begin", 1, { x: 800, y: 0 }));
    emitDrag(dragEvent("move", 2, { x: 1100, y: 48 }));
    await flushTasks();
    expect(preview).toHaveBeenCalledWith("unsnap");
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "top", dockAnchor: 0.2, x: 11, y: 22 });
    emitDrag(dragEvent("end", 3, { x: 1100, y: 64 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", dockAnchor: 0.2, x: 1100, y: 64 });
    expect(onDockChange).toHaveBeenCalledWith("none");
    cleanup();
  });

  it("settles back to docked when an unsnap preview returns to the edge", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.2, x: 11, y: 22 }),
    );
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    mocks.winFns.outerSize.mockResolvedValue({ width: 600, height: 16 });
    const preview = vi.fn();
    const cleanup = await attachFloatingChrome(() => false, undefined, undefined, undefined, preview);
    await resizeFloatingWindow(600, 16, false, true);
    emitDrag(dragEvent("begin", 1, { x: 800, y: 0 }));
    emitDrag(dragEvent("move", 2, { x: 1000, y: 70 }));
    await flushTasks();
    expect(preview).toHaveBeenCalledWith("unsnap");
    emitDrag(dragEvent("move", 3, { x: 1200, y: 10 }));
    await flushTasks();
    expect(preview).toHaveBeenLastCalledWith(null);
    emitDrag(dragEvent("end", 4, { x: 1200, y: 10 }));
    await flushTasks();
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.dock).toBe("top");
    expect(prefs.x).toBe(11);
    expect(prefs.y).toBe(22);
    expect(prefs.dockAnchor).toBeCloseTo((1500 - 1720) / 3440, 8);
    cleanup();
  });

  it("does not dock when snap preview is cancelled or ends unknown", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "none", x: 15, y: 25, dockAnchor: null }),
    );
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    const cleanup = await attachFloatingChrome(() => false);
    await gesture([{ x: 500, y: 300 }, { x: 500, y: 12 }], "cancel");
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 15, y: 25 });
    expect(mocks.invoke).toHaveBeenCalledWith("restore_floating_drag_origin", {
      generation: 41,
      gesture: 9,
      x: 500,
      y: 300,
    });
    cleanup();
  });

  it("stays docked when unsnap preview ends unknown", async () => {
    window.localStorage.setItem(
      FLOATING_PREFS_STORAGE_KEY,
      JSON.stringify({ dock: "top", dockAnchor: 0.25, x: 15, y: 25 }),
    );
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    const cleanup = await attachFloatingChrome(() => false);
    await gesture([{ x: 700, y: 0 }, { x: 700, y: 90 }], "unknown");
    expect(loadFloatingQuotaPrefs()).toMatchObject({
      dock: "top",
      dockAnchor: 0.25,
      x: 15,
      y: 25,
    });
    cleanup();
  });

  it("ignores a stale move after the docking gesture has ended", async () => {
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    mocks.winFns.outerSize.mockResolvedValue({ width: 480, height: 64 });
    const cleanup = await attachFloatingChrome(() => false);
    await gesture([{ x: 400, y: 500 }, { x: 400, y: 200 }]);
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 400, y: 200 });
    emitDrag(dragEvent("move", 4, { x: 400, y: 8 }));
    await flushTasks();
    expect(loadFloatingQuotaPrefs()).toMatchObject({ dock: "none", x: 400, y: 200 });
    cleanup();
  });

  it("keeps the Dock and Undock menu path after a non-docking drag", async () => {
    mocks.state.monitor = monitor(0, 0, 3440, 1400);
    const cleanup = await attachFloatingChrome(() => false);
    await gesture([{ x: 100, y: 500 }, { x: 180, y: 220 }]);
    await setFloatingDock("top");
    expect(loadFloatingQuotaPrefs().dock).toBe("top");
    await setFloatingDock("none");
    expect(loadFloatingQuotaPrefs().dock).toBe("none");
    cleanup();
  });

  it("scales docking eligibility once at 150% without recentering an off-center drag", async () => {
    mocks.state.monitor = monitor(0, 0, 3440, 1400, 1.5);
    mocks.winFns.outerSize.mockResolvedValue({ width: 480, height: 64 });
    const cleanup = await attachFloatingChrome(() => false);
    // 50 physical px is inside the 60px eligibility at 150%, and outside 40 at 100%.
    await gesture([{ x: 2000, y: 400 }, { x: 2000, y: 50 }]);
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.dock).toBe("top");
    expect(prefs.dockAnchor).toBeCloseTo((2240 - 1720) / 3440, 8);
    cleanup();
  });
});

describe("attachDockReanchor (docked DPI hardening)", () => {
  beforeEach(async () => { await setFloatingDock("top"); });
  function captureWindowEvents() {
    const handlers = new Map<string, (event: unknown) => void>();
    const winAny = mocks.winFns as unknown as Record<string, unknown>;
    winAny.onScaleChanged = vi.fn(async (handler: (event: unknown) => void) => {
      handlers.set("scale", handler);
      return () => handlers.delete("scale");
    });
    winAny.onFocusChanged = vi.fn(async (handler: (event: unknown) => void) => {
      handlers.set("focus", handler);
      return () => handlers.delete("focus");
    });
    return handlers;
  }

  it("drops a pending focus re-anchor across Undock/Dock and keeps new events working", async () => {
    const handlers = captureWindowEvents();
    const reanchor = vi.fn();
    const detach = attachDockReanchor(reanchor);
    let release!: () => void;
    vi.mocked(currentMonitor).mockImplementationOnce(() => new Promise((resolve) => {
      release = () => resolve(null);
    }));
    handlers.get("focus")?.({ payload: true });
    await setFloatingDock("none");
    handlers.get("scale")?.(undefined);
    await setFloatingDock("top");
    release();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(reanchor).not.toHaveBeenCalled();
    handlers.get("scale")?.(undefined);
    expect(reanchor).toHaveBeenCalledOnce();
    detach();
  });

  it("drops a focus re-anchor that resolves after detach", async () => {
    const handlers = captureWindowEvents();
    const reanchor = vi.fn();
    const detach = attachDockReanchor(reanchor);
    let release!: () => void;
    vi.mocked(currentMonitor).mockImplementationOnce(() => new Promise((resolve) => {
      release = () => resolve(null);
    }));
    handlers.get("focus")?.({ payload: true });
    detach();
    release();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(reanchor).not.toHaveBeenCalled();
  });

  it("re-anchors on scale change and focus regain, ignoring blur, and cleans up", async () => {
    const handlers = captureWindowEvents();
    const onReanchor = vi.fn();

    const cleanup = attachDockReanchor(onReanchor);

    handlers.get("scale")?.(undefined);
    handlers.get("focus")?.({ payload: true });
    handlers.get("focus")?.({ payload: false });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(onReanchor).toHaveBeenCalledTimes(2);

    cleanup();
    // The cleanup resolves the unlisten promises before unsubscribing.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(handlers.has("scale")).toBe(false);
    expect(handlers.has("focus")).toBe(false);
  });

  it("is a no-op outside the Tauri runtime", () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    const onReanchor = vi.fn();
    const cleanup = attachDockReanchor(onReanchor);
    cleanup();
    expect(onReanchor).not.toHaveBeenCalled();
  });

  it("does not re-anchor on focus when the monitor geometry is unchanged", async () => {
    mocks.state.monitor = {
      scaleFactor: 1.5,
      workArea: { position: { x: -5160, y: 100 }, size: { width: 5160, height: 2100 } },
    };
    await resizeFloatingWindow(600, 16, false, true);
    const handlers = captureWindowEvents();
    const reanchor = vi.fn();
    const detach = attachDockReanchor(reanchor);
    handlers.get("focus")?.({ payload: true });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(reanchor).not.toHaveBeenCalled();
    mocks.state.monitor.workArea.position.y = 140;
    // Native monitor calls return new snapshots rather than mutable objects.
    mocks.state.monitor = { ...mocks.state.monitor, workArea: { ...mocks.state.monitor.workArea, position: { x: -5160, y: 180 } } };
    handlers.get("focus")?.({ payload: true });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(reanchor).toHaveBeenCalledOnce();
    detach();
  });
});

describe("native smart docking session commits", () => {
  const GEN = 5;
  const MONITOR = {
    scaleFactor: 1,
    workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
  };
  const restoreCalls: Array<Record<string, unknown>> = [];
  let cleanupFn: (() => void) | null = null;
  let onDockChange: ReturnType<typeof vi.fn>;
  let gesturePreview: ReturnType<typeof vi.fn>;

  const send = (payload: unknown) => {
    const handler = mocks.state.listeners.get(FLOATING_DRAG_LIFECYCLE_EVENT) as
      | ((event: { payload: unknown }) => void)
      | undefined;
    if (!handler) throw new Error("lifecycle listener was not registered");
    handler({ payload });
  };

  const dragEvent = (
    phase: "begin" | "move" | "end",
    sequence: number,
    x: number,
    y: number,
    overrides: Record<string, unknown> = {},
  ) => ({
    generation: GEN,
    gesture: 1,
    sequence,
    phase,
    reason: phase === "end" ? "release" : null,
    position: { x, y },
    ...overrides,
  });

  const settle = async () => {
    for (let i = 0; i < 12; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  };

  const attach = async () => {
    cleanupFn = await attachFloatingChrome(
      () => false,
      onDockChange,
      undefined,
      undefined,
      gesturePreview,
    );
  };

  beforeEach(() => {
    mocks.state.monitor = MONITOR;
    vi.mocked(availableMonitors).mockResolvedValue([MONITOR as never]);
    restoreCalls.length = 0;
    onDockChange = vi.fn();
    gesturePreview = vi.fn();
    mocks.invoke.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "attach_floating_drag_lifecycle") return GEN;
      if (command === "restore_floating_drag_origin") {
        restoreCalls.push(args as Record<string, unknown>);
        return true;
      }
      return undefined;
    });
  });

  afterEach(() => {
    cleanupFn?.();
    cleanupFn = null;
    vi.mocked(availableMonitors).mockReset();
  });

  it("commits DOCKED only on a release-qualified terminal at an eligible edge", async () => {
    // The shared beforeEach leaves the bar free-floating.
    await attach();
    send(dragEvent("begin", 1, 2000, 400));
    send(dragEvent("move", 2, 2000, 120));
    send(dragEvent("move", 3, 2000, 25));
    await settle();
    // Moves preview only: the floating placement is still uncommitted.
    expect(loadFloatingQuotaPrefs().dock).toBe("none");
    expect(gesturePreview).toHaveBeenLastCalledWith("snap-ready");

    send(dragEvent("end", 4, 2000, 25));
    await vi.waitFor(() => expect(loadFloatingQuotaPrefs().dock).toBe("top"));
    // Off-center x survives the snap as a work-area-relative anchor:
    // window center 2260 (520 wide) against work-area center 1720.
    expect(loadFloatingQuotaPrefs().dockAnchor).toBeCloseTo((2260 - 1720) / 3440, 8);
    expect(onDockChange).toHaveBeenCalledWith("top");
    expect(restoreCalls).toEqual([]);
  });

  it("cancel and unknown terminals fail safe back to the gesture origin", async () => {
    await attach();
    for (const [index, reason] of ["cancel", "unknown"].entries()) {
      const gesture = index + 1;
      send(dragEvent("begin", 1, 2000, 400, { gesture }));
      send(dragEvent("move", 2, 2000, 25, { gesture })); // hover eligible, never commits
      send(dragEvent("end", 3, 2000, 25, { gesture, reason }));
      await vi.waitFor(() => expect(restoreCalls.length).toBe(1));
      expect(restoreCalls[0]).toEqual({ generation: GEN, gesture, x: 2000, y: 400 });
      const prefs = loadFloatingQuotaPrefs();
      expect(prefs.dock).toBe("none");
      expect(prefs.x).toBeNull();
      expect(prefs.y).toBeNull();
      // The closed gesture is single-shot: a duplicate terminal can neither
      // restore nor commit again.
      send(dragEvent("end", 4, 2000, 25, { gesture, reason }));
      await settle();
      expect(restoreCalls.length).toBe(1);
      expect(loadFloatingQuotaPrefs().dock).toBe("none");
      restoreCalls.length = 0;
    }
  });

  it("commits undock only on release; a move through the detach band only previews", async () => {
    await setFloatingDock("top");
    await attach();
    send(dragEvent("begin", 1, 1400, 0));
    send(dragEvent("move", 2, 1400, 200)); // far past the detach threshold
    await settle();
    expect(gesturePreview).toHaveBeenCalledWith("unsnap");
    // The move alone must not clear the dock.
    expect(loadFloatingQuotaPrefs().dock).toBe("top");
    expect(loadFloatingQuotaPrefs().x).toBeNull();

    send(dragEvent("end", 3, 1400, 200));
    await vi.waitFor(() => expect(loadFloatingQuotaPrefs().dock).toBe("none"));
    expect(loadFloatingQuotaPrefs()).toMatchObject({ x: 1400, y: 200 });
    expect(onDockChange).toHaveBeenCalledWith("none");
    expect(restoreCalls).toEqual([]);
  });

  it("returning to the dock edge before release keeps DOCKED and writes only the anchor", async () => {
    await setFloatingDock("top");
    await attach();
    send(dragEvent("begin", 1, 1400, 0));
    send(dragEvent("move", 2, 1400, 200));
    send(dragEvent("move", 3, 1400, 8));
    await settle();
    expect(loadFloatingQuotaPrefs().dock).toBe("top");

    send(dragEvent("end", 4, 1400, 8));
    await vi.waitFor(() =>
      expect(loadFloatingQuotaPrefs().dockAnchor).toBeCloseTo((1660 - 1720) / 3440, 8),
    );
    expect(loadFloatingQuotaPrefs().dock).toBe("top");
    expect(onDockChange).toHaveBeenCalledWith("top");
  });

  it("cancelling a docked drag restores the docked origin and asks native to reposition", async () => {
    saveFloatingQuotaPrefs({ ...loadFloatingQuotaPrefs(), dock: "top", dockAnchor: 0.25 });
    await attach();
    send(dragEvent("begin", 1, 1400, 0));
    send(dragEvent("move", 2, 1400, 300));
    send(dragEvent("end", 3, 1400, 300, { reason: "cancel" }));
    await vi.waitFor(() => expect(restoreCalls.length).toBe(1));
    expect(restoreCalls[0]).toEqual({ generation: GEN, gesture: 1, x: 1400, y: 0 });
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.dock).toBe("top");
    expect(prefs.dockAnchor).toBe(0.25);
    expect(onDockChange).toHaveBeenCalledWith("top");
  });

  it("rejects a whole gesture whose generation does not match the attached one", async () => {
    await attach();
    send(dragEvent("begin", 1, 2000, 25, { generation: GEN - 1 }));
    send(dragEvent("move", 2, 2000, 25, { generation: GEN - 1 }));
    send(dragEvent("end", 3, 2000, 25, { generation: GEN - 1 }));
    await settle();
    expect(loadFloatingQuotaPrefs().dock).toBe("none");
    expect(restoreCalls).toEqual([]);
    expect(gesturePreview).not.toHaveBeenCalled();

    // The attached generation is unaffected by the stale traffic.
    send(dragEvent("begin", 1, 1800, 30));
    send(dragEvent("end", 2, 1800, 30));
    await vi.waitFor(() => expect(loadFloatingQuotaPrefs().dock).toBe("top"));
  });

  it("an explicit menu Undock during a gesture makes that session inert", async () => {
    await setFloatingDock("top");
    await attach();
    send(dragEvent("begin", 1, 1400, 0));
    await setFloatingDock("none"); // the menu fallback wins over the gesture
    send(dragEvent("move", 2, 1400, 25));
    send(dragEvent("end", 3, 1400, 25)); // release must not re-dock
    await settle();
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.dock).toBe("none");
    expect(prefs.x).toBeNull();
    expect(prefs.y).toBeNull();
    expect(restoreCalls).toEqual([]);
  });
});
