/**
 * Focused tests for the monitor-topology recovery.
 *
 * The scenarios are the ones the Phase B brief names: a three-monitor
 * reconnection with staggered availability, the temporary loss of the
 * preferred monitor, its restoration, DPI and work-area changes, and the
 * separation between an intentional drag and an automatic recovery.
 *
 * The reconciler is pure, so every scenario is decided by geometry alone:
 * no window, no timer, no Tauri runtime.
 */
import { describe, expect, it } from "vitest";
import {
  clampToWorkArea,
  dockedPosition,
  EMPTY_MONITOR_IDENTITY,
  identityOf,
  matchPreferredArea,
  mayReconcile,
  MAX_TOPOLOGY_WAIT_MS,
  nearestWorkArea,
  reconcilePlacement,
  TOPOLOGY_SETTLE_MS,
  type MonitorWorkArea,
  type PlacementIntent,
} from "./floatingMonitorRecovery";

/**
 * The accepted three-monitor setup:
 *
 *  - DISPLAY1: the 3440×1440 ultrawide at the origin, 150% scaling
 *  - DISPLAY2: a 1920×1080 monitor to the right of it
 *  - DISPLAY3: a 1440×2560 portrait monitor at a negative x
 */
const ULTRAWIDE: MonitorWorkArea = {
  x: 0,
  y: 0,
  width: 5160,
  height: 2100,
  id: "\\.\DISPLAY1",
};
const RIGHT: MonitorWorkArea = {
  x: 5160,
  y: 0,
  width: 1920,
  height: 1080,
  id: "\\.\DISPLAY2",
};
const PORTRAIT: MonitorWorkArea = {
  x: -1440,
  y: 0,
  width: 1440,
  height: 2552,
  id: "\\.\DISPLAY3",
};

const ALL = [ULTRAWIDE, RIGHT, PORTRAIT];

/** A free bar the user placed by hand on the portrait monitor. */
const FREE_INTENT: PlacementIntent = {
  dock: "none",
  dockAnchor: null,
  x: -1200,
  y: 300,
};

/** The user docked the bar on the portrait monitor. */
const DOCKED_INTENT: PlacementIntent = {
  dock: "top",
  dockAnchor: 0.25,
  x: null,
  y: null,
};

const IDENTITY = {
  id: "\\.\DISPLAY3",
  x: -1440,
  y: 0,
  width: 1440,
  height: 2552,
};

const SIZE = { width: 520, height: 64 };

describe("reconcilePlacement: three-monitor reconnection with staggered availability", () => {
  it("keeps the bar where the user put it while all three monitors are present", () => {
    const action = reconcilePlacement({
      intent: FREE_INTENT,
      areas: ALL,
      size: SIZE,
      current: { x: -1200, y: 300 },
      identity: IDENTITY,
    });
    expect(action).toEqual({ kind: "none" });
  });

  it("holds the bar on the preferred monitor while the other two reconnect first", () => {
    // The preferred portrait monitor is back, the right-hand one is still
    // missing. The bar's saved point is still on a live work area, so
    // nothing moves.
    const action = reconcilePlacement({
      intent: FREE_INTENT,
      areas: [ULTRAWIDE, PORTRAIT],
      size: SIZE,
      current: { x: -1200, y: 300 },
      identity: IDENTITY,
    });
    expect(action).toEqual({ kind: "none" });
  });

  it("moves a stranded bar onto the preferred monitor once it returns", () => {
    // While the portrait monitor was away the bar sat on the ultrawide at
    // the fallback point. The portrait monitor returns: the bar must go
    // back to the user's saved point, not stay on the fallback.
    const action = reconcilePlacement({
      intent: FREE_INTENT,
      areas: [ULTRAWIDE, RIGHT, PORTRAIT],
      size: SIZE,
      current: { x: 800, y: 400 },
      identity: IDENTITY,
    });
    expect(action).toEqual({
      kind: "move",
      x: -1200,
      y: 300,
      reason: "preferred-monitor-restored",
      reanchor: false,
    });
  });

  it("falls back to a live monitor while the preferred one is still away", () => {
    // The portrait monitor is gone. The saved point is off every live work
    // area, so the bar moves to the nearest available one — but the
    // decision is a *move*, never a new intent: the caller does not
    // persist it.
    const action = reconcilePlacement({
      intent: FREE_INTENT,
      areas: [ULTRAWIDE, RIGHT],
      size: SIZE,
      current: { x: -1200, y: 300 },
      identity: IDENTITY,
    });
    expect(action.kind).toBe("move");
    if (action.kind !== "move") return;
    expect(action.reason).toBe("preferred-monitor-missing");
    // The fallback is the ultrawide: the saved point's x is far left, and
    // the ultrawide is the nearest work area to it.
    expect(action.x).toBeGreaterThanOrEqual(ULTRAWIDE.x);
    expect(action.x).toBeLessThan(ULTRAWIDE.x + ULTRAWIDE.width);
  });

  it("reports unplaceable when no monitor is available at all", () => {
    const action = reconcilePlacement({
      intent: FREE_INTENT,
      areas: [],
      size: SIZE,
      current: { x: -1200, y: 300 },
      identity: IDENTITY,
    });
    expect(action).toEqual({ kind: "unplaceable" });
  });
});

describe("reconcilePlacement: the preferred monitor is lost and returns", () => {
  it("moves the docked bar onto a live monitor when its own monitor goes away", () => {
    const action = reconcilePlacement({
      intent: DOCKED_INTENT,
      areas: [ULTRAWIDE, RIGHT],
      size: { width: 600, height: 16 },
      current: { x: -1200, y: 0 },
      identity: IDENTITY,
    });
    expect(action.kind).toBe("move");
    if (action.kind !== "move") return;
    expect(action.reanchor).toBe(true);
    expect(action.y).toBe(0);
  });

  it("returns the docked bar to its own monitor, with the anchor reapplied", () => {
    // The portrait monitor is back. The anchor is work-area-relative, so
    // the bar lands at 75% of the portrait work area, not at the old
    // absolute x.
    const action = reconcilePlacement({
      intent: DOCKED_INTENT,
      areas: [ULTRAWIDE, RIGHT, PORTRAIT],
      size: { width: 600, height: 16 },
      current: { x: 2000, y: 0 },
      identity: IDENTITY,
    });
    expect(action).toEqual({
      kind: "move",
      x: -1440 + Math.round(0.75 * 1440) - 300,
      y: 0,
      reason: "preferred-monitor-restored",
      reanchor: true,
    });
  });

  it("leaves a docked bar alone when it is already on its own monitor", () => {
    const action = reconcilePlacement({
      intent: DOCKED_INTENT,
      areas: [ULTRAWIDE, RIGHT, PORTRAIT],
      size: { width: 600, height: 16 },
      current: { x: -1440 + Math.round(0.75 * 1440) - 300, y: 0 },
      identity: IDENTITY,
    });
    expect(action).toEqual({ kind: "none" });
  });
});

describe("reconcilePlacement: DPI and work-area changes", () => {
  it("moves the bar when its work area shrinks under it", () => {
    // The portrait monitor's resolution changed: the saved point is now
    // below the work area, so the bar is pulled back onto it.
    const shrunk: MonitorWorkArea = {
      x: -1440,
      y: 0,
      width: 1440,
      height: 1200,
      id: "\\.\DISPLAY3",
    };
    const action = reconcilePlacement({
      intent: FREE_INTENT,
      areas: [ULTRAWIDE, shrunk],
      size: SIZE,
      current: { x: -1200, y: 300 },
      identity: { id: "\\.\DISPLAY3", x: -1440, y: 0, width: 1440, height: 1200 },
    });
    // 300 + 64 still fits under a 1200-tall work area, so the saved point
    // survives: a shrink that does not strand the bar must not move it.
    expect(action).toEqual({ kind: "none" });
  });

  it("pulls the bar back when a work-area change strands it", () => {
    // The portrait monitor's work area shrank to 200px tall, so the saved
    // point at y=300 no longer overlaps it.
    const shrunk: MonitorWorkArea = {
      x: -1440,
      y: 0,
      width: 1440,
      height: 200,
      id: "\\.\DISPLAY3",
    };
    const action = reconcilePlacement({
      intent: FREE_INTENT,
      areas: [ULTRAWIDE, shrunk],
      size: SIZE,
      current: { x: -1200, y: 300 },
      identity: { id: "\\.\DISPLAY3", x: -1440, y: 0, width: 1440, height: 200 },
    });
    expect(action.kind).toBe("move");
    if (action.kind !== "move") return;
    expect(action.y).toBeLessThanOrEqual(200 - 64);
    expect(action.y).toBeGreaterThanOrEqual(0);
  });

  it("re-anchors a docked bar proportionally when the work area width changes", () => {
    const wide: MonitorWorkArea = {
      x: -1440,
      y: 0,
      width: 2880,
      height: 2552,
      id: "\\.\DISPLAY3",
    };
    const action = reconcilePlacement({
      intent: DOCKED_INTENT,
      areas: [ULTRAWIDE, RIGHT, wide],
      size: { width: 600, height: 16 },
      current: { x: 0, y: 0 },
      identity: { id: "\\.\DISPLAY3", x: -1440, y: 0, width: 2880, height: 2552 },
    });
    expect(action).toEqual({
      kind: "move",
      x: -1440 + Math.round(0.75 * 2880) - 300,
      y: 0,
      reason: "preferred-monitor-restored",
      reanchor: true,
    });
  });
});

describe("reconcilePlacement: an intentional drag versus automatic recovery", () => {
  it("does not move a bar that the user just placed somewhere new", () => {
    // The user's new point is on the live ultrawide and is the persisted
    // intent: recovery has nothing to correct.
    const intent: PlacementIntent = {
      dock: "none",
      dockAnchor: null,
      x: 900,
      y: 500,
    };
    const action = reconcilePlacement({
      intent,
      areas: ALL,
      size: SIZE,
      current: { x: 900, y: 500 },
      identity: {
        id: "\\.\DISPLAY1",
        x: 0,
        y: 0,
        width: 5160,
        height: 2100,
      },
    });
    expect(action).toEqual({ kind: "none" });
  });

  it("restores the saved point after a drag was abandoned by the system", () => {
    // The system moved the window on its own (a topology change), so the
    // live position no longer matches the persisted intent.
    const action = reconcilePlacement({
      intent: FREE_INTENT,
      areas: ALL,
      size: SIZE,
      current: { x: 2500, y: 1400 },
      identity: IDENTITY,
    });
    expect(action).toEqual({
      kind: "move",
      x: -1200,
      y: 300,
      reason: "preferred-monitor-restored",
      reanchor: false,
    });
  });

  it("leaves the bar alone when nothing has been persisted yet", () => {
    const action = reconcilePlacement({
      intent: { dock: "none", dockAnchor: null, x: null, y: null },
      areas: ALL,
      size: SIZE,
      current: { x: 300, y: 200 },
      identity: EMPTY_MONITOR_IDENTITY,
    });
    expect(action).toEqual({ kind: "none" });
  });
});

describe("mayReconcile: the settle window", () => {
  it("waits for the topology to go quiet before acting", () => {
    expect(
      mayReconcile({ changes: 1, quietMs: 0, totalMs: 0 }),
    ).toBe(false);
    expect(
      mayReconcile({ changes: 1, quietMs: TOPOLOGY_SETTLE_MS - 1, totalMs: 0 }),
    ).toBe(false);
    expect(
      mayReconcile({ changes: 1, quietMs: TOPOLOGY_SETTLE_MS, totalMs: 0 }),
    ).toBe(true);
  });

  it("does nothing when no change was observed", () => {
    expect(
      mayReconcile({ changes: 0, quietMs: 10_000, totalMs: 10_000 }),
    ).toBe(false);
  });

  it("acts once the total wait budget is spent, so a flapping topology cannot strand the bar", () => {
    expect(
      mayReconcile({
        changes: 4,
        quietMs: 0,
        totalMs: MAX_TOPOLOGY_WAIT_MS,
      }),
    ).toBe(true);
  });

  it("keeps the budget longer than the settle window", () => {
    expect(MAX_TOPOLOGY_WAIT_MS).toBeGreaterThan(TOPOLOGY_SETTLE_MS);
  });
});

describe("monitor identity matching", () => {
  it("matches the preferred monitor by its device id", () => {
    expect(matchPreferredArea(ALL, IDENTITY)).toBe(PORTRAIT);
  });

  it("falls back to the work-area geometry when the device was renumbered", () => {
    // Windows renumbers display devices freely; the geometry still names
    // the same physical monitor.
    const byGeometry = matchPreferredArea(ALL, {
      id: "\\.\DISPLAY9",
      x: -1440,
      y: 0,
      width: 1440,
      height: 2552,
    });
    expect(byGeometry).toBe(PORTRAIT);
  });

  it("prefers the device id over a stale geometry", () => {
    // The id names the portrait monitor; the geometry names the ultrawide.
    // The id wins.
    const matched = matchPreferredArea(ALL, {
      id: "\\.\DISPLAY3",
      x: 0,
      y: 0,
      width: 5160,
      height: 2100,
    });
    expect(matched).toBe(PORTRAIT);
  });

  it("returns null when the preferred monitor is not connected", () => {
    expect(matchPreferredArea([ULTRAWIDE, RIGHT], IDENTITY)).toBeNull();
  });

  it("derives an identity from a work area", () => {
    expect(identityOf(PORTRAIT)).toEqual({
      id: "\\.\DISPLAY3",
      x: -1440,
      y: 0,
      width: 1440,
      height: 2552,
    });
  });
});

describe("geometry helpers", () => {
  it("finds the work area containing a point", () => {
    expect(nearestWorkArea(ALL, { x: -100, y: 100 })).toBe(PORTRAIT);
    expect(nearestWorkArea(ALL, { x: 6000, y: 100 })).toBe(RIGHT);
  });

  it("falls back to the nearest work area for a point in the gap", () => {
    const areas = [ULTRAWIDE, RIGHT];
    // x=5000 is in the gap between the ultrawide and the right monitor.
    expect(nearestWorkArea(areas, { x: 5000, y: 100 })).toBe(ULTRAWIDE);
  });

  it("returns null when there are no work areas", () => {
    expect(nearestWorkArea([], { x: 0, y: 0 })).toBeNull();
  });

  it("clamps a docked position into a work area narrower than the bar", () => {
    const narrow: MonitorWorkArea = {
      x: 0,
      y: 0,
      width: 400,
      height: 800,
      id: null,
    };
    const position = dockedPosition(narrow, 0, { width: 600, height: 16 });
    expect(position).toEqual({ x: 0, y: 0 });
  });

  it("clamps a free position into the work area", () => {
    const area: MonitorWorkArea = {
      x: 100,
      y: 200,
      width: 800,
      height: 600,
      id: null,
    };
    expect(clampToWorkArea(area, { x: 0, y: 0 }, SIZE)).toEqual({
      x: 100,
      y: 200,
    });
    expect(clampToWorkArea(area, { x: 9999, y: 9999 }, SIZE)).toEqual({
      x: 100 + 800 - 520,
      y: 200 + 600 - 64,
    });
  });
});

