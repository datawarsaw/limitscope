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
    // Production subscribes more than once to the same native event: the
    // chrome's drag-lifecycle consumer and the monitor recovery's deferred
    // reconcile both listen to FLOATING_DRAG_LIFECYCLE_EVENT. A single
    // handler per event would silently drop one of them, so the mock keeps
    // every active subscriber in registration order and dispatches to all.
    listeners: new Map<string, Set<(event: unknown) => void>>(),
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
  // Mirrors the real `listen` contract: each call adds an independent
  // subscriber and its returned unlisten removes only that subscriber. The
  // same native event therefore reaches every live handler, exactly as the
  // Rust emitter broadcasts to each registered webview listener.
  const listen = vi.fn(async (event: string, handler: (event: unknown) => void) => {
    let subscribers = state.listeners.get(event);
    if (!subscribers) {
      subscribers = new Set();
      state.listeners.set(event, subscribers);
    }
    subscribers.add(handler);
    return () => {
      const current = state.listeners.get(event);
      if (!current) return;
      current.delete(handler);
      if (current.size === 0) state.listeners.delete(event);
    };
  });
  /** Delivers one native event to every active subscriber; returns how many. */
  const dispatch = (event: string, payload?: unknown): number => {
    const subscribers = [...(state.listeners.get(event) ?? [])];
    for (const subscriber of subscribers) subscriber({ payload });
    return subscribers.length;
  };
  const listenerCount = (event: string): number =>
    state.listeners.get(event)?.size ?? 0;
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
    setFocusable: vi.fn(async () => undefined),
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
    dispatch,
    listenerCount,
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
  resizeFloatingWindow,
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

/** Emits a native topology change to every subscriber of the webview event. */
function emitTopology() {
  mocks.dispatch(FLOATING_MONITOR_TOPOLOGY_EVENT, { change: "display" });
}

/**
 * Emits one lifecycle event to every subscriber, like the native emitter.
 * Both the chrome's drag consumer and the monitor recovery's deferred
 * reconcile listen on this event; delivering to only one of them was the
 * one-listener mock defect this suite had.
 */
function emitLifecycle(payload: Record<string, unknown>) {
  mocks.dispatch(FLOATING_DRAG_LIFECYCLE_EVENT, payload);
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
  it("retries a rejected placement without another topology event", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    mocks.state.position = { x: 400, y: 700 };
    const reconciled = vi.fn();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const detach = attachMonitorRecovery(reconciled);
    await settle();
    vi.useFakeTimers();
    try {
      mocks.winFns.setPosition.mockRejectedValueOnce(new Error("transient"));
      emitTopology();
      await vi.advanceTimersByTimeAsync(600);
      expect(reconciled).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1000);
      expect(mocks.winFns.setPosition).toHaveBeenCalledTimes(2);
      expect(reconciled).toHaveBeenCalledTimes(1);
      expect(mocks.state.position).toEqual({ x: -1200, y: 300 });
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      detach();
      warn.mockRestore();
      vi.clearAllTimers();
      vi.useRealTimers();
    }
  });

  it.each([false, true])("bounds permanent failures and cancels the retry on detach: %s", async (detachEarly) => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    mocks.state.position = { x: 400, y: 700 };
    const reconciled = vi.fn();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const detach = attachMonitorRecovery(reconciled);
    await settle();
    vi.useFakeTimers();
    try {
      mocks.winFns.setPosition.mockRejectedValueOnce(new Error("permanent"));
      if (!detachEarly) mocks.winFns.setPosition.mockRejectedValueOnce(new Error("permanent"));
      emitTopology();
      await vi.advanceTimersByTimeAsync(600);
      if (detachEarly) detach();
      await vi.advanceTimersByTimeAsync(5000);
      expect(mocks.winFns.setPosition).toHaveBeenCalledTimes(detachEarly ? 1 : 2);
      expect(warn).toHaveBeenCalledTimes(detachEarly ? 1 : 2);
      expect(reconciled).not.toHaveBeenCalled();
      expect(vi.getTimerCount()).toBe(0);
      if (!detachEarly) {
        emitTopology();
        await vi.advanceTimersByTimeAsync(600);
        expect(reconciled).toHaveBeenCalledTimes(1);
      }
    } finally {
      detach();
      warn.mockRestore();
      vi.clearAllTimers();
      vi.useRealTimers();
    }
  });

  it("reconciles when a released worker becomes idle after the old polling budget", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    const reconciled = vi.fn();
    const cleanup = await attachFloatingChrome(() => false);
    const detach = attachMonitorRecovery(reconciled);
    await settle();
    mocks.winFns.setPosition.mockClear();
    vi.useFakeTimers();
    let unblock!: (value: { width: number; height: number }) => void;
    try {
      emitLifecycle({ generation: 7, gesture: 1, sequence: 1, phase: "begin", reason: null, position: { x: 400, y: 700 } });
      emitTopology();
      await vi.advanceTimersByTimeAsync(600);
      mocks.winFns.outerSize.mockImplementationOnce(() => new Promise((resolve) => { unblock = resolve; }));
      emitLifecycle({ generation: 7, gesture: 1, sequence: 2, phase: "end", reason: "release", position: { x: 400, y: 700 } });
      await vi.advanceTimersByTimeAsync(1500);
      expect(reconciled).not.toHaveBeenCalled();
      expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
      unblock({ width: 520, height: 64 });
      await vi.advanceTimersByTimeAsync(100);
      expect(reconciled).toHaveBeenCalledTimes(1);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      unblock?.({ width: 520, height: 64 });
      detach();
      cleanup();
      vi.clearAllTimers();
      vi.useRealTimers();
    }
  });
  it.each([
    ["geometry", "release"], ["geometry", "cancel"],
    ["queue", "release"], ["queue", "cancel"],
  ] as const)("retries a recovery invalidated during %s after %s", async (stage, ending) => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    mocks.state.position = { x: 400, y: 700 };
    const reconciled = vi.fn();
    const cleanup = await attachFloatingChrome(() => false);
    const detach = attachMonitorRecovery(reconciled);
    await settle();
    mocks.state.position = { x: 400, y: 700 };
    mocks.winFns.setPosition.mockClear();
    vi.useFakeTimers();
    let unblock!: () => void;
    try {
      const blocked = new Promise<void>((resolve) => { unblock = resolve; });
      if (stage === "geometry") {
        mocks.winFns.outerSize.mockImplementationOnce(async () => {
          await blocked;
          return { width: 520, height: 64 };
        });
      } else {
        mocks.winFns.setMinSize.mockImplementationOnce(async () => { await blocked; });
        void resizeFloatingWindow(520, 64, false);
        await vi.advanceTimersByTimeAsync(0);
      }
      emitTopology();
      await vi.advanceTimersByTimeAsync(600);
      emitLifecycle({ generation: 7, gesture: 1, sequence: 1, phase: "begin", reason: null, position: { x: 400, y: 700 } });
      unblock();
      await vi.advanceTimersByTimeAsync(0);
      expect(reconciled).not.toHaveBeenCalled();
      expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
      emitLifecycle({ generation: 7, gesture: 1, sequence: 2, phase: "end", reason: ending, position: { x: 400, y: 700 } });
      await vi.advanceTimersByTimeAsync(1200);
      expect(reconciled).toHaveBeenCalledTimes(1);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      unblock();
      detach();
      cleanup();
      vi.clearAllTimers();
      vi.useRealTimers();
    }
  });

  it.each(["geometry", "queue"] as const)("does not move when the observer detaches during %s", async (stage) => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
    mocks.state.position = { x: 400, y: 700 };
    const reconciled = vi.fn();
    const detach = attachMonitorRecovery(reconciled);
    await settle();
    vi.useFakeTimers();
    let unblock!: () => void;
    try {
      const blocked = new Promise<void>((resolve) => { unblock = resolve; });
      if (stage === "geometry") {
        mocks.winFns.outerSize.mockImplementationOnce(async () => {
          await blocked;
          return { width: 520, height: 64 };
        });
      } else {
        mocks.winFns.setMinSize.mockImplementationOnce(async () => { await blocked; });
        void resizeFloatingWindow(520, 64, false);
        await vi.advanceTimersByTimeAsync(0);
      }
      emitTopology();
      await vi.advanceTimersByTimeAsync(600);
      detach();
      unblock();
      await vi.advanceTimersByTimeAsync(1200);
      expect(mocks.winFns.setPosition).not.toHaveBeenCalled();
      expect(reconciled).not.toHaveBeenCalled();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      unblock();
      detach();
      vi.clearAllTimers();
      vi.useRealTimers();
    }
  });
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

    const reconciled = vi.fn();
    const cleanup = await attachFloatingChrome(() => false);
    const detach = attachMonitorRecovery(reconciled);
    await settle();

    // Production registers two independent subscribers for the lifecycle
    // event: the chrome's drag consumer and the recovery's deferred
    // reconcile. Pin that, so a regression back to one handler fails here
    // instead of silently exercising half of production.
    expect(mocks.listenerCount(FLOATING_DRAG_LIFECYCLE_EVENT)).toBe(2);

    // A native drag begins: the user owns the placement from here.
    emitLifecycle({
      generation: 7,
      gesture: 1,
      sequence: 1,
      phase: "begin",
      reason: null,
      position: { x: 400, y: 700 },
    });
    await settle();

    // The user's drag put the bar somewhere new; from here the placement
    // is theirs.
    mocks.state.position = { x: 400, y: 700 };

    emitTopology();
    // Wait past the settle window, so the recovery's timer really fires
    // while the gesture is live. (Before the mock dispatched to every
    // subscriber this test never even started a drag; it only passed
    // because the settle timer had not fired yet.)
    await new Promise((resolve) => setTimeout(resolve, 600));

    expect(reconciled).not.toHaveBeenCalled();
    expect(mocks.state.position).toEqual({ x: 400, y: 700 });
    detach();
    cleanup();
  });

  it("removes only the unsubscribed handler when one owner tears down", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);

    const cleanup = await attachFloatingChrome(() => false);
    const detach = attachMonitorRecovery();
    await settle();
    expect(mocks.listenerCount(FLOATING_DRAG_LIFECYCLE_EVENT)).toBe(2);

    // Real unlisten removes exactly one registration: the recovery goes, the
    // chrome's drag consumer stays subscribed.
    detach();
    await settle();
    expect(mocks.listenerCount(FLOATING_DRAG_LIFECYCLE_EVENT)).toBe(1);

    cleanup();
    await settle();
    expect(mocks.listenerCount(FLOATING_DRAG_LIFECYCLE_EVENT)).toBe(0);
  });

  it("reconciles after the drag ends, not during it", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);

    const reconciled = vi.fn();
    const cleanup = await attachFloatingChrome(() => false);
    const detach = attachMonitorRecovery(reconciled);
    await settle();

    expect(mocks.listenerCount(FLOATING_DRAG_LIFECYCLE_EVENT)).toBe(2);

    // The user starts dragging, and the topology changes mid-gesture.
    emitLifecycle({
      generation: 7,
      gesture: 1,
      sequence: 1,
      phase: "begin",
      reason: null,
      position: { x: 400, y: 700 },
    });
    await settle();
    mocks.state.position = { x: 400, y: 700 };

    emitTopology();
    await settle();
    // Still the user's position: the drag owns the placement. The reconcile
    // is deferred, so it is not reported yet.
    expect(mocks.state.position).toEqual({ x: 400, y: 700 });
    expect(reconciled).not.toHaveBeenCalled();

    // The gesture ends: a reconcile runs only after this point, never during
    // the gesture. The release is the user's placement, so it honors the drop
    // point instead of pulling the bar back to the pre-drag point.
    //
    // Reported, not pinned: with both subscribers live, the recovery's own
    // end handler can still observe the chrome's session as active, because
    // the chrome concludes a released drag asynchronously. The handler then
    // sets `deferred` again and nothing retries it until the *next* topology
    // change, so a topology change that settles mid-drag can be stranded. The
    // reconcile below arrives from the settle timer instead. Asserting either
    // the intended or the stranded ordering here would either fail on a real
    // product defect or bake that defect in, so the assertion stays at the
    // behavior that must hold: no reconcile mid-drag, one after the release.
    emitLifecycle({
      generation: 7,
      gesture: 1,
      sequence: 2,
      phase: "end",
      reason: "release",
      position: { x: 400, y: 700 },
    });
    await vi.waitFor(() => expect(reconciled).toHaveBeenCalled());
    await settle();
    expect(reconciled).toHaveBeenCalledTimes(1);
    expect(mocks.state.position).toEqual({ x: 400, y: 700 });
    // The release reached the chrome's consumer too: the drop point is the
    // user's new placement, which is what the deferred reconcile honored.
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.x).toBe(400);
    expect(prefs.y).toBe(700);
    detach();
    cleanup();
  });

  it("retries a topology change that settled mid-drag once the released drag is idle", async () => {
    placeOnPortrait();
    setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);

    const reconciled = vi.fn();
    const cleanup = await attachFloatingChrome(() => false);
    const detach = attachMonitorRecovery(reconciled);
    await settle();

    expect(mocks.listenerCount(FLOATING_DRAG_LIFECYCLE_EVENT)).toBe(2);

    // A native drag begins, and the topology changes mid-gesture.
    emitLifecycle({
      generation: 7,
      gesture: 1,
      sequence: 1,
      phase: "begin",
      reason: null,
      position: { x: 400, y: 700 },
    });
    await settle();
    mocks.state.position = { x: 400, y: 700 };

    emitTopology();
    // Wait beyond the settle window while still dragging, so the recovery's
    // timer fires and defers the reconcile during the gesture.
    await new Promise((resolve) => setTimeout(resolve, 600));
    expect(reconciled).not.toHaveBeenCalled();
    expect(mocks.state.position).toEqual({ x: 400, y: 700 });

    // The user releases. The chrome concludes a released drag
    // asynchronously, so at the moment the recovery's end handler runs the
    // session is still marked active — the retry must wait for that true
    // conclusion instead of deferring again. No further topology event
    // arrives: the deferred reconcile must still run exactly once.
    emitLifecycle({
      generation: 7,
      gesture: 1,
      sequence: 2,
      phase: "end",
      reason: "release",
      position: { x: 400, y: 700 },
    });
    await vi.waitFor(() => expect(reconciled).toHaveBeenCalled());
    await settle();
    expect(reconciled).toHaveBeenCalledTimes(1);
    // The release is the user's placement: the drop point stands, and the
    // reconcile honors it instead of pulling the bar back mid-drag.
    expect(mocks.state.position).toEqual({ x: 400, y: 700 });
    const prefs = loadFloatingQuotaPrefs();
    expect(prefs.x).toBe(400);
    expect(prefs.y).toBe(700);
    detach();
    cleanup();
  });

  it.each(["release", "cancel", "teardown"] as const)(
    "retains deferred recovery across a long second drag (%s)",
    async (ending) => {
      placeOnPortrait();
      setMonitors(ULTRAWIDE, RIGHT, PORTRAIT);
      const reconciled = vi.fn();
      const cleanup = await attachFloatingChrome(() => false);
      const detach = attachMonitorRecovery(reconciled);
      await settle();
      vi.useFakeTimers();
      let finishFirstRead!: (size: { width: number; height: number }) => void;
      try {
        emitLifecycle({ generation: 7, gesture: 1, sequence: 1, phase: "begin", reason: null, position: { x: 400, y: 700 } });
        emitTopology();
        await vi.advanceTimersByTimeAsync(600);
        expect(reconciled).not.toHaveBeenCalled();

        mocks.winFns.outerSize.mockImplementationOnce(() => new Promise((resolve) => {
          finishFirstRead = resolve;
        }));
        emitLifecycle({ generation: 7, gesture: 1, sequence: 2, phase: "end", reason: "release", position: { x: 400, y: 700 } });
        emitLifecycle({ generation: 7, gesture: 2, sequence: 1, phase: "begin", reason: null, position: { x: 500, y: 700 } });
        mocks.state.position = { x: 500, y: 700 };
        finishFirstRead({ width: 520, height: 64 });
        if (ending === "teardown") {
          expect(vi.getTimerCount()).toBe(1);
          detach();
          expect(vi.getTimerCount()).toBe(0);
        }
        await vi.advanceTimersByTimeAsync(1100);
        expect(reconciled).not.toHaveBeenCalled();
        expect(mocks.state.position).toEqual({ x: 500, y: 700 });
        expect(vi.getTimerCount()).toBe(0);

        emitLifecycle({ generation: 7, gesture: 2, sequence: 2, phase: "end", reason: ending === "cancel" ? "cancel" : "release", position: { x: 500, y: 700 } });
        await vi.advanceTimersByTimeAsync(100);
        expect(reconciled).toHaveBeenCalledTimes(ending === "teardown" ? 0 : 1);
        await vi.advanceTimersByTimeAsync(1200);
        expect(reconciled).toHaveBeenCalledTimes(ending === "teardown" ? 0 : 1);
      } finally {
        detach();
        cleanup();
        vi.clearAllTimers();
        vi.useRealTimers();
      }
    },
  );

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

    expect(mocks.listenerCount(TRAY_LOCATE_FLOATING_EVENT)).toBe(1);
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

    const handler = mocks.state.listeners.get(TRAY_LOCATE_FLOATING_EVENT);
    expect(handler?.size ?? 0).toBe(1);
    mocks.dispatch(TRAY_LOCATE_FLOATING_EVENT);
    await vi.waitFor(() => expect(mocks.state.position).toEqual({ x: -1200, y: 300 }));
    expect(loadFloatingQuotaPrefs().visible).toBe(true);
    cleanup();
  });
});

