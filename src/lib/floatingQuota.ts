import type { ProviderUsage } from "../types";
import { isUsableQuotaWindow, primaryQuotaWindow } from "./quotaStrip";
import {
  providerStatusPresentation,
  type ProviderStatusPresentation,
} from "./v03Integration";
import {
  quotaPresentation,
  type QuotaPerspective,
} from "./quotaPresentation";

/**
 * Closed window height. The pill sits in a 4px drag-capable frame instead of
 * the old 24px shadow gutter, so the collapsed bar is a true 64px surface.
 */
export const FLOATING_WINDOW_HEIGHT = 64;

export type FloatingOverlay = "none" | "detail" | "menu";

export type FloatingQuotaState = "healthy" | "stale" | "error" | "unavailable";

/** One usable quota window, as the expanded popover lists it. */
export type FloatingQuotaWindowRow = {
  label: string;
  usedPercent: number;
  resetAt: string | null;
};

/** The provider's primary countdown target: its longest meaningful reset. */
export type FloatingQuotaReset = {
  label: string;
  resetAt: string;
};

export type FloatingQuotaItem = {
  providerId: string;
  name: string;
  state: FloatingQuotaState;
  /** Whole percent of the highest usable window, or null when none exists. */
  percent: number | null;
  /** Canonical raw percentage when available; `percent` stays the compact readout. */
  usedPercent?: number | null;
  windowLabel: string | null;
  resetAt: string | null;
  /**
   * Usable windows in provider order; the expanded popover lists the ones
   * other than the primary reset as secondary rows.
   */
  windows: FloatingQuotaWindowRow[];
  /**
   * The quota reset the detail card features as its primary countdown.
   * The generic rule is the longest meaningful reset (monthly over weekly
   * over a 5-hour window, by actual reset distance — never parsed from
   * labels); Google Antigravity instead follows its primary quota window
   * (see primaryResetTarget). The collapsed bar stays countdown-free.
   */
  primaryReset: FloatingQuotaReset | null;
  /**
   * Display-safe account identity when the provider proved one; the popover
   * keeps it muted on its own bottom line.
   */
  accountLabel?: string;
  /** Snapshot is stale even when an error marker is also showing. */
  stale: boolean;
  statusLabel: ProviderStatusPresentation["label"];
  statusClass: ProviderStatusPresentation["className"];
  error?: string;
};

export type FloatingClickAction = {
  type: "quick-view";
  providerId: string;
};

/**
 * Logical width of the floating bar. Four providers land at 520px and five
 * cap at 600px, without a layout that only fits the current registry.
 */
export function floatingBarWidth(providerCount: number): number {
  const count = Math.max(providerCount, 1);
  return Math.min(600, Math.max(300, 40 + count * 120));
}

// ---------- Resting dock (top edge) ----------

/** Logical size of the resting dock: a meter-only strip, five 3px meters. */
export const DOCK_WIDTH = 600;
export const DOCK_HEIGHT = 16;
/** The resting meters keep the revealed bar's 3px meter height. */
export const DOCK_METER_HEIGHT = 3;

/** One meter's horizontal extent, in logical px from the window's left edge. */
export type MeterRect = { left: number; width: number };

// The production Halo bar's layout constants that meter geometry depends on.
// They mirror floating.css and are pinned there by floating.css.test.ts, so
// the resting dock's computed meter positions can never silently drift from
// the revealed bar's real ones.
const ROOT_PAD = 4; // .fq-root padding
const SHELL_BORDER = 1; // .fq-shell border
const SHELL_PAD = 4; // .fq-shell padding
const SHELL_GAP = 2; // .fq-shell gap between grip and items
const GRIP_WIDTH = 12; // .fq-grip
const ITEM_PAD_X = 10; // .fq-item horizontal padding
const MARK_COL = 22; // .fq-item mark column
const ITEM_COL_GAP = 8; // .fq-item column-gap between mark and meter column

/** Horizontal distance from an item's left edge to its meter's left edge. */
const METER_INSET = ITEM_PAD_X + MARK_COL + ITEM_COL_GAP;

/**
 * Meter x-geometry of the production Halo bar for a window of `windowWidth`
 * logical px hosting `providerCount` segments: one rect per provider, in
 * provider order. Pure math over the same constants floating.css lays the
 * real bar out with — the resting dock uses it to sit exactly on the meters
 * the revealed bar will show.
 */
export function haloMeterRects(
  windowWidth: number,
  providerCount: number,
): MeterRect[] {
  const count = Math.max(1, Math.floor(providerCount));
  const shellContent =
    windowWidth - ROOT_PAD * 2 - SHELL_BORDER * 2 - SHELL_PAD * 2;
  const itemWidth = (shellContent - GRIP_WIDTH - count * SHELL_GAP) / count;
  const firstItemLeft =
    ROOT_PAD + SHELL_BORDER + SHELL_PAD + GRIP_WIDTH + SHELL_GAP;
  return Array.from({ length: count }, (_, index) => ({
    left: firstItemLeft + index * (itemWidth + SHELL_GAP) + METER_INSET,
    width: itemWidth - METER_INSET - ITEM_PAD_X,
  }));
}

/**
 * Meter rects for the resting dock. The strip is a fixed 600px wide while
 * the revealed bar may be narrower (hidden providers shrink the production
 * width), so each rect is offset by half the difference: the reveal re-centers
 * the window, and with this offset every resting meter stays stationary on
 * screen while the bar fades in underneath it.
 */
export function dockMeterRects(providerCount: number): MeterRect[] {
  const revealedWidth = floatingBarWidth(providerCount);
  const offset = (DOCK_WIDTH - revealedWidth) / 2;
  return haloMeterRects(revealedWidth, providerCount).map((rect) => ({
    left: offset + rect.left,
    width: rect.width,
  }));
}

/**
 * Subtle separator positions between the resting meters, one per gap. They
 * sit on the revealed bar's own segment boundaries (each is `METER_INSET`
 * left of the meter that follows it), so the strip reads as a compressed
 * continuation of the bar rather than an evenly ruled ruler.
 */
export function dockSeparatorLefts(providerCount: number): number[] {
  const rects = dockMeterRects(providerCount);
  return rects.slice(1).map((rect) => rect.left - METER_INSET);
}

/** Extra logical height while the shared detail card or menu is open. For
 * the detail card the caller passes the rendered card height when known: the
 * window reserves exactly the card plus its frame (8px flex gap above the
 * card, 4px bottom frame), so no provider's card can overflow the window.
 * The static headroom is only the pre-measurement fallback; it must stay at
 * or above the approved mockup's two-window card so the window never needs
 * to shrink between render and measurement. */
export const DETAIL_CARD_FALLBACK_EXTRA = 292;

/** Frame around the measured card: 8px column gap + 4px root bottom padding. */
export const DETAIL_CARD_FRAME = 12;

export function floatingOverlayExtra(
  overlay: FloatingOverlay,
  detailCardHeight?: number | null,
): number {
  if (overlay === "detail") {
    return detailCardHeight && detailCardHeight > 0
      ? Math.ceil(detailCardHeight) + DETAIL_CARD_FRAME
      : DETAIL_CARD_FALLBACK_EXTRA;
  }
  if (overlay === "menu") return 168;
  return 0;
}

export function floatingWindowHeight(
  overlay: FloatingOverlay,
  detailCardHeight?: number | null,
): number {
  return FLOATING_WINDOW_HEIGHT + floatingOverlayExtra(overlay, detailCardHeight);
}

/** Horizontal anchor for the detail card: it opens under its own provider
 * segment and is clamped so neither card edge pokes past the bar. Segments
 * near the right edge pull the card left; a degenerate shell keeps it at 0. */
export function clampPopoverAlign(
  segmentOffsetLeft: number,
  shellWidth: number,
  cardWidth: number,
): number {
  const maxLeft = Math.max(0, shellWidth - cardWidth);
  return Math.min(Math.max(0, segmentOffsetLeft), maxLeft);
}

/** Click pins the shared detail card open. It never navigates by itself. */
export function floatingClickAction(item: FloatingQuotaItem): FloatingClickAction {
  return { type: "quick-view", providerId: item.providerId };
}

/**
 * Spoken label for the compact control. Includes stale and error when set.
 * The floating surface always speaks the remaining perspective, matching
 * what the segment shows.
 */
export function floatingItemLabel(item: FloatingQuotaItem): string {
  const presentation = quotaPresentation(
    item.usedPercent ?? item.percent,
    FLOATING_QUOTA_PERSPECTIVE,
  );
  const value = presentation.severityLabel
    ? `${presentation.severityLabel} · ${presentation.label}`
    : presentation.label;
  const parts = [item.name];
  parts.push(value);
  if (item.windowLabel) parts.push(item.windowLabel);
  if (item.state === "error") parts.push("error");
  if (item.stale) parts.push("stale");
  else if (item.statusLabel !== "Live" && item.state !== "unavailable") {
    parts.push(item.statusLabel);
  }
  return parts.join(", ");
}

/**
 * The floating bar's fixed perspective: it answers "how much do I have
 * left?", independent of the dashboard's display preference.
 */
export const FLOATING_QUOTA_PERSPECTIVE: QuotaPerspective = "remaining";

/**
 * Primary countdown target: the usable window whose reset lies farthest in
 * the future. Reset distance is what "monthly over weekly over a 5-hour
 * window" means without parsing labels, and it self-corrects as a long
 * window approaches its reset. Ties keep the earliest window in the
 * provider's own order. Windows without a reset time never qualify.
 */
export function primaryResetWindow(
  usage: ProviderUsage,
): FloatingQuotaReset | null {
  let primary: FloatingQuotaReset | null = null;
  for (const limit of usage.limits) {
    if (!isUsableQuotaWindow(limit)) continue;
    if (!limit.resetAt || Number.isNaN(Date.parse(limit.resetAt))) continue;
    if (!primary || limit.resetAt > primary.resetAt) {
      primary = { label: limit.label.trim(), resetAt: limit.resetAt };
    }
  }
  return primary;
}

/** The provider whose detail card follows its primary quota window instead
 * of the generic longest-reset rule. */
const ANTIGRAVITY_PROVIDER_ID = "antigravity";

/**
 * The detail card's primary countdown target.
 *
 * Google Antigravity reports two model families as one provider and their
 * weekly windows reset days apart, so reaching across families for the
 * farthest reset pairs the headline number (one family) with a countdown
 * for the other. Antigravity's countdown therefore describes the primary
 * quota window itself — the same window the headline number comes from.
 * Every other provider keeps the longest-reset rule, which is also the
 * fallback when Antigravity's primary window carries no usable reset time.
 */
export function primaryResetTarget(usage: ProviderUsage): FloatingQuotaReset | null {
  if (usage.id !== ANTIGRAVITY_PROVIDER_ID) return primaryResetWindow(usage);
  const primary = primaryQuotaWindow(usage);
  if (primary?.resetAt && !Number.isNaN(Date.parse(primary.resetAt))) {
    return { label: primary.label.trim(), resetAt: primary.resetAt };
  }
  return primaryResetWindow(usage);
}

function isStaleSnapshot(usage: ProviderUsage): boolean {
  return usage.health === "stale" || usage.dataFreshness === "stale";
}

function stateFor(
  status: ProviderStatusPresentation,
  hasWindow: boolean,
): FloatingQuotaState {
  if (!hasWindow) {
    return status.className === "error" ? "error" : "unavailable";
  }
  if (status.className === "error") return "error";
  // Unknown still has a number, but it must not read as a confirmed live value.
  if (status.className === "stale" || status.className === "unknown") return "stale";
  return "healthy";
}

/**
 * Compact rows for the floating bar, in registry order.
 *
 * The number comes from primaryQuotaWindow: highest usable used-percent,
 * malformed windows ignored, never an average or a sum. The countdown target
 * comes from primaryResetWindow: the longest meaningful reset. Simulated
 * providers are omitted. A repeated id keeps the first real usage. A
 * production provider with no usable window stays on the bar as unavailable.
 */
export function floatingQuotaItems(
  usages: readonly ProviderUsage[],
): FloatingQuotaItem[] {
  const items: FloatingQuotaItem[] = [];
  const seen = new Set<string>();
  for (const usage of usages) {
    if (usage.simulated) continue;
    if (seen.has(usage.id)) continue;
    seen.add(usage.id);

    const status = providerStatusPresentation(usage);
    const primary = primaryQuotaWindow(usage);
    const hasWindow = primary !== undefined;
    const windows: FloatingQuotaWindowRow[] = usage.limits
      .filter(isUsableQuotaWindow)
      .map((limit) => ({
        label: limit.label.trim(),
        usedPercent: limit.usedPercent,
        resetAt: limit.resetAt ?? null,
      }));
    items.push({
      providerId: usage.id,
      name: usage.name,
      state: stateFor(status, hasWindow),
      percent: hasWindow ? Math.round(primary.usedPercent) : null,
      usedPercent: hasWindow ? primary.usedPercent : null,
      windowLabel: hasWindow ? primary.label.trim() : null,
      resetAt: hasWindow && primary.resetAt ? primary.resetAt : null,
      windows,
      primaryReset: primaryResetTarget(usage),
      ...(usage.account?.label ? { accountLabel: usage.account.label } : {}),
      stale: isStaleSnapshot(usage) || status.className === "stale",
      statusLabel: status.label,
      statusClass: status.className,
      ...(usage.error ? { error: usage.error } : {}),
    });
  }
  return items;
}
