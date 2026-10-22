/**
 * The floating bar's persisted placement, extended with the monitor the user
 * placed it on.
 *
 * The identity is what makes a three-monitor reconnection recoverable: the
 * persisted x/y describe a point, but a point cannot say *which* monitor it
 * belonged to. Without that, a bar whose monitor went away has no way back
 * when the monitor returns.
 *
 * The record is deliberately additive and self-describing: a prefs file
 * written before this change simply has no monitor field and parses as
 * "unknown monitor", which is the same state as a bar that has never been
 * placed on a named monitor.
 */

/** The monitor the bar was last placed on. */
export type FloatingMonitorIdentity = {
  /**
   * The platform's monitor id (the Windows device name, e.g.
   * "\\.\DISPLAY1"). Null when the platform reports no name, or when the
   * display devices were renumbered since it was recorded.
   */
  id: string | null;
  /** The work-area origin, in physical pixels, as a geometry fallback. */
  x: number | null;
  y: number | null;
  width: number | null;
  height: number | null;
};

export const EMPTY_FLOATING_MONITOR: FloatingMonitorIdentity = {
  id: null,
  x: null,
  y: null,
  width: null,
  height: null,
};

/**
 * Parses a stored monitor identity. Every field is validated on its own, so
 * a partially corrupt record degrades to the fields that are still usable
 * rather than being discarded whole.
 */
export function parseFloatingMonitorIdentity(
  raw: unknown,
): FloatingMonitorIdentity {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return { ...EMPTY_FLOATING_MONITOR };
  }
  const record = raw as Record<string, unknown>;
  const finiteOrNull = (value: unknown): number | null =>
    typeof value === "number" && Number.isFinite(value) ? value : null;
  return {
    id: typeof record.id === "string" && record.id.length > 0 ? record.id : null,
    x: finiteOrNull(record.x),
    y: finiteOrNull(record.y),
    width: finiteOrNull(record.width),
    height: finiteOrNull(record.height),
  };
}

/**
 * Whether two identities describe the same monitor. The device id is
 * authoritative; the geometry is only consulted when neither side has one.
 */
export function sameFloatingMonitor(
  a: FloatingMonitorIdentity,
  b: FloatingMonitorIdentity,
): boolean {
  if (a.id && b.id) return a.id === b.id;
  if (a.id || b.id) return false;
  if (a.x === null || a.y === null || b.x === null || b.y === null) return false;
  return a.x === b.x && a.y === b.y;
}

