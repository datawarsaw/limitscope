/**
 * Monitor-topology recovery for the floating bar.
 *
 * The bar's placement is persisted (free x/y, or a work-area-relative dock
 * anchor) but the *monitor* it belongs to is not. After a display sleep or
 * a staggered three-monitor reconnection the bar can end up on a monitor
 * the user never chose, or off-screen entirely, while the persisted
 * placement still describes the intended one.
 *
 * Two rules make the recovery safe:
 *
 * 1. The persisted placement is the intent. A temporary fallback position
 *    (the monitor that happens to be available while the preferred one is
 *    still waking up) is never written back over it.
 * 2. Recovery is a *reconciliation*, not a policy: it moves the window to
 *    where the intent says, and it never overwrites the intent itself.
 *
 * All geometry is physical pixels, like the rest of the floating-window
 * subsystem. Nothing here knows the bar's width: the caller passes the
 * measured size, so a future dynamic width (Phase A) needs no change here.
 */
import { clampDockAnchor } from "./floatingWindowPrefs";
import { clampWindowBoundsToWorkArea } from "./windowBounds";

/** One monitor's work area, in physical pixels. */
export type MonitorWorkArea = {
  x: number;
  y: number;
  width: number;
  height: number;
  /**
   * The platform's monitor identity (the Windows device name, e.g.
   * "\\.\DISPLAY1"). Null when the platform reports no name. This is
   * what makes "the user's preferred monitor" identifiable across a
   * reconnection: a coordinate can move when a monitor is re-added, but a
   * device name does not.
   */
  id: string | null;
};

/**
 * The identity of the monitor the user placed the bar on. The device name
 * is the precise key; the work-area geometry is the fallback for a
 * topology where Windows renumbered the display devices.
 */
export type MonitorIdentity = {
  id: string | null;
  x: number | null;
  y: number | null;
  width: number | null;
  height: number | null;
};

/** The bar's persisted intent, as read from the floating prefs. */
export type PlacementIntent = {
  /** "top" = docked to a monitor's top work-area edge; "none" = free. */
  dock: "none" | "top";
  /** The dock's horizontal anchor fraction; ignored while free. */
  dockAnchor: number | null;
  /** The free position in physical pixels; null when never stored. */
  x: number | null;
  y: number | null;
};

/** The window size used for the visibility test, in physical pixels. */
export type WindowSize = { width: number; height: number };

/** What the reconciler decided to do. */
export type RecoveryAction =
  /** Nothing to do: the placement already matches the intent. */
  | { kind: "none" }
  /**
   * The window must move to these physical coordinates. The intent is
   * unchanged — the caller moves the window and leaves the persisted
   * placement alone.
   */
  | {
      kind: "move";
      x: number;
      y: number;
      reason: RecoveryReason;
      /**
       * True when the docked placement must be re-derived afterwards: the
       * dock's exact position is owned by the existing anchoring path, so
       * this module only moves the bar onto the right monitor and lets
       * that path place it.
       */
      reanchor: boolean;
    }
  /**
   * No monitor could host the window at all. The tray locator is the
   * user's way out; nothing is moved.
   */
  | { kind: "unplaceable" };

export type RecoveryReason =
  /** The intended monitor is available again: the bar returns to it. */
  | "preferred-monitor-restored"
  /** The intended monitor is gone; the bar moves to a fallback monitor. */
  | "preferred-monitor-missing"
  /** The intended position no longer overlaps any work area. */
  | "off-screen"
  /** The work area or scale changed under the current placement. */
  | "work-area-changed";

/**
 * How long the reconciler waits before it acts on a topology change.
 *
 * Three monitors reconnecting at slightly different moments produce a
 * burst of topology events. Reconciling on the first one would move the
 * bar to a fallback monitor and then again when the preferred monitor
 * returns — a visible flicker and, worse, a chance to persist the
 * fallback. A short settle window lets the topology reach its final shape
 * first.
 */
export const TOPOLOGY_SETTLE_MS = 400;

/**
 * The total time the reconciler will wait before it must act. A
 * permanently flapping topology (a monitor that reconnects in a loop) must
 * not strand the bar: after this budget the reconciler acts on the best
 * information it has.
 */
export const MAX_TOPOLOGY_WAIT_MS = TOPOLOGY_SETTLE_MS * 8;

/** How much of the bar must overlap a work area to count as visible. */
const MIN_VISIBLE_OVERLAP = 48;

/** The empty identity: nothing has been claimed yet. */
export const EMPTY_MONITOR_IDENTITY: MonitorIdentity = {
  id: null,
  x: null,
  y: null,
  width: null,
  height: null,
};

/**
 * The identity of a work area, for the persisted "which monitor did the
 * user pick" record.
 */
export function identityOf(area: MonitorWorkArea): MonitorIdentity {
  return {
    id: area.id,
    x: area.x,
    y: area.y,
    width: area.width,
    height: area.height,
  };
}

function overlaps(
  window: { x: number; y: number; width: number; height: number },
  area: { x: number; y: number; width: number; height: number },
): boolean {
  const overlapW =
    Math.min(window.x + window.width, area.x + area.width) -
    Math.max(window.x, area.x);
  const overlapH =
    Math.min(window.y + window.height, area.y + area.height) -
    Math.max(window.y, area.y);
  return overlapW >= MIN_VISIBLE_OVERLAP && overlapH >= MIN_VISIBLE_OVERLAP;
}

/**
 * The work area that contains a point, or the nearest one to it. Never
 * assumes the primary monitor: with three monitors the "nearest" is the
 * one the user was actually looking at.
 */
export function nearestWorkArea(
  areas: readonly MonitorWorkArea[],
  point: { x: number; y: number },
): MonitorWorkArea | null {
  if (areas.length === 0) return null;
  const contains = areas.find(
    (area) =>
      point.x >= area.x &&
      point.x < area.x + area.width &&
      point.y >= area.y &&
      point.y < area.y + area.height,
  );
  if (contains) return contains;
  let best = areas[0];
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const area of areas) {
    const dx =
      point.x < area.x
        ? area.x - point.x
        : point.x >= area.x + area.width
          ? point.x - (area.x + area.width)
          : 0;
    const dy =
      point.y < area.y
        ? area.y - point.y
        : point.y >= area.y + area.height
          ? point.y - (area.y + area.height)
          : 0;
    const distance = dx * dx + dy * dy;
    if (distance < bestDistance) {
      best = area;
      bestDistance = distance;
    }
  }
  return best;
}

/**
 * The work area of the monitor the user placed the bar on.
 *
 * The device name wins: it survives a reconnection unchanged. The work-area
 * geometry is the fallback for a topology where Windows renumbered the
 * display devices, which happens when a different monitor claims the same
 * index. Null when the preferred monitor is not currently connected.
 */
export function matchPreferredArea(
  areas: readonly MonitorWorkArea[],
  identity: MonitorIdentity,
): MonitorWorkArea | null {
  if (identity.id) {
    const byId = areas.find((area) => area.id === identity.id);
    if (byId) return byId;
  }
  if (
    identity.x !== null &&
    identity.y !== null &&
    identity.width !== null &&
    identity.height !== null
  ) {
    const byGeometry = areas.find(
      (area) =>
        area.x === identity.x &&
        area.y === identity.y &&
        area.width === identity.width &&
        area.height === identity.height,
    );
    if (byGeometry) return byGeometry;
  }
  return null;
}

/**
 * Where the docked bar sits on a work area, for the given anchor fraction
 * and the window's measured physical size.
 *
 * The anchor is work-area-relative, so a different monitor re-anchors
 * proportionally instead of clamping to an edge. The window is clamped
 * into the work area as one unit, so a work area narrower than the bar
 * still hosts it.
 */
export function dockedPosition(
  area: MonitorWorkArea,
  anchor: number | null,
  size: WindowSize,
): { x: number; y: number } {
  // The same shared anchor clamp and work-area clamp the docked
  // placement path uses, so the two can never disagree about where the
  // bar belongs.
  const fraction = clampDockAnchor(anchor === null ? 0 : anchor);
  const center = area.x + (0.5 + fraction) * area.width;
  const clamped = clampWindowBoundsToWorkArea(
    {
      x: Math.round(center - size.width / 2),
      y: area.y,
      width: size.width,
      height: size.height,
    },
    {
      x: area.x,
      y: area.y,
      width: area.width,
      height: area.height,
    },
  );
  return { x: clamped.bounds.x, y: clamped.bounds.y };
}

/**
 * Where a free-positioned bar should sit so that it stays grabbable on the
 * given work area. The saved point is kept when it still overlaps the work
 * area; otherwise the bar is pulled inside it.
 */
export function clampToWorkArea(
  area: MonitorWorkArea,
  point: { x: number; y: number },
  size: WindowSize,
): { x: number; y: number } {
  const width = Math.max(1, Math.min(size.width, area.width));
  const height = Math.max(1, Math.min(size.height, area.height));
  return {
    x: Math.round(
      Math.min(Math.max(point.x, area.x), area.x + area.width - width),
    ),
    y: Math.round(
      Math.min(Math.max(point.y, area.y), area.y + area.height - height),
    ),
  };
}

/**
 * Decides where the bar should be, given the intent, the monitors that are
 * currently available, the window's measured size and the monitor the user
 * last placed it on.
 *
 * The decision never mutates the intent. The caller applies the returned
 * action and leaves the persisted placement alone, so a temporary fallback
 * is never written back over the user's intended monitor.
 */
export function reconcilePlacement(input: {
  intent: PlacementIntent;
  areas: readonly MonitorWorkArea[];
  size: WindowSize;
  /** The window's current physical position. */
  current: { x: number; y: number };
  /** The monitor the user last placed the bar on. */
  identity: MonitorIdentity;
}): RecoveryAction {
  const { intent, areas, size, current, identity } = input;
  if (areas.length === 0) return { kind: "unplaceable" };
  const preferred = matchPreferredArea(areas, identity);

  if (intent.dock === "top") {
    // The docked bar's target is derived, never stored: the anchor plus the
    // monitor it belongs to. When the preferred monitor is gone the bar
    // follows the window, which is where the OS already put it.
    const target = preferred ?? nearestWorkArea(areas, current);
    if (!target) return { kind: "unplaceable" };
    const position = dockedPosition(target, intent.dockAnchor, size);
    if (position.x === current.x && position.y === current.y) {
      return { kind: "none" };
    }
    return {
      kind: "move",
      x: position.x,
      y: position.y,
      reason: preferred ? "preferred-monitor-restored" : "work-area-changed",
      reanchor: true,
    };
  }

  // Free-positioned: the stored point is the intent.
  if (intent.x === null || intent.y === null) {
    // Nothing was ever stored; leave the window where the system put it.
    return { kind: "none" };
  }
  const saved = { x: intent.x, y: intent.y };
  const savedIsVisible = areas.some((area) =>
    overlaps(
      { x: saved.x, y: saved.y, width: size.width, height: size.height },
      { x: area.x, y: area.y, width: area.width, height: area.height },
    ),
  );
  if (savedIsVisible) {
    if (saved.x === current.x && saved.y === current.y) return { kind: "none" };
    return {
      kind: "move",
      x: saved.x,
      y: saved.y,
      reason: preferred ? "preferred-monitor-restored" : "work-area-changed",
      reanchor: false,
    };
  }

  // The saved point is on a monitor that is currently gone. Move the window
  // to the preferred monitor if it is back, else to the monitor nearest the
  // saved point — but never save that fallback over the intent.
  const target = preferred ?? nearestWorkArea(areas, saved);
  if (!target) return { kind: "unplaceable" };
  const position = clampToWorkArea(target, saved, size);
  return {
    kind: "move",
    x: position.x,
    y: position.y,
    reason: preferred ? "preferred-monitor-restored" : "preferred-monitor-missing",
    reanchor: false,
  };
}

/**
 * Whether the reconciler may act on the topology change it has seen.
 *
 * It acts when the topology has been quiet for a settle window, or when
 * the total wait budget is spent — a flapping topology must not strand the
 * bar. It waits otherwise, so a burst of reconnects is reconciled once,
 * against the settled topology.
 */
export function mayReconcile(input: {
  /** How many topology changes have been seen since the last reconcile. */
  changes: number;
  /** Milliseconds since the most recent change. */
  quietMs: number;
  /** Milliseconds since the first change of this burst. */
  totalMs: number;
}): boolean {
  if (input.changes === 0) return false;
  if (input.quietMs >= TOPOLOGY_SETTLE_MS) return true;
  return input.totalMs >= MAX_TOPOLOGY_WAIT_MS;
}

