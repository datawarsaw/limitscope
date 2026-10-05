/**
 * Work-area bounds math for W01 (v0.8): one pure clamp used by both window
 * application paths — the main window (attach + DPI/focus re-checks) and the
 * floating bar's overlay correction. All values are physical pixels; the
 * caller converts from whatever the window/monitor APIs returned.
 *
 * The clamp is deliberately conservative: it never enlarges, preserves the
 * user's size whenever it fits the work area, and only shrinks/repositions
 * when the bounds would otherwise be stranded off-screen. Negative work-area
 * coordinates (monitors left of / above the primary) are supported by plain
 * arithmetic — no clamping to zero.
 */

export type BoundsRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type ClampedBounds = {
  /** The clamped outer bounds; identical to the input when `changed` is false. */
  bounds: BoundsRect;
  /** False when the input already fit the work area and is returned untouched. */
  changed: boolean;
};

function isUsableRect(rect: BoundsRect): boolean {
  return (
    Number.isFinite(rect.x) &&
    Number.isFinite(rect.y) &&
    Number.isFinite(rect.width) &&
    Number.isFinite(rect.height) &&
    rect.width > 0 &&
    rect.height > 0
  );
}

/**
 * Clamps a window's outer bounds into a monitor work area. Size shrinks only
 * when the window exceeds the work area on that axis; position pulls in only
 * when the (possibly shrunk) bounds poke out of it.
 */
export function clampWindowBoundsToWorkArea(
  window: BoundsRect,
  workArea: BoundsRect,
): ClampedBounds {
  if (!isUsableRect(window) || !isUsableRect(workArea)) {
    return { bounds: { ...window }, changed: false };
  }

  // width/height never exceed the work area here, so the position clamp
  // interval is always non-empty (rightLimit >= workArea.x).
  const width = Math.min(window.width, workArea.width);
  const height = Math.min(window.height, workArea.height);
  const x = Math.min(
    Math.max(window.x, workArea.x),
    workArea.x + workArea.width - width,
  );
  const y = Math.min(
    Math.max(window.y, workArea.y),
    workArea.y + workArea.height - height,
  );

  const changed =
    x !== window.x ||
    y !== window.y ||
    width !== window.width ||
    height !== window.height;
  return { bounds: { x, y, width, height }, changed };
}
