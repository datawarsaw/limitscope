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
 * The published Mistral pixel-cat emblem in Mistral's published WHITE
 * monochrome variant, as it stands in Mistral's own 2026 brand kit:
 * Icon/Monochrome/RGB/SVG/Icon-Mistral-White.svg
 * (sha256 65b6edef03c108849cea04b82017882624be60e641f0fa9dff72800b4a6c83b5),
 * fetched from https://mistral.ai/cms-media/api/media/file/Mistral_Logos_2026.zip
 * and linked from https://mistral.ai/brand/ . The viewBox is the published
 * one, so the emblem keeps the brand's own clearspace, and the ten rects
 * are the gradient variant's ten rects with the published white fill.
 */
const PUBLISHED_VIEWBOX = "0 0 162 162";

const PUBLISHED_PIXELS: [number, number, number, number, string][] = [
  [31, 31, 20, 20, "white"],
  [111, 31, 20, 20, "white"],
  [71, 91, 20, 20, "white"],
  [31, 91, 20, 20, "white"],
  [111, 91, 20, 20, "white"],
  [31, 51, 40, 20, "white"],
  [11, 111, 60, 20, "white"],
  [91, 111, 60, 20, "white"],
  [31, 71, 100, 20, "white"],
  [91, 51, 40, 20, "white"],
];

/** The gradient variant's published stops, which must not reach the mark. */
const REJECTED_GRADIENT_STOPS = [
  "#FFAF01",
  "#FF8204",
  "#FA500F",
  "#E61300",
  "#C4001D",
];

/** Reads the emblem back out of the DOM, pixel by pixel. */
const readEmblem = (
  view: { container: HTMLElement },
  selector = ".fq-mark svg",
) => {
  const svg = view.container.querySelector(selector);
  expect(svg).not.toBeNull();
  expect(svg?.getAttribute("viewBox")).toBe(PUBLISHED_VIEWBOX);
  const rects = Array.from(svg?.querySelectorAll("rect") ?? []).map((rect) => [
    Number(rect.getAttribute("x")),
    Number(rect.getAttribute("y")),
    Number(rect.getAttribute("width")),
    Number(rect.getAttribute("height")),
    rect.getAttribute("fill"),
  ]);
  return { svg, rects };
};

afterEach(cleanup);

describe("Mistral provider mark", () => {
  it("draws the published pixel-cat emblem on the collapsed segment", () => {
    const [item] = floatingQuotaItems([mistral], now);
    const view = render(segment(item));

    const { rects } = readEmblem(view);

    // The ten published pixels, in the published places and in the
    // published colours: the emblem is the asset, not a redraw of it.
    expect(rects).toEqual(PUBLISHED_PIXELS);

    // The "MI" monogram is gone: no letter fallback stands in for the mark.
    expect(view.container.querySelector(".fq-mark-fallback")).toBeNull();
    expect(view.container.textContent).not.toContain("MI");
  });

  it("draws the same mark for an account-scoped Mistral id", () => {
    const [item] = floatingQuotaItems([scopedMistral], now);
    const view = render(segment(item));

    const { rects } = readEmblem(view);
    expect(rects).toEqual(PUBLISHED_PIXELS);
    expect(view.container.querySelector(".fq-mark-fallback")).toBeNull();
  });

  it("draws the mark in one published white fill, with no gradient and no multicolour", () => {
    const view = render(<ProviderMark id="mistral" />);
    const { svg, rects } = readEmblem(view, "svg");

    // One colour, the published one: the emblem is white, so the mark
    // reads as a monochrome silhouette like every other mark in the deck.
    expect(svg?.querySelector('[fill="currentColor"]')).toBeNull();
    expect(new Set(rects.map((rect) => rect[4]))).toEqual(new Set(["white"]));

    // The rejected gradient variant is gone: none of its five published
    // stops survives anywhere in the mark.
    for (const stop of REJECTED_GRADIENT_STOPS) {
      expect(rects.map((rect) => rect[4])).not.toContain(stop);
    }
    const source = view.container.innerHTML;
    for (const stop of REJECTED_GRADIENT_STOPS) {
      expect(source).not.toContain(stop);
    }
  });

  it("keeps the published white fill out of a currentColor recolour", () => {
    const view = render(<ProviderMark id="mistral" />);
    const { svg, rects } = readEmblem(view, "svg");

    // The white file is reproduced as published: the rects carry Mistral's
    // own "white", not the slot's inherited colour, and the gradient
    // variant's stops are not reintroduced through another mechanism.
    expect(rects.every((rect) => rect[4] === "white")).toBe(true);
    expect(svg?.querySelector('[fill^="#"]')).toBeNull();
    expect(svg?.querySelector('linearGradient')).toBeNull();
    expect(svg?.querySelector('[fill^="url("]')).toBeNull();
  });

  it("shares one mark between the collapsed segment and the expanded card", () => {
    const [item] = floatingQuotaItems([mistral], now);

    const segmentView = render(segment(item));
    const { rects } = readEmblem(segmentView);

    const cardView = render(card(item));
    const cardSvg = cardView.container.querySelector(".fq-popover-mark svg");
    expect(cardSvg?.getAttribute("viewBox")).toBe(PUBLISHED_VIEWBOX);
    const cardRects = Array.from(cardSvg?.querySelectorAll("rect") ?? []).map(
      (rect) => [
        Number(rect.getAttribute("x")),
        Number(rect.getAttribute("y")),
        Number(rect.getAttribute("width")),
        Number(rect.getAttribute("height")),
        rect.getAttribute("fill"),
      ],
    );

    // One identity, two places: the expanded card wears the mark of the
    // segment it expands, pixel for pixel.
    expect(cardRects).toEqual(rects);
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
    }
  });
});
