import { useEffect, useState } from "react";

/**
 * Structural layout of the main dashboard. One source of truth for both the
 * `data-layout` attribute the CSS keys on and the rail's tab orientation, so
 * the markup and the stylesheet can never disagree about which zone sits
 * where:
 *
 * - "wide" (≥680px): provider rail | primary overview + detail | attention
 * - "medium" (≥560px): rail + main; attention moves below main
 * - "narrow" (<560px): provider chips in a horizontal row, then the stacked
 *   zones — no horizontal scrolling, nothing critical disappears at 360px.
 */
export type DashboardLayout = "narrow" | "medium" | "wide";

export const WIDE_LAYOUT_MIN_WIDTH = 680;
export const MEDIUM_LAYOUT_MIN_WIDTH = 560;

const WIDE_QUERY = `(min-width: ${WIDE_LAYOUT_MIN_WIDTH}px)`;
const MEDIUM_QUERY = `(min-width: ${MEDIUM_LAYOUT_MIN_WIDTH}px)`;

/** Pure width→layout mapping (the breakpoint truth the queries encode). */
export function dashboardLayoutFor(width: number): DashboardLayout {
  if (width >= WIDE_LAYOUT_MIN_WIDTH) return "wide";
  if (width >= MEDIUM_LAYOUT_MIN_WIDTH) return "medium";
  return "narrow";
}

function layoutFromQueries(wide: boolean, medium: boolean): DashboardLayout {
  return wide ? "wide" : medium ? "medium" : "narrow";
}

function readLayout(): DashboardLayout | null {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return null;
  }
  return layoutFromQueries(
    window.matchMedia(WIDE_QUERY).matches,
    window.matchMedia(MEDIUM_QUERY).matches,
  );
}

/** Viewport-derived layout, live while the window is resized. Environments
 * without matchMedia (tests, very old webviews) deterministically get the
 * stacked narrow layout — always readable, never clipped. */
export function useDashboardLayout(): DashboardLayout {
  const [layout, setLayout] = useState<DashboardLayout>(
    () => readLayout() ?? "narrow",
  );

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }
    const queries = [window.matchMedia(WIDE_QUERY), window.matchMedia(MEDIUM_QUERY)];
    const update = () =>
      setLayout(layoutFromQueries(queries[0].matches, queries[1].matches));
    for (const query of queries) query.addEventListener("change", update);
    update();
    return () => {
      for (const query of queries) query.removeEventListener("change", update);
    };
  }, []);

  return layout;
}
