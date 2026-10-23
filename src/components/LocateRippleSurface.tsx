import { useEffect } from "react";

/**
 * The locate beacon's wave pool, rendered inside the transient overlay
 * window (see lib/locateRipple.ts). Three staggered rings expand from the
 * stage's center and fade out; the motion lives entirely in floating.css.
 *
 * The surface does no IPC and reads no state: the overlay window exists for
 * about two seconds, has no capabilities of its own, and must never wait on
 * anything before it can paint.
 */
export function LocateRippleSurface() {
  useEffect(() => {
    // The shared entry's inline script only clears the background for the
    // floating bar's label; the overlay's own label needs the same, or the
    // rings would paint over an opaque page background.
    document.documentElement.dataset.window = "locate-ripple";
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
  }, []);
  return (
    <div className="locate-ripple-stage" aria-hidden="true">
      <span className="locate-ripple-ring locate-ripple-ring-1" />
      <span className="locate-ripple-ring locate-ripple-ring-2" />
      <span className="locate-ripple-ring locate-ripple-ring-3" />
    </div>
  );
}
