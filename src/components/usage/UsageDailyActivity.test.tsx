// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import { UsageDailyActivity } from "./UsageDailyActivity";
import type { UsageDailyDay } from "../../lib/usageIntelligence";

/**
 * Focused presentation pins for the Daily activity heatmap: one cell per
 * backend day, relative intensity classes, the hatched missing state, the
 * before-collection state, accessible labels, and the honesty note.
 */

function day(overrides: Partial<UsageDailyDay>): UsageDailyDay {
  return {
    date: "2026-10-24",
    observed: true,
    events: 3,
    totalTokens: 1_000,
    beforeCollection: false,
    ...overrides,
  };
}

function dayKey(date: string): string {
  const [year, month, dayNumber] = date.split("-").map(Number);
  return new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  }).format(new Date(year, month - 1, dayNumber));
}

/** Five days: a peak, two mid days, an observed zero, and a missing day. */
function week(): UsageDailyDay[] {
  return [
    day({ date: "2026-10-20", totalTokens: 4_000_000, events: 40 }),
    day({ date: "2026-10-21", totalTokens: 1_000_000, events: 10 }),
    day({ date: "2026-10-22", totalTokens: 0, events: 2 }),
    day({ date: "2026-10-23", observed: false, events: 0, totalTokens: undefined }),
    day({ date: "2026-10-24", totalTokens: 3_000_000, events: 31 }),
  ];
}

describe("UsageDailyActivity", () => {
  afterEach(cleanup);

  it("renders one cell per backend day inside a labeled list", () => {
    render(<UsageDailyActivity days={week()} />);
    const grid = screen.getByRole("list", {
      name: "Daily reported token activity",
    });
    expect(within(grid).getAllByRole("listitem")).toHaveLength(5);
  });

  it("normalizes intensity against the busiest observed day", () => {
    render(<UsageDailyActivity days={week()} />);
    const grid = screen.getByRole("list", {
      name: "Daily reported token activity",
    });
    const cells = within(grid).getAllByRole("listitem");
    expect(cells[0].className).toContain("usage-daily-l4"); // 4.0M = peak
    expect(cells[1].className).toContain("usage-daily-l1"); // 1.0M = 25%
    expect(cells[4].className).toContain("usage-daily-l3"); // 3.0M = 75%
    expect(cells[2].className).toContain("usage-daily-l0"); // observed zero
  });

  it("labels a missing day by its date and missing semantics", () => {
    render(<UsageDailyActivity days={week()} />);
    const grid = screen.getByRole("list", {
      name: "Daily reported token activity",
    });
    const cells = within(grid).getAllByRole("listitem");
    expect(cells[3].getAttribute("aria-label")).toBe(
      `${dayKey("2026-10-23")}: no recorded events — not confirmed zero usage`,
    );
    expect(cells[3].className).toContain("usage-daily-cell-none");
    expect(cells[3].getAttribute("title")).toBe(
      `${dayKey("2026-10-23")}: no recorded events — not confirmed zero usage`,
    );
  });

  it("states that pre-collection days can have no data", () => {
    render(
      <UsageDailyActivity
        days={[
          day({
            date: "2026-10-18",
            observed: false,
            events: 0,
            totalTokens: undefined,
            beforeCollection: true,
          }),
          day({ date: "2026-10-19" }),
        ]}
      />,
    );
    const before = screen.getByRole("listitem", {
      name: `${dayKey("2026-10-18")}: before local collection started — no data`,
    });
    expect(before.className).toContain("usage-daily-cell-none");
  });

  it("labels observed cells with their date, reported tokens, and events", () => {
    render(<UsageDailyActivity days={week()} />);
    expect(
      screen.getByRole("listitem", {
        name: `${dayKey("2026-10-20")}: 4M reported tokens across 40 recorded events`,
      }),
    ).toBeTruthy();
  });

  it("carries the five-level legend and the no-recorded-events swatch", () => {
    render(<UsageDailyActivity days={week()} />);
    for (const label of [
      "no reported tokens",
      "light",
      "moderate",
      "high",
      "peak",
      "no recorded events",
    ]) {
      expect(screen.getByText(label)).toBeTruthy();
    }
  });

  it("explains the relative scale and disclaims quota consumption", () => {
    render(<UsageDailyActivity days={week()} />);
    expect(
      screen.getByText(/relative reported activity, not quota consumption/),
    ).toBeTruthy();
    expect(screen.getByText(/One cell per local day/)).toBeTruthy();
    expect(
      screen.getByText(/days before local collection started have no data at all/),
    ).toBeTruthy();
  });

  it("shows the covered local date range", () => {
    render(<UsageDailyActivity days={week()} />);
    expect(
      screen.getByText(
        new RegExp(
          `${dayKey("2026-10-20")} – ${dayKey("2026-10-24")}`,
        ),
      ),
    ).toBeTruthy();
  });

  it("renders without days without falling over", () => {
    const { container } = render(<UsageDailyActivity days={[]} />);
    expect(container.querySelectorAll(".usage-daily-cell")).toHaveLength(0);
    expect(screen.getByText(/One cell per local day/)).toBeTruthy();
  });
});
