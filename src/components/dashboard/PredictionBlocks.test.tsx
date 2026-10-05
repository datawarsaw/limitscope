// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import {
  PredictionDetail,
  PredictionSummary,
  thresholdRowText,
} from "./PredictionBlocks";
import { QuotaWindowList } from "./QuotaWindowList";
import { predictWindow } from "../../lib/prediction/engine";
import type {
  QuotaObservation,
  QuotaPrediction,
} from "../../lib/prediction/types";
import { predictionBasisLabel } from "../../lib/v03Integration";
import { CRITICAL_PERCENT, NEAR_LIMIT_PERCENT } from "../../lib/thresholds";
import type { ProviderUsage } from "../../types";

// Same fixed timeline as the engine tests: 2026-09-27T06:00:00Z is minute 0.
const BASE = Date.parse("2026-09-27T06:00:00Z");
const iso = (minutes: number): string =>
  new Date(BASE + minutes * 60_000).toISOString();

function obsAt(
  minutes: number,
  usedPercent: number,
  resetAt?: string,
): QuotaObservation {
  return {
    providerId: "p",
    windowLabel: "w",
    usedPercent,
    observedAt: iso(minutes),
    ...(resetAt !== undefined ? { resetAt } : {}),
  };
}

function series(
  from: number,
  to: number,
  step: number,
  percentAt: (minutes: number) => number,
  resetAt?: string,
): QuotaObservation[] {
  const out: QuotaObservation[] = [];
  for (let m = from; m <= to; m += step) out.push(obsAt(m, percentAt(m), resetAt));
  return out;
}

function enginePrediction(
  observations: QuotaObservation[],
  now: string = iso(360),
): QuotaPrediction {
  return predictWindow({
    observations,
    providerId: "p",
    windowLabel: "w",
    now,
  });
}

// 12%/h over two hours: latest 62% at minute 360, reset at minute 480.
// 80 crosses at minute 450 (90 minutes out); 95 lands past the reset.
const crossingPrediction = enginePrediction(
  series(240, 360, 15, (m) => 38 + 12 * ((m - 240) / 60), iso(480)),
);

// Idle at 85%: 80 is an observed fact, 95 cannot be reached at zero burn.
const crossedPrediction = enginePrediction(
  series(0, 360, 15, () => 85, iso(480)),
);

// 12%/h into 79%: the 80% crossing (minute 365) predates `now` (minute 385)
// without a confirming sample, so it is unresolvable; 95 lands at minute 440.
const unresolvablePrediction = enginePrediction(
  [obsAt(240, 55, iso(480)), obsAt(300, 67, iso(480)), obsAt(360, 79, iso(480))],
  iso(385),
);

// Span of 15 minutes: confidence low — the engine still resolves thresholds,
// but the product gate must hide every one of them.
const lowConfidencePrediction = enginePrediction(
  [obsAt(345, 34, iso(540)), obsAt(360, 40, iso(540))],
);

const insufficientPrediction = enginePrediction([]);

function usageWithLimit(usedPercent: number): ProviderUsage {
  return {
    id: "p",
    name: "Provider P",
    status: "ok",
    health: "live",
    checkedAt: iso(360),
    limits: [{ label: "w", usedPercent }],
  };
}

function renderDetail(prediction: QuotaPrediction, now = iso(360)) {
  return render(
    <PredictionDetail
      prediction={prediction}
      perspective="used"
      now={new Date(now)}
    />,
  );
}

function rowLabels(container: HTMLElement): string[] {
  return [...container.querySelectorAll(".prediction-threshold")].map(
    (element) => element.getAttribute("aria-label") ?? "",
  );
}

/** The visible row text: the first text node, before the tooltip span. */
function rowTexts(container: HTMLElement): string[] {
  return [...container.querySelectorAll(".prediction-threshold")].map(
    (element) => element.childNodes[0]?.textContent ?? "",
  );
}

afterEach(cleanup);

describe("thresholdRowText — exact string table", () => {
  it("states an already-crossed level as an observed fact", () => {
    expect(
      thresholdRowText(
        {
          thresholdPercent: NEAR_LIMIT_PERCENT,
          crossesBeforeReset: true,
          alreadyCrossed: true,
          estimatedAt: iso(360),
        },
        Date.parse(iso(360)),
      ),
    ).toBe("80% used reached");
  });

  it("states a projected crossing as an approximate duration from now", () => {
    expect(
      thresholdRowText(
        {
          thresholdPercent: CRITICAL_PERCENT,
          crossesBeforeReset: true,
          estimatedAt: iso(450),
        },
        Date.parse(iso(360)),
      ),
    ).toBe("Est. 95% used in ~1h 30m");
  });

  it("keeps <1m approximate without an extra tilde", () => {
    expect(
      thresholdRowText(
        {
          thresholdPercent: NEAR_LIMIT_PERCENT,
          crossesBeforeReset: true,
          estimatedAt: iso(360.5),
        },
        Date.parse(iso(360)),
      ),
    ).toBe("Est. 80% used in <1m");
  });

  it("marks an anchor that the clock passed as ~0m, never negative", () => {
    expect(
      thresholdRowText(
        {
          thresholdPercent: CRITICAL_PERCENT,
          crossesBeforeReset: true,
          estimatedAt: iso(350),
        },
        Date.parse(iso(360)),
      ),
    ).toBe("Est. 95% used in ~0m");
  });

  it("states a level the reset beats before the fit does", () => {
    expect(
      thresholdRowText(
        { thresholdPercent: CRITICAL_PERCENT, crossesBeforeReset: false },
        Date.parse(iso(360)),
      ),
    ).toBe("95% used not expected before reset");
  });

  it("falls back to Estimate unavailable when the claim is unresolvable", () => {
    expect(
      thresholdRowText(
        { thresholdPercent: NEAR_LIMIT_PERCENT, crossesBeforeReset: undefined },
        Date.parse(iso(360)),
      ),
    ).toBe("Estimate unavailable");
  });

  it("never fabricates a duration from an unusable anchor", () => {
    expect(
      thresholdRowText(
        { thresholdPercent: NEAR_LIMIT_PERCENT, crossesBeforeReset: true },
        Date.parse(iso(360)),
      ),
    ).toBe("Estimate unavailable");
  });
});

describe("PredictionDetail — threshold rows", () => {
  it("renders the engine's resolved levels with the contract wording", () => {
    const { container } = renderDetail(crossingPrediction);
    expect(screen.getByText("Est. 80% used in ~1h 30m")).toBeTruthy();
    expect(
      screen.getByText("95% used not expected before reset"),
    ).toBeTruthy();
    expect(container.querySelectorAll(".prediction-threshold")).toHaveLength(2);
  });

  it("orders rows 80 then 95 regardless of crossing state", () => {
    const { container } = renderDetail(crossedPrediction);
    const labels = rowLabels(container);
    expect(labels).toHaveLength(2);
    expect(labels[0].startsWith("80% used reached")).toBe(true);
    expect(labels[1].startsWith("95% used not expected before reset")).toBe(
      true,
    );
    expect(screen.getByText("80% used reached")).toBeTruthy();
  });

  it("renders one row per resolved level and none for an omitted level", () => {
    const singleLevel: QuotaPrediction = {
      ...crossingPrediction,
      thresholds: [
        {
          thresholdPercent: NEAR_LIMIT_PERCENT,
          crossesBeforeReset: true,
          estimatedAt: iso(450),
        },
      ],
    };
    const { container } = renderDetail(singleLevel);
    expect(container.querySelectorAll(".prediction-threshold")).toHaveLength(1);
    expect(screen.queryByText(/95% used/)).toBeNull();

    const noLevels: QuotaPrediction = {
      ...crossingPrediction,
      thresholds: undefined,
    };
    const without = renderDetail(noLevels);
    expect(
      without.container.querySelectorAll(".prediction-threshold"),
    ).toHaveLength(0);
    without.unmount();
  });

  it("renders the unresolvable state as Estimate unavailable beside a live projection", () => {
    renderDetail(unresolvablePrediction, iso(385));
    expect(screen.getByText("Estimate unavailable")).toBeTruthy();
    // Duration recomputed at render: minute 440 anchor minus minute-385 now.
    expect(screen.getByText("Est. 95% used in ~55m")).toBeTruthy();
  });

  it("carries the basis tooltip and `${rowText} (${basis})` aria-label per row", () => {
    const { container } = renderDetail(crossingPrediction);
    const basis = predictionBasisLabel(crossingPrediction);
    const tooltips = container.querySelectorAll(
      ".prediction-threshold .prediction-tooltip",
    );
    expect(tooltips).toHaveLength(2);
    for (const tooltip of tooltips) {
      expect(tooltip.getAttribute("role")).toBe("tooltip");
      expect(tooltip.textContent).toBe(basis);
    }
    const labels = rowLabels(container);
    expect(labels[0]).toBe(`Est. 80% used in ~1h 30m (${basis})`);
    expect(labels[1]).toBe(`95% used not expected before reset (${basis})`);
  });
});

describe("threshold rows — confidence gating", () => {
  function renderList(prediction: QuotaPrediction, usedPercent = 62) {
    return render(
      <QuotaWindowList
        usage={usageWithLimit(usedPercent)}
        now={new Date(iso(360))}
        predictionFor={() => prediction}
        perspective="used"
      />,
    );
  }

  it("renders rows at medium confidence behind the single gate", () => {
    const { container } = renderList(crossingPrediction);
    expect(container.querySelectorAll(".prediction-threshold")).toHaveLength(2);
  });

  it("renders no threshold text at low confidence", () => {
    const { container } = renderList(lowConfidencePrediction);
    expect(container.querySelectorAll(".prediction-threshold")).toHaveLength(0);
    expect(screen.queryByText(/Est\./)).toBeNull();
    expect(screen.queryByText(/used reached/)).toBeNull();
    expect(screen.queryByText(/not expected before reset/)).toBeNull();
    expect(screen.queryByText("Estimate unavailable")).toBeNull();
  });

  it("renders no threshold text when confidence is insufficient", () => {
    const { container } = renderList(insufficientPrediction);
    expect(container.querySelectorAll(".prediction-threshold")).toHaveLength(0);
    expect(screen.queryByText(/Est\./)).toBeNull();
    expect(screen.queryByText("Estimate unavailable")).toBeNull();
  });
});

describe("threshold rows — Used/Remaining invariance", () => {
  it("shows identical threshold rows in both perspectives, always in used terms", () => {
    const used = render(
      <PredictionDetail
        prediction={crossingPrediction}
        perspective="used"
        now={new Date(iso(360))}
      />,
    );
    const remaining = render(
      <PredictionDetail
        prediction={crossingPrediction}
        perspective="remaining"
        now={new Date(iso(360))}
      />,
    );
    expect(rowLabels(remaining.container)).toEqual(rowLabels(used.container));
    const texts = rowTexts(remaining.container);
    expect(texts).toHaveLength(2);
    for (const text of texts) {
      expect(text.includes("used")).toBe(true);
      expect(text.includes("remaining")).toBe(false);
    }
  });
});

describe("threshold rows — severity independence", () => {
  it("keeps row content identical across severity bands", () => {
    const labelsFor = (usedPercent: number): string[] => {
      const { container, unmount } = render(
        <QuotaWindowList
          usage={usageWithLimit(usedPercent)}
          now={new Date(iso(360))}
          predictionFor={() => crossingPrediction}
          perspective="used"
        />,
      );
      const labels = rowLabels(container);
      unmount();
      return labels;
    };
    const calm = labelsFor(42);
    const warn = labelsFor(82);
    const critical = labelsFor(96);
    expect(calm).toEqual(warn);
    expect(warn).toEqual(critical);
    expect(calm).toHaveLength(2);
  });
});

describe("A02 tooltip — pinned persistence, Escape dismissal, suppression", () => {
  function firstRow(container: HTMLElement): HTMLElement {
    const row = container.querySelector<HTMLElement>(".prediction-threshold");
    if (!row) throw new Error("no threshold row rendered");
    return row;
  }

  it("starts unpinned and unsuppressed", () => {
    const { container } = renderDetail(crossingPrediction);
    const row = firstRow(container);
    expect(row.getAttribute("data-tooltip-pinned")).toBe("false");
    expect(row.getAttribute("data-tooltip-suppressed")).toBe("false");
  });

  it("click pins the tooltip open; clicking again unpins", async () => {
    const user = userEvent.setup();
    const { container } = renderDetail(crossingPrediction);
    const row = firstRow(container);
    await user.click(row);
    expect(row.getAttribute("data-tooltip-pinned")).toBe("true");
    // The pin is persistence while reading: it survives pointer leaves.
    fireEvent.mouseLeave(row);
    expect(row.getAttribute("data-tooltip-pinned")).toBe("true");
    await user.click(row);
    expect(row.getAttribute("data-tooltip-pinned")).toBe("false");
  });

  it("Escape dismisses without moving focus and clears an existing pin", async () => {
    const user = userEvent.setup();
    const { container } = renderDetail(crossingPrediction);
    const row = firstRow(container);
    await user.click(row);
    expect(row.getAttribute("data-tooltip-pinned")).toBe("true");
    row.focus();
    await user.keyboard("{Escape}");
    expect(row.getAttribute("data-tooltip-suppressed")).toBe("true");
    expect(row.getAttribute("data-tooltip-pinned")).toBe("false");
    // Dismissal never moves the pointer or the focus.
    expect(document.activeElement).toBe(row);
  });

  it("suppression survives pointer re-entry until a deliberate refocus or activation", async () => {
    const user = userEvent.setup();
    const { container } = renderDetail(crossingPrediction);
    const row = firstRow(container);
    row.focus();
    await user.keyboard("{Escape}");
    expect(row.getAttribute("data-tooltip-suppressed")).toBe("true");
    // Pointer re-entry alone never reopens it (CSS guardrail pins the
    // override rule; here the state itself must not have been cleared).
    fireEvent.mouseEnter(row);
    fireEvent.mouseLeave(row);
    expect(row.getAttribute("data-tooltip-suppressed")).toBe("true");
    // Focus leaving the row is the deliberate move that ends suppression.
    fireEvent.blur(row);
    expect(row.getAttribute("data-tooltip-suppressed")).toBe("false");
  });

  it("click after Escape reactivates and pins in one deliberate action", async () => {
    const user = userEvent.setup();
    const { container } = renderDetail(crossingPrediction);
    const row = firstRow(container);
    row.focus();
    await user.keyboard("{Escape}");
    await user.click(row);
    expect(row.getAttribute("data-tooltip-suppressed")).toBe("false");
    expect(row.getAttribute("data-tooltip-pinned")).toBe("true");
  });

  it("keeps reveal state per element — the container never suppresses its rows", async () => {
    const user = userEvent.setup();
    const { container } = renderDetail(crossingPrediction);
    const panel = container.querySelector<HTMLElement>(".prediction");
    if (!panel) throw new Error("no prediction panel rendered");
    panel.focus();
    await user.keyboard("{Escape}");
    expect(panel.getAttribute("data-tooltip-suppressed")).toBe("true");
    expect(firstRow(container).getAttribute("data-tooltip-suppressed")).toBe(
      "false",
    );
  });

  it("PredictionSummary carries the same pin and Escape contract", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <PredictionSummary prediction={crossingPrediction} perspective="used" />,
    );
    const note = container.querySelector<HTMLElement>(".pace-note");
    if (!note) throw new Error("no pace note rendered");
    const tooltip = note.querySelector<HTMLElement>(".prediction-tooltip");
    expect(tooltip?.getAttribute("role")).toBe("tooltip");
    await user.click(note);
    expect(note.getAttribute("data-tooltip-pinned")).toBe("true");
    note.focus();
    await user.keyboard("{Escape}");
    expect(note.getAttribute("data-tooltip-suppressed")).toBe("true");
    expect(note.getAttribute("data-tooltip-pinned")).toBe("false");
  });

  it("keeps role=tooltip on every basis tooltip through pin and Escape", async () => {
    const user = userEvent.setup();
    const { container } = renderDetail(crossingPrediction);
    const row = firstRow(container);
    await user.click(row);
    row.focus();
    await user.keyboard("{Escape}");
    const tooltips = container.querySelectorAll(".prediction-tooltip");
    expect(tooltips.length).toBe(3); // container + two threshold rows
    for (const tooltip of tooltips) {
      expect(tooltip.getAttribute("role")).toBe("tooltip");
    }
  });
});
