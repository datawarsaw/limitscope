import { describe, expect, it } from "vitest";
import {
  DETACH_THRESHOLD_LOGICAL_PX,
  DOCK_ELIGIBILITY_LOGICAL_PX,
  SNAP_PREVIEW_ONSET_LOGICAL_PX,
  evaluateSmartDock,
  gestureFromPhase,
  physicalThresholdPx,
  selectDragMonitor,
  type SmartDockMonitor,
} from "./smartDocking";

const PRIMARY: SmartDockMonitor = {
  scaleFactor: 1,
  workArea: { x: 0, y: 0, width: 3440, height: 1400 },
};

const PORTRAIT: SmartDockMonitor = {
  scaleFactor: 1,
  workArea: { x: 3440, y: 0, width: 1440, height: 2560 },
};

const SIDE: SmartDockMonitor = {
  scaleFactor: 1.5,
  workArea: { x: 2560, y: 80, width: 2560, height: 1360 },
};

function at(
  stable: "floating" | "docked",
  x: number,
  y: number,
  monitor: SmartDockMonitor = PRIMARY,
  width = 480,
) {
  return evaluateSmartDock({ stable, x, y, width, monitor });
}

describe("smart docking thresholds", () => {
  it("keeps the exploratory distances as one set of logical constants", () => {
    expect(SNAP_PREVIEW_ONSET_LOGICAL_PX).toBe(120);
    expect(DOCK_ELIGIBILITY_LOGICAL_PX).toBe(40);
    expect(DETACH_THRESHOLD_LOGICAL_PX).toBe(28);
    expect(physicalThresholdPx(40, 1)).toBe(40);
    expect(physicalThresholdPx(40, 1.5)).toBe(60);
    expect(physicalThresholdPx(28, 2)).toBe(56);
    expect(physicalThresholdPx(120, 0)).toBe(120);
  });

  it("leaves a floating drag far from the top edge on the ordinary path", () => {
    expect(at("floating", 400, 121)).toEqual({ kind: "floating" });
    expect(at("floating", 400, 800)).toEqual({ kind: "floating" });
  });

  it("enters snap preview at the onset and docks only inside eligibility", () => {
    const preview = at("floating", 400, 120);
    expect(preview.kind).toBe("snap-preview");
    if (preview.kind === "snap-preview") expect(preview.eligible).toBe(false);

    const band = at("floating", 400, 41);
    expect(band.kind).toBe("snap-preview");
    if (band.kind === "snap-preview") expect(band.eligible).toBe(false);

    const ready = at("floating", 400, 40);
    expect(ready.kind).toBe("snap-preview");
    if (ready.kind === "snap-preview") expect(ready.eligible).toBe(true);

    const above = at("floating", 400, -8);
    expect(above.kind).toBe("snap-preview");
    if (above.kind === "snap-preview") expect(above.eligible).toBe(true);
  });

  it("preserves an off-center horizontal anchor instead of centering", () => {
    const phase = at("floating", 2000, 10, PRIMARY, 480);
    expect(phase.kind).toBe("snap-preview");
    if (phase.kind !== "snap-preview") return;
    // Center 2240 against work-area center 1720.
    expect(phase.anchor).toBeCloseTo((2240 - 1720) / 3440, 8);
    expect(phase.anchor).not.toBe(0);
  });

  it("uses the work area that contains the window, including a portrait monitor", () => {
    const point = { x: 3800 + 240, y: 12 };
    expect(selectDragMonitor([PRIMARY, PORTRAIT], point)).toBe(PORTRAIT);
    expect(selectDragMonitor([PRIMARY, PORTRAIT], { x: 100, y: 20 })).toBe(PRIMARY);

    const phase = at("floating", 3800, 12, PORTRAIT, 480);
    expect(phase.kind).toBe("snap-preview");
    if (phase.kind !== "snap-preview") return;
    // Portrait center is 3440 + 720. Window center is 4040.
    expect(phase.anchor).toBeCloseTo((4040 - 4160) / 1440, 8);
    expect(phase.distancePx).toBe(12);
  });

  it("measures distance from the work-area top, not the primary screen origin", () => {
    const phase = at("floating", 2700, 100, SIDE, 480);
    expect(phase.kind).toBe("snap-preview");
    if (phase.kind !== "snap-preview") return;
    // 100 - 80 = 20 physical px. At 150% eligibility is 60, so this docks.
    expect(phase.distancePx).toBe(20);
    expect(phase.eligible).toBe(true);
    const far = at("floating", 2700, 80 + 181, SIDE, 480);
    expect(far).toEqual({ kind: "floating" });
  });

  it("keeps a docked drag inside the detach threshold and previews past it", () => {
    expect(at("docked", 1400, 28).kind).toBe("docked");
    const pulled = at("docked", 1400, 29);
    expect(pulled).toMatchObject({ kind: "unsnap-preview", eligible: true, distancePx: 29 });
  });

  it("returns to the docked phase when a pull comes back to the edge", () => {
    expect(at("docked", 1400, 80).kind).toBe("unsnap-preview");
    const returned = at("docked", 1800, 10, PRIMARY, 600);
    expect(returned.kind).toBe("docked");
    if (returned.kind !== "docked") return;
    expect(returned.anchor).toBeCloseTo((2100 - 1720) / 3440, 8);
  });

  it("scales a threshold once and leaves the anchor fraction scale-invariant", () => {
    const low = { ...PRIMARY, scaleFactor: 1 };
    const high = { ...PRIMARY, scaleFactor: 2 };
    const lowPhase = at("floating", 2000, 50, low, 480);
    const highPhase = at("floating", 2000, 50, high, 480);
    expect(lowPhase.kind).toBe("snap-preview");
    expect(highPhase.kind).toBe("snap-preview");
    if (lowPhase.kind !== "snap-preview" || highPhase.kind !== "snap-preview") return;
    // 50 physical px is outside 40 at 100% and inside 80 at 200%.
    expect(lowPhase.eligible).toBe(false);
    expect(highPhase.eligible).toBe(true);
    expect(highPhase.anchor).toBeCloseTo(lowPhase.anchor, 8);

    expect(at("docked", 1400, 40, low).kind).toBe("unsnap-preview");
    expect(at("docked", 1400, 40, high).kind).toBe("docked");
  });

  it("chooses the nearest work area when the point is in a gap", () => {
    const gap = selectDragMonitor(
      [PRIMARY, PORTRAIT],
      { x: 3400, y: -30 },
    );
    expect(gap).toBe(PRIMARY);
    expect(selectDragMonitor([], { x: 0, y: 0 })).toBeNull();
  });
});

describe("smart docking geometry edges", () => {
  const LEFT: SmartDockMonitor = {
    scaleFactor: 1,
    workArea: { x: -1920, y: 0, width: 1920, height: 1080 },
  };

  const UP: SmartDockMonitor = {
    scaleFactor: 1,
    workArea: { x: 0, y: -1440, width: 2560, height: 1440 },
  };

  const UP_LEFT_SCALED: SmartDockMonitor = {
    scaleFactor: 1.5,
    workArea: { x: -2560, y: -1440, width: 2560, height: 1400 },
  };

  it("measures a monitor left of the primary from its own work area", () => {
    expect(selectDragMonitor([PRIMARY, LEFT], { x: -500, y: 30 })).toBe(LEFT);
    const phase = at("floating", -1500, 20, LEFT, 480);
    expect(phase.kind).toBe("snap-preview");
    if (phase.kind !== "snap-preview") return;
    expect(phase.eligible).toBe(true);
    expect(phase.distancePx).toBe(20);
    // Window center -1260 against work-area center -960.
    expect(phase.anchor).toBeCloseTo((-1260 - -960) / 1920, 8);
    expect(at("floating", -1500, 121, LEFT, 480)).toEqual({ kind: "floating" });
  });

  it("measures a monitor above the primary from its own top edge", () => {
    expect(selectDragMonitor([PRIMARY, UP], { x: 900, y: -1400 })).toBe(UP);
    const phase = at("floating", 1000, -1440 + 15, UP, 480);
    expect(phase.kind).toBe("snap-preview");
    if (phase.kind !== "snap-preview") return;
    expect(phase.eligible).toBe(true);
    expect(phase.distancePx).toBe(15);
    expect(at("floating", 1000, -1440 + 121, UP, 480)).toEqual({ kind: "floating" });
    expect(at("docked", 1000, -1440 + 28, UP, 480).kind).toBe("docked");
    expect(at("docked", 1000, -1440 + 29, UP, 480).kind).toBe("unsnap-preview");
  });

  it("applies 150%-scaled thresholds on a monitor left of and above the primary", () => {
    // At 150%: onset 180, eligibility 60, detach 42.
    expect(at("floating", -1400, -1440 + 60, UP_LEFT_SCALED, 480))
      .toMatchObject({ kind: "snap-preview", eligible: true, distancePx: 60 });
    expect(at("floating", -1400, -1440 + 61, UP_LEFT_SCALED, 480))
      .toMatchObject({ kind: "snap-preview", eligible: false });
    expect(at("floating", -1400, -1440 + 180, UP_LEFT_SCALED, 480).kind).toBe("snap-preview");
    expect(at("floating", -1400, -1440 + 181, UP_LEFT_SCALED, 480)).toEqual({ kind: "floating" });
    expect(at("docked", -1400, -1440 + 42, UP_LEFT_SCALED, 480).kind).toBe("docked");
    expect(at("docked", -1400, -1440 + 43, UP_LEFT_SCALED, 480).kind).toBe("unsnap-preview");
    const anchored = at("floating", -1400, -1440 + 10, UP_LEFT_SCALED, 480);
    expect(anchored.kind).toBe("snap-preview");
    if (anchored.kind !== "snap-preview") return;
    // Window center -1160 against work-area center -1280.
    expect(anchored.anchor).toBeCloseTo((-1160 - -1280) / 2560, 8);
  });

  it("re-resolves monitor and thresholds on every gesture-local evaluation", () => {
    const movedWorkArea: SmartDockMonitor = {
      scaleFactor: 1,
      workArea: { x: 3600, y: 40, width: 1440, height: 2560 },
    };
    expect(selectDragMonitor([PRIMARY, PORTRAIT], { x: 4040, y: 12 })).toBe(PORTRAIT);
    expect(selectDragMonitor([PRIMARY, movedWorkArea], { x: 4040, y: 12 })).toBe(movedWorkArea);
    // Distance measures from the moved top edge (y=40), not the old y=0.
    const phase = evaluateSmartDock({
      stable: "floating",
      x: 3800,
      y: 52,
      width: 480,
      monitor: movedWorkArea,
    });
    expect(phase).toMatchObject({ kind: "snap-preview", eligible: true, distancePx: 12 });
    // A DPI change mid-gesture re-derives eligibility for the same point.
    expect(evaluateSmartDock({
      stable: "floating",
      x: 2000,
      y: 70,
      width: 480,
      monitor: PRIMARY,
    })).toMatchObject({ kind: "snap-preview", eligible: false });
    expect(evaluateSmartDock({
      stable: "floating",
      x: 2000,
      y: 70,
      width: 480,
      monitor: { ...PRIMARY, scaleFactor: 2 },
    })).toMatchObject({ kind: "snap-preview", eligible: true });
  });

  it("keeps degenerate and tiny work areas finite and clamped", () => {
    const zeroWidth: SmartDockMonitor = {
      scaleFactor: 1,
      workArea: { x: 500, y: 300, width: 0, height: 800 },
    };
    const phase = at("floating", 520, 310, zeroWidth, 480);
    expect(phase.kind).toBe("snap-preview");
    if (phase.kind !== "snap-preview") return;
    expect(phase.anchor).toBe(0);
    expect(Number.isFinite(phase.distancePx)).toBe(true);

    const negativeWidth = at(
      "floating",
      10,
      10,
      { scaleFactor: 1, workArea: { x: 400, y: 0, width: -100, height: 600 } },
      480,
    );
    expect(negativeWidth.kind).toBe("snap-preview");
    if (negativeWidth.kind !== "snap-preview") return;
    expect(negativeWidth.anchor).toBe(0);

    const onePx = at(
      "floating",
      -3600,
      -900,
      { scaleFactor: 1, workArea: { x: -3601, y: -901, width: 1, height: 1 } },
      480,
    );
    expect(onePx).toMatchObject({ kind: "snap-preview", eligible: true, distancePx: 1 });
    if (onePx.kind !== "snap-preview") return;
    expect(onePx.anchor).toBe(0.5);

    const degenerate: SmartDockMonitor = {
      scaleFactor: 1,
      workArea: { x: 0, y: 0, width: 800, height: 0 },
    };
    expect(selectDragMonitor([degenerate, PRIMARY], { x: 100, y: 5 })).toBe(PRIMARY);
    expect(selectDragMonitor([degenerate], { x: 100, y: 5 })).toBe(degenerate);
  });

  it("survives 32-bit boundary coordinates without overflow or invalid placement", () => {
    const MAX = 2147483647;
    const MIN = -2147483648;
    expect(at("floating", MAX, MAX, PRIMARY, 480)).toEqual({ kind: "floating" });
    const minCorner = at("floating", MIN, MIN, PRIMARY, 480);
    expect(minCorner.kind).toBe("snap-preview");
    if (minCorner.kind !== "snap-preview") return;
    expect(minCorner.eligible).toBe(true);
    expect(minCorner.anchor).toBe(-0.5);
    const farDocked = at("docked", MAX, MAX, PRIMARY, 480);
    expect(farDocked.kind).toBe("unsnap-preview");
    if (farDocked.kind !== "unsnap-preview") return;
    expect(Number.isFinite(farDocked.distancePx)).toBe(true);
    const rightEdge = at("floating", MAX, 10, PRIMARY, 480);
    expect(rightEdge.kind).toBe("snap-preview");
    if (rightEdge.kind !== "snap-preview") return;
    expect(rightEdge.anchor).toBe(0.5);
    const leftEdge = at("floating", MIN, 10, PRIMARY, 480);
    expect(leftEdge.kind).toBe("snap-preview");
    if (leftEdge.kind !== "snap-preview") return;
    expect(leftEdge.anchor).toBe(-0.5);
    expect(selectDragMonitor([PRIMARY, PORTRAIT], { x: MAX, y: MAX })).toBe(PORTRAIT);
  });

  it("converts the logical thresholds exactly once at 200% and 125%", () => {
    const double = { ...PRIMARY, scaleFactor: 2 };
    expect(at("floating", 400, 240, double, 480).kind).toBe("snap-preview");
    expect(at("floating", 400, 241, double, 480)).toEqual({ kind: "floating" });
    expect(at("floating", 400, 80, double, 480)).toMatchObject({ eligible: true });
    expect(at("floating", 400, 81, double, 480)).toMatchObject({ eligible: false });
    expect(at("docked", 1400, 56, double, 480).kind).toBe("docked");
    expect(at("docked", 1400, 57, double, 480).kind).toBe("unsnap-preview");

    const fractional = { ...PRIMARY, scaleFactor: 1.25 };
    expect(at("floating", 400, 150, fractional, 480).kind).toBe("snap-preview");
    expect(at("floating", 400, 151, fractional, 480)).toEqual({ kind: "floating" });
    expect(at("floating", 400, 50, fractional, 480)).toMatchObject({ eligible: true });
    expect(at("floating", 400, 51, fractional, 480)).toMatchObject({ eligible: false });
    expect(at("docked", 1400, 35, fractional, 480).kind).toBe("docked");
    expect(at("docked", 1400, 36, fractional, 480).kind).toBe("unsnap-preview");
  });

  it("floating never resolves to a docked kind across the whole vertical band", () => {
    for (let y = -200; y <= 400; y++) {
      const phase = at("floating", 2000, y, PRIMARY, 480);
      if (phase.kind === "snap-preview") {
        expect(phase.eligible).toBe(y <= DOCK_ELIGIBILITY_LOGICAL_PX);
        expect(Number.isFinite(phase.anchor)).toBe(true);
      } else {
        expect(phase).toEqual({ kind: "floating" });
        expect(y).toBeGreaterThan(SNAP_PREVIEW_ONSET_LOGICAL_PX);
      }
    }
  });

  it("docked never resolves to floating and unsnap previews stay eligible", () => {
    for (let y = -200; y <= 400; y++) {
      const phase = at("docked", 2000, y, PRIMARY, 480);
      if (phase.kind === "docked") {
        expect(y).toBeLessThanOrEqual(DETACH_THRESHOLD_LOGICAL_PX);
      } else {
        expect(phase).toMatchObject({ kind: "unsnap-preview", eligible: true });
        expect(y).toBeGreaterThan(DETACH_THRESHOLD_LOGICAL_PX);
      }
    }
  });

  it("a docked pull past the detach threshold returns to docked at the same edge", () => {
    const start = at("docked", 1800, 10, PRIMARY, 600);
    const pulled = at("docked", 1800, 200, PRIMARY, 600);
    const returned = at("docked", 1800, 10, PRIMARY, 600);
    expect(start.kind).toBe("docked");
    expect(pulled.kind).toBe("unsnap-preview");
    expect(returned.kind).toBe("docked");
    if (start.kind !== "docked" || returned.kind !== "docked") return;
    expect(returned.anchor).toBe(start.anchor);
  });

  it("the off-center anchor is identical across the dock and undock boundary", () => {
    const snap = at("floating", 2000, 30, PRIMARY, 480);
    const docked = at("docked", 2000, 10, PRIMARY, 480);
    expect(snap.kind).toBe("snap-preview");
    expect(docked.kind).toBe("docked");
    if (snap.kind !== "snap-preview" || docked.kind !== "docked") return;
    expect(snap.anchor).toBe(docked.anchor);
    // Anchor ⇄ center roundtrip on the work-area definition.
    const center = PRIMARY.workArea.x + (0.5 + docked.anchor) * PRIMARY.workArea.width;
    expect(center).toBeCloseTo(2000 + 480 / 2, 8);
    const clamped = at("floating", -4000, 30, PRIMARY, 480);
    expect(clamped.kind).toBe("snap-preview");
    if (clamped.kind !== "snap-preview") return;
    expect(clamped.anchor).toBe(-0.5);
  });

  it("publishes content-only gestures and never one for a stable phase", () => {
    expect(gestureFromPhase({ kind: "floating" })).toBeNull();
    expect(gestureFromPhase({ kind: "docked", anchor: 0.2 })).toBeNull();
    expect(gestureFromPhase({ kind: "snap-preview", eligible: false, anchor: 0, distancePx: 100 })).toBe("snap");
    expect(gestureFromPhase({ kind: "snap-preview", eligible: true, anchor: 0, distancePx: 10 })).toBe("snap-ready");
    expect(gestureFromPhase({ kind: "unsnap-preview", eligible: true, distancePx: 60 })).toBe("unsnap");
  });
});
