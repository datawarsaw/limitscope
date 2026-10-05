/** Where the floating bar lives. "top" is the meter-only resting dock
 * anchored to the top edge of the current monitor's work area; "none" is the
 * free-floating bar. */
export type FloatingDockMode = "none" | "top";

export type FloatingQuotaPrefs = {
  visible: boolean;
  /**
   * Feature switch for the bar itself (v0.6). Independent of `visible`:
   * hiding the window is a lifecycle action that leaves this on, while
   * disabling the feature keeps geometry so re-enabling restores it.
   */
  floatingBarEnabled: boolean;
  alwaysOnTop: boolean;
  /**
   * Resting dock state. While docked the window position is derived from the
   * current monitor's work area, so `x`/`y` are not consulted; dragging the
   * dock away from the top edge clears this back to "none".
   */
  dock: FloatingDockMode;
  /**
   * The dock's horizontal anchor while `dock` is "top": the dock center's
   * offset from the work-area center as a fraction of the work-area width,
   * clamped to ±DOCK_ANCHOR_LIMIT. Work-area-relative, not absolute x, so a
   * scale or monitor change re-anchors proportionally instead of clamping to
   * an edge. null means centered. It is only read while docked; free-bar
   * `x`/`y` are never written while docked, and the anchor survives undock
   * so re-docking returns to the user's spot.
   */
  dockAnchor: number | null;
  /** Last outer position in physical pixels, when one has been stored. */
  x: number | null;
  y: number | null;
};

export type WorkArea = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export const DEFAULT_FLOATING_PREFS: FloatingQuotaPrefs = {
  visible: true,
  floatingBarEnabled: true,
  alwaysOnTop: true,
  dock: "none",
  dockAnchor: null,
  x: null,
  y: null,
};

export const FLOATING_PREFS_STORAGE_KEY = "rate-limits.floating-quota.v1";

const STORAGE_KEY = FLOATING_PREFS_STORAGE_KEY;

// Keys this schema owns; foreign keys in stored JSON survive writes.
const KNOWN_FLOATING_KEYS: ReadonlySet<string> = new Set([
  "visible",
  "floatingBarEnabled",
  "alwaysOnTop",
  "dock",
  "dockAnchor",
  "x",
  "y",
]);

function finiteOrNull(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Legal magnitude bound for the dock anchor fraction (±half the work area). */
export const DOCK_ANCHOR_LIMIT = 0.5;

/**
 * Clamps an anchor fraction into ±DOCK_ANCHOR_LIMIT; a non-finite value reads
 * as centered, so a corrupted anchor can never strand the dock off-screen.
 */
export function clampDockAnchor(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(DOCK_ANCHOR_LIMIT, Math.max(-DOCK_ANCHOR_LIMIT, value));
}

/**
 * Validates arbitrary parsed storage content into prefs. Absent or invalid
 * booleans fall back to defaults; positions fall back to null (system
 * placed) when they are not finite numbers.
 */
export function parseFloatingQuotaPrefs(raw: unknown): FloatingQuotaPrefs {
  const prefs: FloatingQuotaPrefs = { ...DEFAULT_FLOATING_PREFS };
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return prefs;
  const record = raw as Record<string, unknown>;
  if (typeof record.visible === "boolean") prefs.visible = record.visible;
  if (typeof record.floatingBarEnabled === "boolean") {
    prefs.floatingBarEnabled = record.floatingBarEnabled;
  }
  if (typeof record.alwaysOnTop === "boolean") prefs.alwaysOnTop = record.alwaysOnTop;
  // Anything but the exact "top" mode reads as the free-floating bar, so a
  // corrupted value can never dock the window at startup.
  if (record.dock === "top") prefs.dock = "top";
  const dockAnchor = finiteOrNull(record.dockAnchor);
  prefs.dockAnchor = dockAnchor === null ? null : clampDockAnchor(dockAnchor);
  prefs.x = finiteOrNull(record.x);
  prefs.y = finiteOrNull(record.y);
  return prefs;
}

export function loadFloatingQuotaPrefs(): FloatingQuotaPrefs {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return parseFloatingQuotaPrefs(null);
    return parseFloatingQuotaPrefs(JSON.parse(raw));
  } catch {
    // Corrupted storage falls back to the defaults.
    return parseFloatingQuotaPrefs(null);
  }
}

/**
 * Builds the stored value for a save: keys outside the floating schema
 * pass through from the raw stored object untouched, then the validated
 * prefs overwrite their canonical keys. Corrupt or non-object storage
 * preserves nothing.
 */
export function mergeFloatingPrefsWithRaw(
  raw: string | null,
  prefs: FloatingQuotaPrefs,
): string {
  let base: Record<string, unknown> = {};
  if (raw) {
    try {
      const parsed: unknown = JSON.parse(raw);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        base = parsed as Record<string, unknown>;
      }
    } catch {
      base = {};
    }
  }
  const merged: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(base)) {
    if (!KNOWN_FLOATING_KEYS.has(key)) merged[key] = value;
  }
  merged.visible = prefs.visible;
  merged.floatingBarEnabled = prefs.floatingBarEnabled;
  merged.alwaysOnTop = prefs.alwaysOnTop;
  merged.dock = prefs.dock;
  merged.dockAnchor = prefs.dockAnchor;
  merged.x = prefs.x;
  merged.y = prefs.y;
  return JSON.stringify(merged);
}

export function saveFloatingQuotaPrefs(prefs: FloatingQuotaPrefs): void {
  try {
    localStorage.setItem(
      STORAGE_KEY,
      mergeFloatingPrefsWithRaw(localStorage.getItem(STORAGE_KEY), prefs),
    );
  } catch {
    // Persistence is best-effort; the bar still works for this session.
  }
}

/**
 * The canonical reset for the floating bar's preference class (v0.7 "Reset
 * preferences"): visibility, feature switch, pin, dock state, and geometry
 * all return to their defaults, so the next attach places the bar the way a
 * fresh install would. Like an ordinary save it merges against the raw
 * stored object, so fields this schema does not own survive.
 */
export function resetFloatingQuotaPrefs(): FloatingQuotaPrefs {
  const defaults: FloatingQuotaPrefs = { ...DEFAULT_FLOATING_PREFS };
  saveFloatingQuotaPrefs(defaults);
  return defaults;
}

export type TrailingPositionSave = {
  /** Records a moved position and restarts the settle timer. */
  schedule: (x: number, y: number) => void;
  /** Writes the pending position now, if one is waiting. */
  flush: () => void;
  /** Drops the pending position without writing. */
  cancel: () => void;
};

/**
 * Trailing-edge saver for native move events: keeps only the newest position
 * and writes it once movement has settled, instead of a localStorage write
 * per pointer event. flush() persists the final position on unmount or
 * close, so a normally ended drag is never lost.
 */
export function createTrailingPositionSave(
  delayMs: number,
  save: (x: number, y: number) => void,
): TrailingPositionSave {
  let timer: number | null = null;
  let pending: { x: number; y: number } | null = null;

  const fire = () => {
    timer = null;
    const coords = pending;
    pending = null;
    if (coords) save(coords.x, coords.y);
  };

  return {
    schedule(x, y) {
      pending = { x, y };
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(fire, delayMs);
    },
    flush() {
      if (timer === null) return;
      window.clearTimeout(timer);
      fire();
    },
    cancel() {
      if (timer !== null) {
        window.clearTimeout(timer);
        timer = null;
      }
      pending = null;
    },
  };
}

/**
 * A restored position is usable when the window still overlaps a work area
 * by enough pixels to grab. Disconnected monitors therefore drop the saved
 * point instead of stranding the bar off-screen.
 */
export function isFloatingPositionVisible(
  x: number,
  y: number,
  width: number,
  height: number,
  workAreas: readonly WorkArea[],
): boolean {
  if (!Number.isFinite(x) || !Number.isFinite(y) || width <= 0 || height <= 0) {
    return false;
  }
  const minOverlap = 48;
  return workAreas.some((area) => {
    if (area.width <= 0 || area.height <= 0) return false;
    const overlapW = Math.min(x + width, area.x + area.width) - Math.max(x, area.x);
    const overlapH = Math.min(y + height, area.y + area.height) - Math.max(y, area.y);
    return overlapW >= minOverlap && overlapH >= minOverlap;
  });
}
