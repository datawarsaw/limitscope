import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { DOCK_HEIGHT, DOCK_WIDTH } from "./lib/floatingQuota";
import { HALO_REMAINING_QUOTA_SCALE } from "./lib/quotaPresentation";

const css = readFileSync(new URL("./floating.css", import.meta.url), "utf8");

describe("floating quota bar accessibility guards", () => {
  it("keeps a visible focus ring on the compact controls", () => {
    expect(css).toContain(".fq-item:focus-visible");
    expect(css).toContain(".fq-actions button:focus-visible");
    expect(css).toContain(".fq-menu button:focus-visible");
    expect(css).toContain("outline: 2px solid var(--teal)");
  });

  it("drops motion under prefers-reduced-motion", () => {
    const block = css.match(
      /@media \(prefers-reduced-motion: reduce\) \{[\s\S]*\n\}/,
    );
    expect(block).not.toBeNull();
    expect(block?.[0]).toContain(".fq-meter-fill");
    expect(block?.[0]).toContain("transition: none");
    expect(block?.[0]).toContain(".fq-popover");
    expect(block?.[0]).toContain(".fq-menu");
    expect(block?.[0]).toContain("animation: none");

    const motion = css.indexOf("transition: width 240ms ease");
    const enter = css.indexOf("animation: fq-in 120ms ease");
    const overrides = css.indexOf("@media (prefers-reduced-motion: reduce)");
    expect(overrides).toBeGreaterThan(motion);
    expect(overrides).toBeGreaterThan(enter);
  });

  it("paints the shell from shared theme tokens", () => {
    expect(css).toContain("background: var(--card)");
    expect(css).toContain("border: 1px solid var(--card-border)");
    expect(css).toContain("background: var(--bar-track)");
    expect(css).toContain("color: var(--text)");
    expect(css).not.toContain("data-theme=\"graphite\"");
  });

  it("keeps the floating surface free of backdrop-filter", () => {
    // The floating window sits over the desktop, not over page content, so a
    // blur never renders anything through the transparent WebView2.
    expect(css).not.toContain("backdrop-filter");
  });

  it("keeps the floating document unscrollable so the pill can never scroll away", () => {
    // The window grows to the measured card instead; an overflow must clip,
    // never scroll the bar out of the viewport.
    expect(css).toMatch(
      /html\[data-window="floating"\],\s*\nhtml\[data-window="floating"\] body \{\s*\n  overflow: hidden;/,
    );
  });

  it("keeps the collapsed shell inside a drag-capable frame", () => {
    expect(css).toContain("height: 56px");
    const rootBlock = css.match(/\.fq-root \{[^}]*\}/)?.[0] ?? "";
    expect(rootBlock).toContain("padding: 4px");
    const noDragRules = css
      .split("}")
      .filter((rule) => rule.includes("app-region: no-drag"))
      .join("\n");
    expect(noDragRules).toContain(".fq-shell .fq-item");
    expect(noDragRules).toContain(".fq-popover");
    expect(noDragRules).not.toContain("fq-grip");
  });

  it("paints the grab band from shared theme tokens", () => {
    const gripBlock = css.match(/\.fq-grip::before \{[^}]*\}/)?.[0] ?? "";
    expect(gripBlock).toContain("var(--text-faint)");
  });
});

describe("floating quota bar motion guards", () => {
  it("glides the active selector while visible and keeps it inert to input", () => {
    const block = css.match(/\.fq-active \{[^}]*\}/)?.[0] ?? "";
    expect(block).toContain("position: absolute");
    expect(block).toContain("pointer-events: none");
    expect(block).toContain("transition: opacity 140ms ease");
    const onBlock = css.match(/\.fq-active\[data-on="true"\] \{[^}]*\}/)?.[0] ?? "";
    expect(onBlock).toContain("opacity: 1");
    expect(onBlock).toContain("transform 180ms ease-out");
    expect(onBlock).toContain("width 180ms ease-out");
  });

  it("eases the semantic quota color between Halo bands", () => {
    expect(css).toContain("@property --fq-quota");
    expect(css).toContain('syntax: "<color>"');
    expect(css).toContain("inherits: true");
    expect(css).toContain("transition: --fq-quota 240ms ease");
  });

  it("glides the card between segments and crossfades its content per provider", () => {
    const popover = css.match(/\.fq-popover \{[^}]*\}/)?.[0] ?? "";
    expect(popover).toContain("translate: var(--fq-align, 0px) 0");
    expect(popover).toContain("translate 180ms ease-out");
    expect(popover).toContain("animation: fq-in 120ms ease");
    expect(css).toContain(".fq-card {");
    expect(css).toContain("animation: fq-swap 140ms ease");
  });

  it("fades the card out on close without taking input", () => {
    const exiting = css.match(/\.fq-popover\.is-exiting \{[^}]*\}/)?.[0] ?? "";
    expect(exiting).toContain("animation: fq-out 120ms ease forwards");
    expect(exiting).toContain("pointer-events: none");
  });

  it("drops all of the new motion under prefers-reduced-motion", () => {
    const block = css.match(
      /@media \(prefers-reduced-motion: reduce\) \{[\s\S]*\n\}/,
    )?.[0];
    expect(block).not.toBeNull();
    expect(block).toContain(".fq-active[data-on=\"true\"]");
    expect(block).toContain(".fq-item");
    expect(block).toContain(".fq-meter");
    expect(block).toContain(".fq-card");
    expect(block).toContain(".fq-popover.is-exiting");
    expect(block).toContain("transition: none");
    expect(block).toContain("animation: none");
  });
});

describe("floating quota bar semantic color guards", () => {
  const QUOTA_LEVELS: ReadonlyArray<readonly [string, string | null]> = [
    ...HALO_REMAINING_QUOTA_SCALE.map(
      (band) => [band.level, band.color] as const,
    ),
    ["unavailable", null],
  ];

  it.each(QUOTA_LEVELS)(
    "maps the %s quota level for both the segment and its popover",
    (level) => {
      expect(css).toContain(`.fq-item[data-quota="${level}"]`);
      expect(css).toContain(`.fq-popover[data-quota="${level}"]`);
    },
  );

  it("paints the six Halo bands with the approved hex values; unavailable stays muted", () => {
    for (const band of HALO_REMAINING_QUOTA_SCALE) {
      const block =
        css.match(
          new RegExp(`\\.fq-item\\[data-quota="${band.level}"\\][\\s\\S]*?\\}`),
        )?.[0] ?? "";
      expect(block).toContain(band.color);
    }
    const unavailableBlock =
      css.match(/\.fq-item\[data-quota="unavailable"\][\s\S]*?\}/)?.[0] ?? "";
    expect(unavailableBlock).toContain("var(--text-faint)");
    // Both percent readouts share the semantic scale; the meter fill shares
    // it through the Halo gradient guarded below.
    expect(css).toContain("color: var(--fq-quota, var(--green))");
  });

  it("fills every meter with the Halo tonal gradient over a quiet dark track", () => {
    const fillBlock = css.match(/\.fq-meter-fill \{[^}]*\}/)?.[0] ?? "";
    expect(fillBlock).toContain("linear-gradient(");
    expect(fillBlock).toContain("90deg");
    expect(fillBlock).toContain(
      "color-mix(in oklab, var(--fq-quota, var(--green)) 55%, transparent)",
    );
    const trackBlock = css.match(/\.fq-meter \{[^}]*\}/)?.[0] ?? "";
    expect(trackBlock).toContain("height: 3px");
    expect(trackBlock).toContain("background: var(--bar-track)");
    expect(trackBlock).toContain("border-radius: 99px");
  });

  it("makes the primary card meter the strongest — taller, and the only glow", () => {
    const primaryBlock = css.match(/\.fq-meter-primary \{[^}]*\}/)?.[0] ?? "";
    expect(primaryBlock).toContain("height: 6px");
    expect(primaryBlock).toContain("box-shadow: 0 0 6px -2px");
    expect(primaryBlock).toContain(
      "color-mix(in oklab, var(--fq-quota, var(--green)) 30%, transparent)",
    );
    // Secondary meters stay thin and quiet: same ramp, no glow.
    const rowBlock = css.match(/\.fq-meter-row \{[^}]*\}/)?.[0] ?? "";
    expect(rowBlock).toContain("height: 3px");
    expect(rowBlock).not.toContain("box-shadow");
  });

  it("keeps provider identity on muted marks instead of status colors", () => {
    const markBlock = css.match(/\.fq-mark \{[^}]*\}/)?.[0] ?? "";
    expect(markBlock).toContain("var(--text-dim)");
    // No provider-specific accent variables remain on the floating surface.
    expect(css).not.toContain("--fq-accent");
    expect(css).not.toContain('[data-provider="openai-codex"]');
    expect(css).not.toContain('[data-provider="zai"]');
    expect(css).not.toContain('[data-provider="opencode-go"]');
    expect(css).not.toContain('[data-provider="antigravity"]');
  });

  it("keeps the collapsed bar countdown-free; secondary windows and account stay styled", () => {
    expect(css).not.toContain(".fq-reset");
    expect(css).toContain(".fq-windows");
    const accountBlock =
      css.match(/\.fq-popover-account \{[^}]*\}/)?.[0] ?? "";
    expect(accountBlock).toContain("var(--text-faint)");
    expect(accountBlock).toContain("font-size: 11px");
  });
});

describe("resting dock guards", () => {
  // floatingQuota.ts computes the strip's meter positions from these exact
  // layout constants; if any of them drifts, the resting meters drift off
  // the revealed bar's meters and the geometry tests lie.
  it("keeps the meter geometry sources aligned with floatingQuota.ts", () => {
    const rootBlock = css.match(/\.fq-root \{[^}]*\}/)?.[0] ?? "";
    expect(rootBlock).toContain("padding: 4px");
    expect(rootBlock).toContain("position: relative");
    const shellBlock = css.match(/\.fq-shell \{[^}]*\}/)?.[0] ?? "";
    expect(shellBlock).toContain("border: 1px solid var(--card-border)");
    expect(shellBlock).toContain("padding: 4px");
    expect(shellBlock).toContain("gap: 2px");
    const gripBlock = css.match(/\.fq-grip \{[^}]*\}/)?.[0] ?? "";
    expect(gripBlock).toContain("width: 12px");
    const itemBlock = css.match(/\.fq-item \{[^}]*\}/)?.[0] ?? "";
    expect(itemBlock).toContain("padding: 8px 10px");
    expect(itemBlock).toContain("grid-template-columns: 22px minmax(0, 1fr)");
    expect(itemBlock).toContain("column-gap: 8px");
  });

  it("paints a faint resting surface across the exact dock footprint", () => {
    const stripBlock = css.match(/\.fq-dock-strip \{[^}]*\}/)?.[0] ?? "";
    expect(stripBlock).toContain("position: absolute");
    // The strip holds the resting footprint while the reveal plays — the
    // native window has already grown to the bar while it fades out.
    expect(stripBlock).toContain(`width: ${DOCK_WIDTH}px`);
    expect(stripBlock).toContain(`height: ${DOCK_HEIGHT}px`);
    // Perceptible on a black desktop, still minimal: a barely-there fill and
    // one hairline round the footprint.
    expect(stripBlock).toContain("background: rgba(255, 255, 255, 0.04)");
    expect(stripBlock).toContain("border: 1px solid rgba(255, 255, 255, 0.08)");
    // No glow on the resting surface — the primary card meter keeps the only
    // shadow on the floating surface.
    expect(stripBlock).not.toContain("box-shadow");
  });

  it("renders the resting dock as a bare 3px meter strip", () => {
    const meterBlock = css.match(/\.fq-dock-meter \{[^}]*\}/)?.[0] ?? "";
    expect(meterBlock).toContain("height: 3px");
    expect(meterBlock).toContain("top: 6px");
    // No glow on the resting meters — the primary card meter keeps the only
    // one, and the strip never shows it.
    expect(meterBlock).not.toContain("box-shadow");
    const sepBlock = css.match(/\.fq-dock-sep \{[^}]*\}/)?.[0] ?? "";
    expect(sepBlock).toContain("width: 1px");
    expect(sepBlock).toContain("height: 5px");
    expect(sepBlock).toContain("var(--divider)");
  });

  it("shows no readouts, marks, or labels in the resting surface", () => {
    // The strip's only children are its own meter and separator classes;
    // nothing that could paint a percentage, a provider mark, or text.
    expect(css).toContain(".fq-dock-meter");
    expect(css).toContain(".fq-dock-sep");
    expect(css).not.toMatch(/\.fq-dock-strip[^{]*\.fq-percent/);
    expect(css).not.toMatch(/\.fq-dock-strip[^{]*\.fq-mark/);
    expect(css).not.toMatch(/\.fq-dock-strip[^{]*\.fq-readout/);
  });

  it("hides the pill while resting and reveals it with content motion only", () => {
    const resting = css.match(
      /\.fq-root\[data-dock-state="resting"\] \.fq-shell \{[^}]*\}/,
    )?.[0];
    expect(resting).toContain("visibility: hidden");

    const reveal = css.match(/@keyframes fq-dock-in \{[\s\S]*?\n\}/)?.[0] ?? "";
    expect(reveal).toContain("translateY(-3px)");
    const revealRule = css.match(
      /\.fq-root\[data-dock-state="revealing"\] \.fq-shell \{[^}]*\}/,
    )?.[0];
    expect(revealRule).toContain("animation: fq-dock-in 220ms ease-out");
    const exitRule = css.match(/\.fq-dock-strip\.is-exiting \{[^}]*\}/)?.[0];
    expect(exitRule).toContain("animation: fq-dock-out 180ms ease forwards");
    expect(exitRule).toContain("pointer-events: none");
  });

  it("drops all dock motion under prefers-reduced-motion", () => {
    const block = css.match(
      /@media \(prefers-reduced-motion: reduce\) \{[\s\S]*\n\}/,
    )?.[0];
    expect(block).not.toBeNull();
    expect(block).toContain(".fq-dock-meter");
    expect(block).toContain(".fq-dock-strip.is-exiting");
    expect(block).toContain('.fq-root[data-dock-state="revealing"] .fq-shell');
  });
});
