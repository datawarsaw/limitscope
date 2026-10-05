import {
  currentMonitor,
  getCurrentWindow,
  PhysicalPosition,
  PhysicalSize,
  primaryMonitor,
  type Window,
} from "@tauri-apps/api/window";
import { isTauriRuntime } from "./floatingWindowChrome";
import {
  clampWindowBoundsToWorkArea,
  type BoundsRect,
} from "./windowBounds";

/**
 * W01 (v0.8): work-area-aware main-window bounds.
 *
 * The window is created 700×760 logical with no clamp anywhere; at high DPI
 * that default is larger than the physical work area (a 1080p work area
 * cannot host 760 logical at 200%), and a restored position can strand the
 * window on a monitor that is gone. The clamp is applied at attach and
 * re-checked on DPI change, resize, and focus regain — always through the
 * pure clamp, so it only ever corrects bounds that violate the current work
 * area and never touches a placement that fits.
 *
 * Kept frontend-owned by design (architecture plan §6.2): the Tauri window
 * API covers everything from the webview, no Rust window code required.
 */

/** Structural slice of a Tauri window the clamp needs; test seam. */
export type ClampableWindow = Pick<
  Window,
  "outerPosition" | "outerSize" | "innerSize" | "setSize" | "setPosition"
>;

export type MonitorLike = {
  workArea: { position: { x: number; y: number }; size: { width: number; height: number } };
};

function workAreaOf(monitor: MonitorLike): BoundsRect {
  return {
    x: monitor.workArea.position.x,
    y: monitor.workArea.position.y,
    width: monitor.workArea.size.width,
    height: monitor.workArea.size.height,
  };
}

/**
 * Clamps the window's current outer bounds into the work area of the monitor
 * it is on (falling back to the primary monitor when that one is gone).
 * Resolves without acting when no monitor is available or the bounds fit.
 */
export async function clampMainWindowToWorkArea(
  win: ClampableWindow = getCurrentWindow(),
  monitors: {
    current: () => Promise<MonitorLike | null>;
    primary: () => Promise<MonitorLike | null>;
  } = { current: currentMonitor, primary: primaryMonitor },
): Promise<void> {
  const [position, outer] = await Promise.all([
    win.outerPosition(),
    win.outerSize(),
  ]);
  const monitor = (await monitors.current()) ?? (await monitors.primary());
  if (!monitor) return;

  const clamped = clampWindowBoundsToWorkArea(
    { x: position.x, y: position.y, width: outer.width, height: outer.height },
    workAreaOf(monitor),
  );
  if (!clamped.changed) return;

  // tao's set_size targets the client area on a decorated window, so the
  // clamped outer intent is converted through the current outer−inner delta
  // (borders + title bar, which scale with DPI). Undersizing by a rounding
  // pixel is harmless; oversizing would defeat the clamp.
  const inner = await win.innerSize();
  const deltaWidth = Math.max(0, outer.width - inner.width);
  const deltaHeight = Math.max(0, outer.height - inner.height);
  await win.setSize(
    new PhysicalSize(
      Math.max(1, clamped.bounds.width - deltaWidth),
      Math.max(1, clamped.bounds.height - deltaHeight),
    ),
  );
  await win.setPosition(new PhysicalPosition(clamped.bounds.x, clamped.bounds.y));
}

/**
 * Applies the clamp once, then re-applies it on scale-factor changes, resizes,
 * and focus regain (the observable proxies for monitor removal). Returns a
 * cleanup that drops the listeners. No-op outside the Tauri runtime.
 */
export function attachMainWindowBounds(): () => void {
  if (!isTauriRuntime()) return () => {};
  const win = getCurrentWindow();
  const reclamp = () => {
    void clampMainWindowToWorkArea(win).catch((error) => {
      console.warn("Main window could not apply work-area bounds", error);
    });
  };
  reclamp();

  const unlistens: Promise<() => void>[] = [];
  try {
    unlistens.push(win.onScaleChanged(() => reclamp()));
    unlistens.push(win.onResized(() => reclamp()));
    unlistens.push(
      win.onFocusChanged(({ payload: focused }) => {
        if (focused) reclamp();
      }),
    );
  } catch (error) {
    console.warn("Main window could not subscribe to bounds events", error);
  }

  return () => {
    for (const unlisten of unlistens) {
      unlisten.then((fn) => fn()).catch(() => {});
    }
  };
}
