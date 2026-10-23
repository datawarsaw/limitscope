import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

/**
 * The locate beacon's water ripple.
 *
 * The waves must spread past the floating bar's own window bounds, and that
 * window is sized exactly to the bar, so the ripple cannot live there. It is
 * drawn in a temporary transparent overlay window instead: created for one
 * beacon, click-through, focusless, always on top, and destroyed by this
 * module as soon as the last crest fades. Nothing is ever reused, so a
 * failed animation cannot leave a stale window behind: every path that
 * stops early destroys what it created.
 *
 * The overlay page reuses the app's single HTML entry with a query param;
 * the surface dispatch (main.tsx) routes the window's label here. The page
 * itself performs no IPC at all — the parent owns the window's whole
 * lifecycle, so the overlay needs no capabilities of its own.
 */

/** Label prefix of the overlay window; a generation suffix keeps repeated
 * locates from colliding with a window that is still being destroyed. */
export const LOCATE_RIPPLE_WINDOW_PREFIX = "locate-ripple";

/** The overlay page: the shared entry, dispatched to the ripple surface. */
export const LOCATE_RIPPLE_URL = "index.html?window=locate-ripple";

/** How long one wave lives, and how far the next crest trails it. */
export const LOCATE_RIPPLE_RING_MS = 1600;
export const LOCATE_RIPPLE_RING_STAGGER_MS = 220;
export const LOCATE_RIPPLE_RING_COUNT = 3;

/**
 * When the overlay is destroyed: after the last crest has finished fading,
 * plus a small tail so the fade is never cut by the window going away.
 */
export const LOCATE_RIPPLE_TOTAL_MS =
  LOCATE_RIPPLE_RING_STAGGER_MS * (LOCATE_RIPPLE_RING_COUNT - 1) +
  LOCATE_RIPPLE_RING_MS +
  150;

/**
 * The overlay stage is never smaller than this (logical px), so the docked
 * resting strip — 16px tall — still gets a round pool for its waves.
 */
export const LOCATE_RIPPLE_MIN_STAGE = 240;

/**
 * The overlay rectangle, in physical px, centered on the bar's center.
 *
 * The last wave reaches about 1.5× the bar's width (clamped to a floor and
 * a ceiling), so the ripple reads at any bar size without flooding a whole
 * monitor. The waves fade to fully transparent at the end of their
 * expansion, so the stage only has to contain the visible portion — a crest
 * that is invisible cannot clip visibly.
 */
export function rippleOverlayGeometry(
  bar: { x: number; y: number; width: number; height: number },
  scaleFactor: number,
): { x: number; y: number; width: number; height: number } {
  const scale =
    Number.isFinite(scaleFactor) && scaleFactor > 0 ? scaleFactor : 1;
  const barWidth = Math.max(0, bar.width) / scale;
  const barHeight = Math.max(0, bar.height) / scale;
  const reach = Math.min(1000, Math.max(320, barWidth * 1.5));
  const margin = reach * 0.48;
  const width = Math.round((barWidth + margin * 2) * scale);
  const height = Math.round(
    Math.max(barHeight + margin * 2, LOCATE_RIPPLE_MIN_STAGE) * scale,
  );
  const centerX = bar.x + Math.max(0, bar.width) / 2;
  const centerY = bar.y + Math.max(0, bar.height) / 2;
  return {
    x: Math.round(centerX - width / 2),
    y: Math.round(centerY - height / 2),
    width,
    height,
  };
}

/**
 * The beacon is motion; a user who asked the OS for reduced motion gets the
 * in-window static highlight instead of the waves.
 */
export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

type RippleSession = {
  overlay: WebviewWindow;
  closeTimer: number | null;
  onDestroyed: () => void;
};

// One beacon at a time. A newer locate replaces the older ripple instead of
// stacking windows; the single slot plus the generation suffix means nothing
// can accumulate.
let current: RippleSession | null = null;
let generation = 0;
let pagehideHooked = false;

/**
 * Destroys the current ripple, if any. Idempotent: a session that already
 * destroyed itself (animation end, or the window died another way) has
 * already cleared itself, so this is a no-op.
 */
export function closeLocateRipple(): void {
  const session = current;
  current = null;
  if (!session) return;
  if (session.closeTimer !== null) {
    window.clearTimeout(session.closeTimer);
    session.closeTimer = null;
  }
  // destroy, not close: the app-wide CloseRequested handler hides windows
  // and keeps the app alive, which would leave an invisible overlay alive.
  void session.overlay.destroy().catch(() => {});
}

function finishSession(session: RippleSession): void {
  if (current !== session) return;
  current = null;
  if (session.closeTimer !== null) {
    window.clearTimeout(session.closeTimer);
    session.closeTimer = null;
  }
}

/**
 * Shows one ripple beacon centered on the bar's physical rect.
 *
 * Resolves false (with nothing left behind) when the overlay cannot be
 * created, positioned or shown — the caller's in-window pulse is the
 * fallback and always runs. Resolves true once the waves are on screen; the
 * overlay then destroys itself after LOCATE_RIPPLE_TOTAL_MS.
 */
export async function spawnLocateRipple(
  bar: { x: number; y: number; width: number; height: number },
  scaleFactor: number,
): Promise<boolean> {
  if (prefersReducedMotion()) return false;
  closeLocateRipple();
  if (!pagehideHooked && typeof window !== "undefined") {
    // A dev reload of the floating page would orphan the overlay and its
    // close timer with it; hand the overlay back before going away.
    pagehideHooked = true;
    window.addEventListener("pagehide", () => closeLocateRipple(), {
      once: true,
    });
  }
  generation += 1;
  const bounds = rippleOverlayGeometry(bar, scaleFactor);
  let overlay: WebviewWindow;
  try {
    overlay = new WebviewWindow(
      `${LOCATE_RIPPLE_WINDOW_PREFIX}-${generation}`,
      {
        url: LOCATE_RIPPLE_URL,
        // Positioned in physical px before it is ever shown, so it cannot
        // flash at a default spot on the wrong monitor.
        visible: false,
        decorations: false,
        transparent: true,
        shadow: false,
        resizable: false,
        skipTaskbar: true,
        // A beacon must never take input or focus: the waves surround the
        // bar, but clicks pass through to whatever is underneath.
        focus: false,
        alwaysOnTop: true,
        center: false,
        backgroundColor: "#00000000",
      },
    );
  } catch {
    return false;
  }
  const session: RippleSession = {
    overlay,
    closeTimer: null,
    onDestroyed: () => finishSession(session),
  };
  current = session;
  // Never let a wedged creation hold the locate flow: the pulse fallback is
  // already running, and a window that surfaces later is destroyed like any
  // other stale session. Extra settles are no-ops on a settled promise.
  const created = await new Promise<boolean>((resolve) => {
    void overlay.once("tauri://created", () => resolve(true));
    void overlay.once("tauri://error", () => resolve(false));
    window.setTimeout(() => resolve(false), 1500);
  });
  if (!created) {
    finishSession(session);
    void overlay.destroy().catch(() => {});
    return false;
  }
  if (current !== session) return false;
  try {
    // The overlay renders nothing until the page loads; position, de-focus
    // and click-through all land while it is still invisible.
    await overlay.setFocusable(false);
    await overlay.setSize(new PhysicalSize(bounds.width, bounds.height));
    await overlay.setPosition(new PhysicalPosition(bounds.x, bounds.y));
    await overlay.setIgnoreCursorEvents(true);
    await overlay.show();
  } catch {
    finishSession(session);
    void overlay.destroy().catch(() => {});
    return false;
  }
  if (current !== session) return false;
  void overlay.once("tauri://destroyed", session.onDestroyed);
  // The parent owns the lifetime: the page performs no IPC, so a crashed or
  // hung overlay page can never delay its own teardown.
  session.closeTimer = window.setTimeout(() => {
    closeLocateRipple();
  }, LOCATE_RIPPLE_TOTAL_MS);
  return true;
}
