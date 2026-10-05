import { clampDockAnchor } from "./floatingWindowPrefs";

/**
 * Initial Smart Docking thresholds, in logical CSS pixels. Human Acceptance
 * tunes the feel; callers convert them once with the drag monitor's scale.
 * Positions and work areas stay in the physical pixels the native gesture
 * and Tauri monitor geometry already use.
 */
export const SNAP_PREVIEW_ONSET_LOGICAL_PX = 120;
export const DOCK_ELIGIBILITY_LOGICAL_PX = 40;
export const DETACH_THRESHOLD_LOGICAL_PX = 28;

export type SmartDockWorkArea = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type SmartDockMonitor = {
  scaleFactor: number;
  workArea: SmartDockWorkArea;
};

export type SmartDockPhase =
  | { kind: "floating" }
  | { kind: "snap-preview"; eligible: boolean; anchor: number; distancePx: number }
  | { kind: "docked"; anchor: number }
  | { kind: "unsnap-preview"; eligible: true; distancePx: number };

/** Content-only preview. Null means the gesture is not offering a transition. */
export type SmartDockGesture = "snap" | "snap-ready" | "unsnap";

export function physicalThresholdPx(logicalPx: number, scaleFactor: number): number {
  const scale = Number.isFinite(scaleFactor) && scaleFactor > 0 ? scaleFactor : 1;
  return Math.max(1, Math.round(logicalPx * scale));
}

export function gestureFromPhase(phase: SmartDockPhase): SmartDockGesture | null {
  if (phase.kind === "snap-preview") return phase.eligible ? "snap-ready" : "snap";
  if (phase.kind === "unsnap-preview") return "unsnap";
  return null;
}

function anchorFromCenter(centerX: number, workArea: SmartDockWorkArea): number {
  if (!Number.isFinite(centerX) || workArea.width <= 0) return 0;
  return clampDockAnchor(
    (centerX - (workArea.x + workArea.width / 2)) / workArea.width,
  );
}

/**
 * Gesture-local docking decision. stable is the persisted placement the
 * gesture began in. Preview phases do not commit FLOATING or DOCKED.
 */
export function evaluateSmartDock(input: {
  stable: "floating" | "docked";
  x: number;
  y: number;
  width: number;
  monitor: SmartDockMonitor;
}): SmartDockPhase {
  const distancePx = input.y - input.monitor.workArea.y;
  const anchor = anchorFromCenter(input.x + input.width / 2, input.monitor.workArea);
  if (input.stable === "floating") {
    const onset = physicalThresholdPx(
      SNAP_PREVIEW_ONSET_LOGICAL_PX,
      input.monitor.scaleFactor,
    );
    if (distancePx > onset) return { kind: "floating" };
    const eligibility = physicalThresholdPx(
      DOCK_ELIGIBILITY_LOGICAL_PX,
      input.monitor.scaleFactor,
    );
    return {
      kind: "snap-preview",
      eligible: distancePx <= eligibility,
      anchor,
      distancePx,
    };
  }
  const detach = physicalThresholdPx(
    DETACH_THRESHOLD_LOGICAL_PX,
    input.monitor.scaleFactor,
  );
  if (distancePx > detach) {
    return { kind: "unsnap-preview", eligible: true, distancePx };
  }
  return { kind: "docked", anchor };
}

function contains(area: SmartDockWorkArea, x: number, y: number): boolean {
  return x >= area.x &&
    y >= area.y &&
    x < area.x + area.width &&
    y < area.y + area.height;
}

function distanceToArea(area: SmartDockWorkArea, x: number, y: number): number {
  const dx = x < area.x
    ? area.x - x
    : x >= area.x + area.width
      ? x - (area.x + area.width)
      : 0;
  const dy = y < area.y
    ? area.y - y
    : y >= area.y + area.height
      ? y - (area.y + area.height)
      : 0;
  return dx * dx + dy * dy;
}

/**
 * The work area that contains the dragged window top-center. A point in a
 * gap uses the nearest work area. Never assumes the primary monitor.
 */
export function selectDragMonitor<T extends SmartDockMonitor>(
  monitors: readonly T[],
  point: { x: number; y: number },
): T | null {
  if (monitors.length === 0) return null;
  const hit = monitors.find((monitor) => contains(monitor.workArea, point.x, point.y));
  if (hit) return hit;
  let best = monitors[0];
  let bestDistance = distanceToArea(best.workArea, point.x, point.y);
  for (const monitor of monitors.slice(1)) {
    const distance = distanceToArea(monitor.workArea, point.x, point.y);
    if (distance < bestDistance) {
      best = monitor;
      bestDistance = distance;
    }
  }
  return best;
}
