import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// Structural companion to themeCss.test.ts: the dashboard's responsive
// contract lives in styles.css keyed on [data-layout] (the attribute
// useDashboardLayout sets), so a half-added layout mode or a zone missing
// from the grid would only be caught by reading the stylesheet itself.
const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");
const withoutComments = css.replace(/\/\*[\s\S]*?\*\//g, "");

describe("dashboard layout styles", () => {
  it("gives every zone a grid area in every layout mode", () => {
    for (const zone of ["rail", "main", "attention"]) {
      expect(withoutComments).toMatch(
        new RegExp(`grid-area:\\s*${zone}\\s*;`),
      );
    }
  });

  it("defines the three structural layout modes", () => {
    expect(withoutComments).toMatch(
      /\.app\[data-layout="medium"\]\s*\.dashboard\s*\{/,
    );
    expect(withoutComments).toMatch(
      /\.app\[data-layout="wide"\]\s*\.dashboard\s*\{/,
    );
  });

  it("keeps the narrow default a single scrolling column (no horizontal scrolling)", () => {
    // The bare .dashboard block is the narrow layout: one minmax(0,1fr)
    // column. minmax(0,…) is what lets long content shrink instead of
    // overflowing horizontally at 360px.
    const dashboardBlock = withoutComments.match(/\.dashboard\s*\{[^}]*\}/)?.[0] ?? "";
    expect(dashboardBlock).toContain("grid-template-columns: minmax(0, 1fr)");
    expect(dashboardBlock).toContain("overflow-y: auto");
  });

  it("keeps the panels on theme tokens (no literal colors in the dashboard layer)", () => {
    const dashboardSlice = withoutComments.slice(
      withoutComments.indexOf(".dashboard {"),
      withoutComments.indexOf(".utility-bar"),
    );
    const literals = dashboardSlice.match(/#[0-9a-f]{3,8}\b/gi) ?? [];
    expect(literals).toEqual([]);
  });

  it("narrow mode renders the rail as wrapping chips without the tiny meter", () => {
    expect(withoutComments).toMatch(
      /\.app\[data-layout="narrow"\]\s*\.provider-rail-list\s*\{[^}]*flex-wrap:\s*wrap/,
    );
    expect(withoutComments).toMatch(
      /\.app\[data-layout="narrow"\]\s*\.rail-meter\s*\{[^}]*display:\s*none/,
    );
  });

  // W02 (v0.9): Settings is a navigation destination rendered inside the
  // scrolling dashboard column; the page itself must never push the utility
  // bar out of a short viewport, so the footer keeps its fixed height and
  // every destination stays single-column in the wider layout modes.
  it("keeps the settings page inside the scrolling destination column", () => {
    expect(withoutComments).toMatch(/\.settings-view\s*\{/);
    expect(withoutComments).toMatch(/\.settings-section\s*\{/);
    for (const view of ["usage", "settings"]) {
      expect(withoutComments).toMatch(
        new RegExp(
          `\\.app\\[data-view="${view}"\\]\\[data-layout="wide"\\] \\.dashboard`,
        ),
      );
    }
    const singleColumn = withoutComments.match(
      /\.app\[data-view="usage"\]\[data-layout="wide"\] \.dashboard,\s*[^{]*\{[^}]*\}/,
    )?.[0] ?? "";
    expect(singleColumn).toContain('grid-template-areas: "main"');
    const utilityBlock = withoutComments.match(/\.utility-bar\s*\{[^}]*\}/)?.[0] ?? "";
    expect(utilityBlock).toContain("flex: none");
  });

  // A02 (v0.8, parallel-safe slice): the prediction tooltip is hoverable —
  // pointer travel into it must not dismiss it, so hit-testing stays enabled.
  it("keeps the prediction tooltip hit-testable while it is visible", () => {
    // Anchored to line start: with Lane A's threshold reveal rules merged
    // in, several selectors now end in `.prediction-tooltip {`, and only
    // the base block is the hit-testable tooltip itself.
    const tooltipBlock =
      withoutComments.match(/^\.prediction-tooltip\s*\{[^}]*\}/m)?.[0] ?? "";
    expect(tooltipBlock).toContain("pointer-events: auto");
  });

  it("still reveals the prediction tooltip on hover and keyboard focus", () => {
    expect(withoutComments).toMatch(
      /\.prediction:hover \.prediction-tooltip,?/,
    );
    expect(withoutComments).toMatch(
      /\.prediction:focus-visible \.prediction-tooltip,?/,
    );
    expect(withoutComments).toMatch(
      /\.pace-note:hover \.prediction-tooltip,?/,
    );
    expect(withoutComments).toMatch(
      /\.pace-note:focus-visible \.prediction-tooltip \{/,
    );
  });

  // A02/K04 (v0.8 integration completion): the controlled-reveal halves.
  // PredictionBlocks carries pin/suppression as data attributes; these rows
  // pin the CSS half of that contract alongside the DOM tests.
  it("reveals a pinned prediction tooltip without hover or focus", () => {
    const pinnedBlock =
      withoutComments.match(
        /\.prediction-threshold\[data-tooltip-pinned="true"\] > \.prediction-tooltip[^{]*\{[^}]*\}/,
      )?.[0] ?? "";
    expect(pinnedBlock).toContain("visibility: visible");
    expect(pinnedBlock).toContain("opacity: 1");
    expect(withoutComments).toMatch(
      /\.pace-note\[data-tooltip-pinned="true"\] > \.prediction-tooltip,?/,
    );
    expect(withoutComments).toMatch(
      /\.prediction\[data-tooltip-pinned="true"\] > \.prediction-tooltip,?/,
    );
  });

  it("Escape suppression overrides every reveal path", () => {
    const suppressedBlock =
      withoutComments.match(
        /\.prediction-threshold\[data-tooltip-suppressed="true"\] \.prediction-tooltip[^{]*\{[^}]*\}/,
      )?.[0] ?? "";
    expect(suppressedBlock).toContain("visibility: hidden !important");
    expect(suppressedBlock).toContain("opacity: 0 !important");
    for (const container of ["prediction", "pace-note", "prediction-threshold"]) {
      expect(withoutComments).toMatch(
        new RegExp(
          `\\.${container}\\[data-tooltip-suppressed="true"\\] \\.prediction-tooltip,?`,
        ),
      );
    }
  });
});
