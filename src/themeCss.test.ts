import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// Themes live entirely in styles.css custom properties; this keeps a
// half-added theme (new selector, missing tokens) from shipping, which no
// component-level assertion would catch without a DOM environment.
const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");

// The per-theme contract: every theme must define its own surfaces. Text and
// accent tones intentionally cascade unchanged from :root (readability never
// varies by theme), so they are deliberately absent from theme blocks.
const REQUIRED_SURFACE_TOKENS = [
  "--bg",
  "--bg-image",
  "--card",
  "--card-border",
  "--divider",
  "--bar-track",
  "--field-bg",
  "--card-backdrop",
];

/** Selector text → variable declarations for the :root-level theme blocks. */
function themeBlocks(): Map<string, string> {
  const withoutComments = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const blocks = new Map<string, string>();

  // Brace-aware scan so @supports/@media wrappers (and anything nested in
  // them) never masquerade as a theme's primary definition: the full token
  // set must sit in the main block, not behind a feature query.
  let cursor = 0;
  while (cursor < withoutComments.length) {
    const open = withoutComments.indexOf("{", cursor);
    if (open === -1) break;
    const selector = withoutComments.slice(cursor, open).trim();
    let depth = 1;
    let close = open + 1;
    while (close < withoutComments.length && depth > 0) {
      const char = withoutComments[close];
      if (char === "{") depth += 1;
      else if (char === "}") depth -= 1;
      close += 1;
    }
    const body = withoutComments.slice(open + 1, close - 1);
    cursor = close;

    if (selector.startsWith("@")) continue;
    // A theme block is only :root selectors; descendant overrides like
    // `:root[data-theme="oled"] .status-ok` are component rules, not tokens.
    const parts = selector.split(",").map((part) => part.trim());
    if (
      !parts.every((part) =>
        /^:root(\[data-theme="(graphite|glass|oled)"\])?$/.test(part),
      )
    ) {
      continue;
    }
    const theme = selector.includes('"glass"')
      ? "glass"
      : selector.includes('"oled"')
        ? "oled"
        : "graphite";
    blocks.set(theme, body);
  }
  return blocks;
}

/**
 * WCAG 2.1 contrast helpers. The faint text tier renders on card surfaces and
 * on the field-filled prediction grid, so the guardrails below measure the
 * token against every surface the stylesheet can actually paint instead of
 * against one hardcoded pair.
 */
function relativeLuminance(hexColor: string): number {
  const linear = [1, 3, 5]
    .map((offset) => parseInt(hexColor.slice(offset, offset + 2), 16) / 255)
    .map((channel) =>
      channel <= 0.03928
        ? channel / 12.92
        : ((channel + 0.055) / 1.055) ** 2.4,
    );
  const [r, g, b] = linear;
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrastRatio(a: string, b: string): number {
  const [lighter, darker] = [relativeLuminance(a), relativeLuminance(b)].sort(
    (x, y) => y - x,
  );
  return (lighter + 0.05) / (darker + 0.05);
}

function toHex(channels: number[]): string {
  return `#${channels
    .map((channel) => Math.round(channel).toString(16).padStart(2, "0"))
    .join("")}`;
}

/** Flattens a translucent surface (only glass declares them) onto its backdrop. */
function flatten(color: string, backdrop: string): string {
  const rgba = /rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?\)/.exec(color);
  if (!rgba) return color;
  const alpha = rgba[4] === undefined ? 1 : Number(rgba[4]);
  const backdropChannels = [1, 3, 5].map((offset) =>
    parseInt(backdrop.slice(offset, offset + 2), 16),
  );
  return toHex(
    [1, 2, 3].map(
      (index) =>
        Number(rgba[index]) * alpha + backdropChannels[index - 1] * (1 - alpha),
    ),
  );
}

describe("theme stylesheet", () => {
  it("defines all three themes", () => {
    expect([...themeBlocks().keys()].sort()).toEqual(
      ["glass", "graphite", "oled"],
    );
  });

  it("gives every theme the full surface token set", () => {
    for (const [theme, body] of themeBlocks()) {
      for (const token of REQUIRED_SURFACE_TOKENS) {
        expect(body, `${theme} is missing ${token}`).toContain(token);
      }
    }
  });

  it("keeps an opaque glass fallback for missing blur support", () => {
    expect(css).toContain("@supports not (backdrop-filter: blur(1px))");
    expect(css).toContain("@media (prefers-reduced-transparency: reduce)");
  });

  it("drops both animations under prefers-reduced-motion", () => {
    // The refresh spinner and the bar-fill width transition are the only
    // animated properties; both must be neutralized, not merely slowed.
    const block = css.match(
      /@media \(prefers-reduced-motion: reduce\) \{[\s\S]*?\n\}/,
    );
    expect(block).not.toBeNull();
    expect(block?.[0]).toContain(".refresh-btn.spinning svg");
    expect(block?.[0]).toContain("animation: none");
    expect(block?.[0]).toContain(".bar-fill");
    expect(block?.[0]).toContain("transition: none");
  });

  it("declares the reduced-motion overrides after the properties they neutralize", () => {
    // A media query adds no specificity, so an override placed above the base
    // declaration loses the cascade and motion is only apparently reduced.
    const spinner = css.indexOf("animation: spin 0.8s linear infinite");
    const bar = css.indexOf("transition: width 400ms ease");
    const overrides = css.indexOf("@media (prefers-reduced-motion: reduce)");
    expect(spinner).toBeGreaterThan(-1);
    expect(bar).toBeGreaterThan(-1);
    expect(overrides).toBeGreaterThan(spinner);
    expect(overrides).toBeGreaterThan(bar);
  });

  it("keeps the faint text tier at AA contrast on every theme", () => {
    // Regression guard for the #71717c -> #8a8a95 bump (3.4:1 -> 4.8:1 on
    // graphite cards); the value must never go back below AA.
    const faint = /--text-faint:\s*(#[0-9a-f]{6})/.exec(css)?.[1] ?? "";
    expect(faint).toBe("#8a8a95");
    expect(css).not.toContain("#71717c");

    // Glass composites its translucent surfaces over the theme background;
    // that background is darker than the darkest card on every theme, so it
    // is the right backdrop for the semi-transparent declarations.
    const glassBackdrop =
      /--bg:\s*(#[0-9a-f]{6})/.exec(themeBlocks().get("glass") ?? "")?.[1] ?? "";
    expect(glassBackdrop).toMatch(/^#[0-9a-f]{6}$/i);

    // Every declared card and field surface, including the opaque glass
    // fallbacks the @supports/@media blocks swap in.
    const surfaces = [...css.matchAll(/--(?:card|field-bg):\s*([^;]+);/g)].map(
      (match) => match[1].trim(),
    );
    expect(surfaces.length).toBeGreaterThanOrEqual(6);
    for (const surface of surfaces) {
      const ratio = contrastRatio(faint, flatten(surface, glassBackdrop));
      expect(
        ratio,
        `--text-faint on ${surface} is ${ratio.toFixed(2)}:1`,
      ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("gives every interactive control a visible keyboard focus ring", () => {
    const rule = css.match(
      /\.refresh-btn:focus-visible[\s\S]*?outline: [^;]+;/,
    );
    expect(rule).not.toBeNull();
    // One shared rule must cover all three interactive controls.
    expect(css).toContain(".refresh-btn:focus-visible,");
    expect(css).toContain(".setting-row select:focus-visible,");
    expect(css).toContain('.setting-row input[type="checkbox"]:focus-visible');
  });

  it("keeps a visible focus ring on every focusable control", () => {
    // The shared rule above covers the refresh button, both selects and the
    // checkbox. The clear-history button and the focusable projection block
    // declare their own, so a rule deleted there would still satisfy every
    // assertion above while leaving that control invisible to the keyboard.
    for (const control of [
      ".refresh-btn",
      ".setting-row select",
      '.setting-row input[type="checkbox"]',
      ".clear-history-btn",
      ".prediction",
    ]) {
      const at = css.indexOf(`${control}:focus-visible`);
      expect(at, `${control} has no :focus-visible rule`).toBeGreaterThan(-1);
      const body = css.slice(at, css.indexOf("}", at));
      const outline = /outline:\s*([^;]+);/.exec(body)?.[1]?.trim();
      expect(outline, `${control} has no outline`).toBeDefined();
      expect(outline, `${control} outline is invisible`).not.toMatch(
        /^(none|0)\b|transparent/,
      );
    }
  });
});

