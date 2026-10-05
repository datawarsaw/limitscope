import { describe, expect, it } from "vitest";
import {
  clampWindowBoundsToWorkArea,
  type BoundsRect,
} from "./windowBounds";

/** 1920×1080 display with a taskbar reserved along the bottom. */
const AREA_1080P: BoundsRect = { x: 0, y: 0, width: 1920, height: 1032 };
/** Secondary monitor left of the primary: negative coordinates. */
const AREA_LEFT: BoundsRect = { x: -1920, y: 0, width: 1920, height: 1032 };

function clamp(window: BoundsRect, workArea: BoundsRect) {
  return clampWindowBoundsToWorkArea(window, workArea);
}

describe("clampWindowBoundsToWorkArea", () => {
  it("leaves fully contained bounds untouched", () => {
    const window: BoundsRect = { x: 120, y: 80, width: 700, height: 760 };
    expect(clamp(window, AREA_1080P)).toEqual({
      bounds: window,
      changed: false,
    });
  });

  it("leaves contained bounds on a negative-coordinate monitor untouched", () => {
    const window: BoundsRect = { x: -1800, y: 40, width: 700, height: 760 };
    expect(clamp(window, AREA_LEFT)).toEqual({
      bounds: window,
      changed: false,
    });
  });

  it("pulls in a window poking out on the right without resizing it", () => {
    const result = clamp(
      { x: 1500, y: 100, width: 700, height: 760 },
      AREA_1080P,
    );
    expect(result).toEqual({
      bounds: { x: 1220, y: 100, width: 700, height: 760 },
      changed: true,
    });
  });

  it("pulls in a window poking out below the work area", () => {
    const result = clamp(
      { x: 100, y: 600, width: 700, height: 760 },
      AREA_1080P,
    );
    expect(result).toEqual({
      bounds: { x: 100, y: 272, width: 700, height: 760 },
      changed: true,
    });
  });

  it("pulls in a window stranded left of and above the work area", () => {
    const result = clamp({ x: -300, y: -120, width: 700, height: 760 }, AREA_1080P);
    expect(result).toEqual({
      bounds: { x: 0, y: 0, width: 700, height: 760 },
      changed: true,
    });
  });

  it("shrinks a window taller than the work area and anchors it to the top", () => {
    // The 200% DPI case: a 700×760 logical default is 1400×1520 physical on a
    // 1080p work area that cannot host it.
    const result = clamp(
      { x: 200, y: 0, width: 1400, height: 1520 },
      AREA_1080P,
    );
    expect(result).toEqual({
      bounds: { x: 200, y: 0, width: 1400, height: 1032 },
      changed: true,
    });
  });

  it("shrinks a window wider than the work area", () => {
    const result = clamp({ x: 0, y: 0, width: 2400, height: 800 }, AREA_1080P);
    expect(result).toEqual({
      bounds: { x: 0, y: 0, width: 1920, height: 800 },
      changed: true,
    });
  });

  it("shrinks first, then positions inside the reduced size", () => {
    const result = clamp(
      { x: 1800, y: 900, width: 1400, height: 1520 },
      AREA_1080P,
    );
    expect(result).toEqual({
      bounds: { x: 520, y: 0, width: 1400, height: 1032 },
      changed: true,
    });
  });

  it("respects a partially off-screen window on a negative-coordinate monitor", () => {
    const result = clamp(
      { x: -2100, y: 100, width: 700, height: 760 },
      AREA_LEFT,
    );
    expect(result).toEqual({
      bounds: { x: -1920, y: 100, width: 700, height: 760 },
      changed: true,
    });
  });

  it("keeps a window fully off-screen on the right pulled to the edge", () => {
    const result = clamp(
      { x: 4000, y: 100, width: 700, height: 760 },
      AREA_1080P,
    );
    expect(result).toEqual({
      bounds: { x: 1220, y: 100, width: 700, height: 760 },
      changed: true,
    });
  });

  it("returns the input unchanged for degenerate work areas", () => {
    for (const area of [
      { x: 0, y: 0, width: 0, height: 0 },
      { x: 0, y: 0, width: 1920, height: -5 },
    ]) {
      const window: BoundsRect = { x: 50, y: 50, width: 400, height: 300 };
      expect(clamp(window, area)).toEqual({ bounds: window, changed: false });
    }
  });

  it("returns the input unchanged for non-finite window values", () => {
    const window: BoundsRect = {
      x: Number.NaN,
      y: 0,
      width: 700,
      height: 760,
    };
    expect(clamp(window, AREA_1080P)).toEqual({
      bounds: window,
      changed: false,
    });
  });

  it("returns the input unchanged for degenerate window sizes", () => {
    const window: BoundsRect = { x: 50, y: 50, width: 0, height: 300 };
    expect(clamp(window, AREA_1080P)).toEqual({
      bounds: window,
      changed: false,
    });
  });

  it("keeps a small floating bar fully visible at the bottom-right corner", () => {
    // A11 extension: the overlay correction clamps both axes with the same
    // function; a bar parked at the right edge must move left, not clip.
    const result = clamp(
      { x: 1900, y: 990, width: 520, height: 64 },
      AREA_1080P,
    );
    expect(result).toEqual({
      bounds: { x: 1400, y: 968, width: 520, height: 64 },
      changed: true,
    });
  });
});
