// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ProviderUsage } from "../types";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { FloatingProviderItem, ProviderMark } from "./FloatingProviderItem";
import { FloatingProviderPopover } from "./FloatingProviderPopover";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const now = new Date("2026-11-19T12:00:00Z");

const mistral: ProviderUsage = {
  id: "mistral",
  name: "Mistral",
  status: "ok",
  health: "live",
  checkedAt: "2026-11-19T11:59:00Z",
  sourceUpdatedAt: "2026-11-19T11:59:00Z",
  dataFreshness: "fresh",
  account: {
    label: "key ••alue",
    note: "Read-only Mistral billing.",
    identity: "credential-v1:mistral-a",
  },
  planType: "EDU",
  limits: [
    { label: "Vibe Budget", usedPercent: 0.27, resetAt: "2026-12-01T00:00:00Z" },
    { label: "API Budget", usedPercent: 14.12, resetAt: "2026-12-01T00:00:00Z" },
  ],
  mistralBudgets: {
    planName: "EDU",
    rows: [
      {
        key: "vibe",
        label: "Vibe Budget",
        available: true,
        currency: "EUR",
        totalCents: 12750,
        usedCents: 34,
        remainingCents: 12716,
        usedPercent: 0.27,
        resetAt: "2026-12-01T00:00:00Z",
        amountDerived: true,
      },
      {
        key: "api",
        label: "API Budget",
        available: true,
        currency: "EUR",
        totalCents: 1275,
        usedCents: 180,
        remainingCents: 1095,
        usedPercent: 14.12,
        resetAt: "2026-12-01T00:00:00Z",
        amountDerived: true,
      },
    ],
  },
};

/** The same provider under an account-scoped id. */
const scopedMistral: ProviderUsage = { ...mistral, id: "mistral:secondary" };

const segment = (item: ReturnType<typeof floatingQuotaItems>[number]) => (
  <FloatingProviderItem
    item={item}
    expanded={false}
    onHover={() => {}}
    onHoverEnd={() => {}}
    onOpen={() => {}}
    onContextMenu={() => {}}
  />
);

const card = (item: ReturnType<typeof floatingQuotaItems>[number]) => (
  <FloatingProviderPopover
    item={item}
    pinned={false}
    now={now}
    note={null}
    align={0}
    exiting={false}
    onMouseEnter={() => {}}
    onMouseLeave={() => {}}
    onFocus={() => {}}
    onBlur={() => {}}
  />
);

/**
 * The published Mistral pixel-cat emblem GEOMETRY, as it stands in
 * Mistral's own 2026 brand kit:
 * Icon/Monochrome/RGB/SVG/Icon-Mistral-White.svg
 * (sha256 65b6edef03c108849cea04b82017882624be60e641f0fa9dff72800b4a6c83b5),
 * fetched from https://mistral.ai/cms-media/api/media/file/Mistral_Logos_2026.zip
 * and linked from https://mistral.ai/brand/ . The viewBox is the published
 * one, so the emblem keeps the brand's own clearspace, and the ten rects are
 * the gradient variant's ten rects with the published white fill.
 *
 * GEOMETRY AND FILL ARE SEPARATE CLAIMS. The geometry below is Mistral's
 * published one and is what the mark still ships, unchanged and unpinned
 * from the brand's own placement. The FILL is a LimitScope UI adaptation:
 * the mark inherits the same muted tone as every neighbouring provider mark
 * instead of carrying a literal white cut-out.
 */
const PUBLISHED_VIEWBOX = "0 0 162 162";

/** The ten published pixels: x, y, width, height, in the published order. */
const PUBLISHED_GEOMETRY: [number, number, number, number][] = [
  [31, 31, 20, 20],
  [111, 31, 20, 20],
  [71, 91, 20, 20],
  [31, 91, 20, 20],
  [111, 91, 20, 20],
  [31, 51, 40, 20],
  [11, 111, 60, 20],
  [91, 111, 60, 20],
  [31, 71, 100, 20],
  [91, 51, 40, 20],
];

/**
 * The fill the mark ships: the slot's own inherited colour. The neighbouring
 * provider marks use the same keyword, so the browser resolves every mark in
 * the deck from the same `color` property and no mark can drift into its own
 * private tone.
 */
const INHERITED_FILL = "currentColor";

/** The gradient variant's published stops, which must not reach the mark. */
const REJECTED_GRADIENT_STOPS = [
  "#FFAF01",
  "#FF8204",
  "#FA500F",
  "#E61300",
  "#C4001D",
];

/** Reads the emblem back out of the DOM, pixel by pixel. */
type EmblemRect = [number, number, number, number, string | null];

const readEmblem = (
  view: { container: HTMLElement },
  selector = ".fq-mark svg",
) => {
  const svg = view.container.querySelector(selector);
  expect(svg).not.toBeNull();
  expect(svg?.getAttribute("viewBox")).toBe(PUBLISHED_VIEWBOX);
  const rects: EmblemRect[] = Array.from(
    svg?.querySelectorAll("rect") ?? [],
  ).map((rect) => [
    Number(rect.getAttribute("x")),
    Number(rect.getAttribute("y")),
    Number(rect.getAttribute("width")),
    Number(rect.getAttribute("height")),
    rect.getAttribute("fill"),
  ]);
  return { svg, rects };
};

/** The ten rects' geometry, without the fill, for a geometry-only compare. */
const geometryOf = (rects: EmblemRect[]): [number, number, number, number][] =>
  rects.map(([x, y, width, height]) => [x, y, width, height]);

afterEach(cleanup);

describe("Mistral provider mark", () => {
  it("draws the published pixel-cat geometry on the collapsed segment", () => {
    const [item] = floatingQuotaItems([mistral], now);
    const view = render(segment(item));

    const { rects } = readEmblem(view);

    // The ten published pixels, in the published places: the emblem is the
    // official geometry, not a redraw of it.
    expect(geometryOf(rects)).toEqual(PUBLISHED_GEOMETRY);

    // The "MI" monogram is gone: no letter fallback stands in for the mark.
    expect(view.container.querySelector(".fq-mark-fallback")).toBeNull();
    expect(view.container.textContent).not.toContain("MI");
  });

  it("draws the same mark for an account-scoped Mistral id", () => {
    const [item] = floatingQuotaItems([scopedMistral], now);
    const view = render(segment(item));

    const { rects } = readEmblem(view);
    expect(geometryOf(rects)).toEqual(PUBLISHED_GEOMETRY);
    expect(rects.every((rect) => rect[4] === INHERITED_FILL)).toBe(true);
    expect(view.container.querySelector(".fq-mark-fallback")).toBeNull();
  });

  it("inherits the slot's muted tone instead of a published white fill", () => {
    const view = render(<ProviderMark id="mistral" />);
    const { svg, rects } = readEmblem(view, "svg");

    // One colour, and it is the inherited one: every pixel resolves from the
    // slot's `color` property, exactly like every neighbouring mark, so the
    // deck reads as one muted set instead of a white cut-out in the row.
    expect(new Set(rects.map((rect) => rect[4]))).toEqual(new Set([INHERITED_FILL]));
    expect(svg?.querySelectorAll(`[fill="${INHERITED_FILL}"]`).length).toBe(
      PUBLISHED_GEOMETRY.length,
    );

    // No literal white survives: the published white reference is not the
    // shipped treatment, and no other hard-coded colour sneaks in either.
    const source = view.container.innerHTML;
    expect(source).not.toContain('fill="white"');
    expect(svg?.querySelector('[fill="white"]')).toBeNull();
    expect(svg?.querySelector('[fill^="#"]')).toBeNull();
    expect(svg?.querySelector('[fill^="rgb"]')).toBeNull();
    expect(svg?.querySelector('[fill^="url("]')).toBeNull();

    // No opacity trick and no inline colour: the tone is inherited, so it
    // follows the theme token instead of being faked on the mark itself.
    expect(svg?.querySelector("[opacity]")).toBeNull();
    expect(svg?.querySelector("[fill-opacity]")).toBeNull();
    expect(svg?.getAttribute("style")).toBeNull();
    expect(svg?.getAttribute("color")).toBeNull();

    // The rejected gradient variant is gone: none of its five published
    // stops survives anywhere in the mark.
    for (const stop of REJECTED_GRADIENT_STOPS) {
      expect(rects.map((rect) => rect[4])).not.toContain(stop);
    }
    for (const stop of REJECTED_GRADIENT_STOPS) {
      expect(source).not.toContain(stop);
    }
    expect(svg?.querySelector("linearGradient")).toBeNull();
    expect(svg?.querySelector("radialGradient")).toBeNull();
  });

  it("carries the official geometry with the inherited fill in every slot", () => {
    const view = render(<ProviderMark id="mistral" />);
    const { svg, rects } = readEmblem(view, "svg");

    // The official viewBox and the ten published pixels are untouched by the
    // recolour: only the fill changed.
    expect(svg?.getAttribute("viewBox")).toBe(PUBLISHED_VIEWBOX);
    expect(geometryOf(rects)).toEqual(PUBLISHED_GEOMETRY);
    expect(rects).toHaveLength(PUBLISHED_GEOMETRY.length);
  });

  it("shares one mark between the collapsed segment and the expanded card", () => {
    const [item] = floatingQuotaItems([mistral], now);

    const segmentView = render(segment(item));
    const { rects } = readEmblem(segmentView);

    const cardView = render(card(item));
    const cardSvg = cardView.container.querySelector(".fq-popover-mark svg");
    expect(cardSvg?.getAttribute("viewBox")).toBe(PUBLISHED_VIEWBOX);
    const cardRects: EmblemRect[] = Array.from(
      cardSvg?.querySelectorAll("rect") ?? [],
    ).map((rect) => [
        Number(rect.getAttribute("x")),
        Number(rect.getAttribute("y")),
        Number(rect.getAttribute("width")),
        Number(rect.getAttribute("height")),
        rect.getAttribute("fill"),
      ]);

    // One identity, two places: the expanded card wears the mark of the
    // segment it expands, pixel for pixel and in the same inherited tone.
    expect(cardRects).toEqual(rects);
    expect(cardRects.every((rect) => rect[4] === INHERITED_FILL)).toBe(true);
  });

  it("leaves the Vibe and API budgets independent under the shared mark", () => {
    const [item] = floatingQuotaItems([mistral], now);

    // The mark identifies the provider, never the budget: the two monthly
    // budgets stay two separate windows, and the Vibe allowance stays the
    // primary one.
    expect(item.windows.map((row) => row.label)).toEqual([
      "Vibe Budget",
      "API Budget",
    ]);
    expect(item.windowLabel).toBe("Vibe Budget");
    expect(item.primaryReset?.label).toBe("Vibe Budget");

    // Independent monthly data: the primary (Vibe) and the secondary
    // (API) budget keep their own allowance and their own consumption, so
    // the shared mark never merges them.
    expect(item.windows.map((row) => row.usedPercent)).toEqual([0.27, 14.12]);
    expect(item.usedPercent).toBe(0.27);
    expect(item.percent).toBe(0);

    const view = render(card(item));
    const rows = view.container.querySelectorAll("[data-mistral-row]");
    expect(Array.from(rows).map((row) => row.getAttribute("data-mistral-row"))).toEqual([
      "vibe",
      "api",
    ]);
  });

  it("leaves every other provider mark on the inherited fill", () => {
    const svgProviders = [
      "openai-codex",
      "claude-code",
      "zai",
      "opencode-go",
      "antigravity",
      "grok",
    ];

    for (const id of svgProviders) {
      const view = render(<ProviderMark id={id} />);
      const path = view.container.querySelector("path");
      expect(path, id).not.toBeNull();
      expect(path?.getAttribute("fill"), id).toBe("currentColor");

      // The neighbours are untouched by the Mistral change: one path, the
      // same inherited keyword, and no literal colour of their own.
      expect(view.container.querySelectorAll("path"), id).toHaveLength(1);
      expect(view.container.querySelector("rect"), id).toBeNull();
      expect(view.container.innerHTML, id).not.toContain('fill="white"');
      expect(path?.getAttribute("fill"), id).toBe(INHERITED_FILL);
    }
  });

  it("gives the Mistral mark the same inherited tone as its neighbours", () => {
    // The harmonisation is only real if the browser resolves the Mistral
    // mark and the neighbouring marks from the SAME `color` property. A
    // shared keyword is the guarantee; a shared hard-coded hex would be a
    // new hard-coded grey, and an opacity overlay would not match themes.
    const mistralView = render(<ProviderMark id="mistral" />);
    const mistralFills = Array.from(
      mistralView.container.querySelectorAll("rect"),
    ).map((rect) => rect.getAttribute("fill"));
    expect(mistralFills.every((fill) => fill === INHERITED_FILL)).toBe(true);

    const claudeView = render(<ProviderMark id="claude-code" />);
    const claudeFill = claudeView.container
      .querySelector("path")
      ?.getAttribute("fill");
    expect(claudeFill).toBe(mistralFills[0]);
  });
});
