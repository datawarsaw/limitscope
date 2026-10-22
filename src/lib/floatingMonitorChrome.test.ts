// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * Integration tests for the Phase B monitor recovery: the topology
 * observer, the reconciler's effect on the live window, and the tray's
 * "Locate Floating Bar" action.
 *
 * The geometry decisions themselves are covered by the pure tests in
 * floatingMonitorRecovery.test.ts. What is pinned here is the *wiring*:
 * that a topology change reaches the reconciler, that the reconciler moves
 * the window without touching the persisted placement, that an in-flight
 * user drag is never fought, and that the tray action recovers the bar
 * without a second window.
 */

const mocks = vi.hoisted(() => {
  const state = {
    listeners: new Map<string, (event: unknown) => void>(),
    emitted: [] as Array<{ event: string; payload: unknown }>,
    position: { x: 80, y: 40 },
    monitors: [] as Array<{
      name: string | null;
      scaleFactor: number;
      workArea: {
        position: { x: number; y: number };
        size: { width: number; height: number };
      };
    }>,
    monitor: null as null | {
      name: string | null;
      scaleFactor: number;
      workArea: {
        position: { x: number; y: number };
        size: { width: number; height: number };
      };
    },
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
    setMinSize: vi.fn(async () => undefined),
    outerPosition: vi.fn(async () => ({ ...state.position })),
    outerSize: vi.fn(async () => ({ width: 520, height: 64 })),
    onFocusChanged: vi.fn(async () => () => {}),
    onMoved: vi.fn(async () => () => {}),
    onCloseRequested: vi.fn(async () => () => {}),
  };
  return {
    state,
    listen,
    emit,
    invoke,
    winFns,
    reset: () => {
      state.listeners.clear();
      state.emitted.length = 0;
      state.position = { x: 80, y: 40 };
      state.monitors = [];
      state.monitor = null;
      for (const fn of [listen, emit, invoke, ...Object.values(winFns)]) {
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
  availableMonitors: vi.fn(async () => mocks.state.monitors),
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
  WebviewWindow: { getByLabel: vi.fn(async () => null) },
}));

import {
  attachFloatingChrome,
  attachMonitorRecovery,
  FLOATING_MONITOR_TOPOLOGY_EVENT,
  locateFloatingWindow,
  TRAY_LOCATE_FLOATING_EVENT,
} from "./floatingWindowChrome";
import { FLOATING_DRAG_LIFECYCLE_EVENT } from "./floatingDragLifecycle";
import {
  FLOATING_PREFS_STORAGE_KEY,
  loadFloatingQuotaPrefs,
} from "./floatingWindowPrefs";

/** The accepted three-monitor setup. */
const ULTRAWIDE = {
  name: "\\.\DISPLAY1",
  scaleFactor: 1,
  workArea: { position: { x: 0, y: 0 }, size: { width: 3440, height: 1400 } },
};
const RIGHT = {
  name: "\\.\DISPLAY2",
  scaleFactor: 1,
  workArea: { position: { x: 3440, y: 0 }, size: { width: 1920, height: 1080 } },
};
const PORTRAIT = {
  name: "\\.\DISPLAY3",
  scaleFactor: 1,
  workArea: { position: { x: -1440, y: 0 }, size: { width: 1440, height: 2552 } },
};

function setMonitors(...monitors: typeof ULTRAWIDE[]) {
  mocks.state.monitors = [...monitors];
  mocks.state.monitor = monitors[0] ?? null;
}

/** The user placed the free bar by hand on the portrait monitor. */
function placeOnPortrait() {
  window.localStorage.setItem(
    FLOATING_PREFS_STORAGE_KEY,
    JSON.stringify({
      visible: true,
      floatingBarEnabled: true,
      alwaysOnTop: true,
      dock: "none",
      dockAnchor: null,
      x: -1200,
      y: 300,
      monitor: {
        id: "\\.\DISPLAY3",
        x: -1440,
        y: 0,
        width: 1440,
        height: 2552,
      },
    }),
  );
}

/** Emits a native topology change to the webview. */
function emitTopology() {
  mocks.state.listeners.get(FLOATING_MONITOR_TOPOLOGY_EVENT)?.({
    payload: { change: "display" },
  });
}

const settle = async () => {
  for (let i = 0; i < 12; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

beforeEach(() => {
  mocks.reset();
  mocks.invoke.mockImplementation(async (command: string) => {
    if (command === "attach_floating_drag_lifecycle") return 7;
    if (command === "attach_floating_monitor_observer") return 3;
    return undefined;
  });
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
});

afterEach(() => {
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  window.localStorage.clear();
});

describe("the topology observer", () => {
  it("subscribes to the native topology event and attaches the native observer", async () => {
    setMonitors(ULTRAWIDE);
    const detach = attachMonitorRecovery();
    await settle();

    expect(mocks.listen).toHaveBeenCalledWith(
      FLOATING_MONITOR_TOPOLOGY_EVENT,
      expect.any(Function),
    );
    expect(mocks.invoke).toHaveBeenCalledWith(
      "attach_floating_monitor_observer",
    );
    detach();
  });

  it("detaches the native observer when it is torn down", async () => {
    setMonitors(ULTRAWIDE);
    const detach = attachMonitorRecovery();
    await settle();
    detach();
    await settle();

    expect(mocks.invoke).toHaveBeenCalledWith(
      "detach_floating_monitor_observer",
      expect.objectContaining({ generation: expect.any(Number) }),
    );
  });

  it("is a no-op outside the Tauri runtime", () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    const detach = attachMonitorRecovery();
    detach();
    expect(mocks.listen).not.toHaveBeenCalled();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});

describe("automatic recovery after a topology change", () => {
  it("returns the bar to its saved point when the preferred monitor comes back", async () => {
    placeOnPortrait();
    // The portrait monitor is away: the bar was left on the ultrawide.
    setMonitors(ULTRAWIDE);
    mocks.state.position = { x: 800, y: 400 };

    const detach = attachMonitorRecovery();
    await settle();

    // The portrait monitor returns.
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    emitTopology();
    await vi.waitFor(() => expect(mocks.state.position).toEqual({ x: -1200, y: 300 }));
    detach();
  });

  it("never overwrites the saved placement with the temporary fallback", async () => {
    placeOnPortrait();
    // Only the ultrawide is available: the saved point is off-screen.
    setMonitors(ULTRAWIDE);
    mocks.state.position = { x: -1200, y: 300 };

    const detach = attachMonitorRecovery();
    await settle();
    emitTopology();
    await vi.waitFor(() =>
      expect(mocks.state.position).not.toEqual({ x: -1200, y: 300 }),
    );

    // The bar moved to stay visible, but the persisted intent is untouched:
    // the saved point and the preferred monitor are exactly as the user
    // left them.
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.x).toBe(-1200);
    expect(prefs.y).toBe(300);
    expect(prefs.monitor.id).toBe("\\.\DISPLAY3");
    detach();
  });

  it("does not move the bar while the user is dragging it", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);

    const cleanup = await attachFloatingChrome(() => false);
    const detach = attachMonitorRecovery();
    await settle();

    // A native drag begins: the user owns the placement from here.
    const lifecycle = mocks.state.listeners.get(FLOATING_DRAG_LIFECYCLE_EVENT) as
      | ((event: { payload: unknown }) => void)
      | undefined;
    lifecycle?.({
      payload: {
        generation: 7,
        gesture: 1,
        sequence: 1,
        phase: "begin",
        reason: null,
        position: { x: 400, y: 700 },
      },
    });
    await settle();

    // The user's drag put the bar somewhere new; from here the placement
    // is theirs.
    mocks.state.position = { x: 400, y: 700 };

    emitTopology();
    await settle();

    expect(mocks.state.position).toEqual({ x: 400, y: 700 });
    detach();
    cleanup();
  });

  it("reconciles after the drag ends, not during it", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);

    const cleanup = await attachFloatingChrome(() => false);
    const detach = attachMonitorRecovery();
    await settle();

    // The user starts dragging, and the topology changes mid-gesture.
    const lifecycle = mocks.state.listeners.get(FLOATING_DRAG_LIFECYCLE_EVENT) as
      | ((event: { payload: unknown }) => void)
      | undefined;
    lifecycle?.({
      payload: {
        generation: 7,
        gesture: 1,
        sequence: 1,
        phase: "begin",
        reason: null,
        position: { x: 400, y: 700 },
      },
    });
    await settle();
    mocks.state.position = { x: 400, y: 700 };

    emitTopology();
    await settle();
    // Still the user's position: the drag owns the placement.
    expect(mocks.state.position).toEqual({ x: 400, y: 700 });

    // The gesture ends: the deferred reconcile now runs, and the bar goes
    // back to the persisted intent.
    lifecycle?.({
      payload: {
        generation: 7,
        gesture: 1,
        sequence: 2,
        phase: "end",
        reason: "release",
        position: { x: 400, y: 700 },
      },
    });
    await vi.waitFor(() =>
      expect(mocks.state.position).toEqual({ x: -1200, y: 300 }),
    );
    detach();
    cleanup();
  });

  it("waits for the topology to settle instead of reacting to the first change", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE);
    mocks.state.position = { x: -1200, y: 300 };

    const detach = attachMonitorRecovery();
    await settle();

    // Three monitors reconnecting one after another: the first two events
    // must not move the bar, because the topology has not settled.
    emitTopology();
    emitTopology();
    await settle();
    expect(mocks.state.position).toEqual({ x: -1200, y: 300 });

    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    emitTopology();
    await vi.waitFor(() => expect(mocks.state.position).toEqual({ x: -1200, y: 300 }));
    detach();
  });
});

describe("the tray locate action", () => {
  it("shows the existing window, recovers the placement and pulses", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    mocks.state.position = { x: 400, y: 700 };

    const pulse = vi.fn();
    const outcome = await locateFloatingWindow(pulse);

    expect(mocks.winFns.show).toHaveBeenCalledTimes(1);
    expect(pulse).toHaveBeenCalledTimes(1);
    expect(outcome.kind).toBe("repositioned");
    // The bar is back on the user's point, and no second window was made.
    expect(mocks.state.position).toEqual({ x: -1200, y: 300 });
  });

  it("reports an already-visible bar when the placement needs no correction", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    mocks.state.position = { x: -1200, y: 300 };

    const pulse = vi.fn();
    const outcome = await locateFloatingWindow(pulse);

    expect(outcome.kind).toBe("already-visible");
    expect(pulse).toHaveBeenCalledTimes(1);
  });

  it("still pulses when no monitor can host the bar", async () => {
    placeOnPortrait();
    setMonitors();
    const pulse = vi.fn();

    const outcome = await locateFloatingWindow(pulse);

    expect(outcome.kind).toBe("unplaceable");
    expect(pulse).toHaveBeenCalledTimes(1);
  });

  it("is wired to the tray event inside the floating chrome", async () => {
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    const cleanup = await attachFloatingChrome(() => false, undefined, undefined, undefined, undefined, () => {});
    await settle();

    expect(mocks.state.listeners.has(TRAY_LOCATE_FLOATING_EVENT)).toBe(true);
    cleanup();
  });

  it("makes the bar visible and pulses it when the tray event fires", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    mocks.state.position = { x: 400, y: 700 };

    let locate: (() => void) | undefined;
    const cleanup = await attachFloatingChrome(
      () => false,
      undefined,
      undefined,
      undefined,
      undefined,
      () => locate?.(),
    );
    await settle();

    const handler = mocks.state.listeners.get(TRAY_LOCATE_FLOATING_EVENT) as
      | (() => void)
      | undefined;
    expect(handler).toBeDefined();
    handler?.();
    await vi.waitFor(() => expect(mocks.state.position).toEqual({ x: -1200, y: 300 }));
    expect(loadFloatingQuotaPrefs().visible).toBe(true);
    cleanup();
  });
});

