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
 * The published Mistral pixel-cat emblem, as it stands in Mistral's own
 * 2026 brand kit: Icon/Gradient/RGB/SVG/Mistral-Icon-Gradient-RGB.svg
 * (sha256 5aa0534fb52009c3ef9321544cd14d4b84ba34d4ee041abb3bdde9c3f2e765ca),
 * fetched from https://mistral.ai/cms-media/api/media/file/Mistral_Logos_2026.zip
 * and linked from https://mistral.ai/brand/ . The viewBox is the published
 * one, so the emblem keeps the brand's own clearspace.
 */
const PUBLISHED_VIEWBOX = "0 0 162 162";

const PUBLISHED_PIXELS: [number, number, number, number, string][] = [
  [31, 31, 20, 20, "#FFAF01"],
  [111, 31, 20, 20, "#FFAF01"],
  [71, 91, 20, 20, "#E61300"],
  [31, 91, 20, 20, "#E61300"],
  [111, 91, 20, 20, "#E61300"],
  [31, 51, 40, 20, "#FF8204"],
  [11, 111, 60, 20, "#C4001D"],
  [91, 111, 60, 20, "#C4001D"],
  [31, 71, 100, 20, "#FA500F"],
  [91, 51, 40, 20, "#FF8204"],
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

  it("keeps the published gradient instead of recolouring the mark", () => {
    const view = render(<ProviderMark id="mistral" />);
    const { svg, rects } = readEmblem(view, "svg");

    // The brand's preferred treatment, used exactly as published: the
    // emblem does not inherit currentColor the way the monochrome marks
    // in this file do, and no published stop was altered.
    expect(svg?.querySelector('[fill="currentColor"]')).toBeNull();
    expect(new Set(rects.map((rect) => rect[4]))).toEqual(
      new Set(["#FFAF01", "#FF8204", "#FA500F", "#E61300", "#C4001D"]),
    );
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
