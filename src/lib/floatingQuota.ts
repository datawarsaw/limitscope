import type {
  ApiWallet,
  LimitWindow,
  ProviderUsage,
} from "../types";
import { isUsableQuotaWindow } from "./quotaStrip";
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
  resetText?: string;
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
  /** Whole percent of the selected primary window (see floatingPrimaryWindow),
   * or null when none exists. */
  percent: number | null;
  /** Canonical raw percentage when available; `percent` stays the compact readout. */
  usedPercent?: number | null;
  windowLabel: string | null;
  resetAt: string | null;
  resetText?: string;
  /**
   * Usable windows in provider order; the expanded popover lists the ones
   * other than the primary reset as secondary rows.
   */
  windows: FloatingQuotaWindowRow[];
  /**
   * The quota reset the detail card features as its primary countdown: the
   * selected primary window's own reset (see primaryResetTarget), so the
   * headline number and the countdown always describe the same window. The
   * collapsed bar stays countdown-free.
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
 * Wallet presentation state. It deliberately names its own set rather than
 * reusing `FloatingQuotaState`: a wallet can be in an upstream cooldown with
 * a perfectly good last-good balance, which no quota state expresses.
 */
export type FloatingWalletState =
  | "healthy"
  | "stale"
  | "cooldown"
  | "error"
  | "unavailable";

/** One prepaid API wallet as the floating slot and its card show it. */
export type FloatingWalletItem = {
  id: string;
  name: string;
  state: FloatingWalletState;
  /** The wallet's balances, one entry per currency, in source order.
   *
   * Currencies are never merged: an empty list means no trustworthy reading
   * exists (never a zero), and a wallet holding both USD and CNY keeps two
   * entries that no consumer may add together. */
  balances: FloatingWalletBalance[];
  checkedAt: string | null;
  identity?: string;
  error?: string;
  errorCategory?: string;
  errorHttpStatus?: number;
  statusLabel: string;
};

/** One currency's balance inside a wallet, as the floating surface shows it. */
export type FloatingWalletBalance = {
  /** ISO currency code exactly as the source reported it. */
  currency: string;
  /** The remaining amount in `currency`. */
  amount: number;
  /** The source's own breakdown of `amount`, when it reports one. */
  granted?: number;
  /** The topped-up part of `amount`, when the source reports one. */
  toppedUp?: number;
};

// The production Halo bar's layout constants that meter geometry depends on.
// They mirror floating.css and are pinned there by floating.css.test.ts, so
// the resting dock's computed meter positions can never silently drift from
// the revealed bar's real ones.
const ROOT_PAD = 4; // .fq-root padding
const SHELL_BORDER = 1; // .fq-shell border
const SHELL_PAD = 4; // .fq-shell padding
const SHELL_GAP = 2; // .fq-shell gap between grip, items and the wallet slot
const GRIP_WIDTH = 12; // .fq-grip
const ITEM_PAD_X = 10; // .fq-item horizontal padding
const MARK_COL = 22; // .fq-item mark column
const ITEM_COL_GAP = 8; // .fq-item column-gap between mark and meter column

/** The compact wallet slot's own width, in logical px (Variant B). */
export const WALLET_SLOT_WIDTH = 44;

/**
 * Logical width of the floating bar. Four providers land at 520px and five
 * cap at 600px, without a layout that only fits the current registry. A
 * wallet adds its own fixed slot plus one shell gap to the right of the
 * providers, so five providers plus OpenRouter reveal at 646px while every
 * provider segment keeps the exact geometry it had at 600px.
 */
export function floatingBarWidth(
  providerCount: number,
  hasWallet = false,
): number {
  const count = Math.max(providerCount, 1);
  const width = Math.min(600, Math.max(300, 40 + count * 120));
  return width + (hasWallet ? WALLET_SLOT_WIDTH + SHELL_GAP : 0);
}

// ---------- Resting dock (top edge) ----------

/** Logical size of the resting dock: a meter-only strip, five 3px meters. */
export const DOCK_WIDTH = 600;
export const DOCK_HEIGHT = 16;
/** The resting meters keep the revealed bar's 3px meter height. */
export const DOCK_METER_HEIGHT = 3;

/** One meter's horizontal extent, in logical px from the window's left edge. */
export type MeterRect = { left: number; width: number };

/** Horizontal distance from an item's left edge to its meter's left edge. */
const METER_INSET = ITEM_PAD_X + MARK_COL + ITEM_COL_GAP;

function meterInset(providerCount: number): number {
  return providerCount >= 7 ? 6 + 18 + 4 : METER_INSET;
}

/**
 * Shared flex layout of the revealed shell: the width every provider
 * segment gets and where the first one starts. When a wallet occupies the
 * rightmost slot, its own fixed width and gap come out of the shell content
 * before the providers divide the rest — which is why five providers keep
 * their 112px segments and identical positions at 646px.
 */
function shellLayout(
  windowWidth: number,
  providerCount: number,
  hasWallet: boolean,
): { count: number; itemWidth: number; firstItemLeft: number; shellContent: number } {
  const count = Math.max(1, Math.floor(providerCount));
  const shellContent =
    windowWidth - ROOT_PAD * 2 - SHELL_BORDER * 2 - SHELL_PAD * 2;
  const walletExtra = hasWallet ? WALLET_SLOT_WIDTH + SHELL_GAP : 0;
  const itemWidth =
    (shellContent - GRIP_WIDTH - count * SHELL_GAP - walletExtra) / count;
  const firstItemLeft =
    ROOT_PAD + SHELL_BORDER + SHELL_PAD + GRIP_WIDTH + SHELL_GAP;
  return { count, itemWidth, firstItemLeft, shellContent };
}

/**
 * Meter x-geometry of the production Halo bar for a window of `windowWidth`
 * logical px hosting `providerCount` segments: one rect per provider, in
 * provider order. Pure math over the same constants floating.css lays the
 * real bar out with — the resting dock uses it to sit exactly on the meters
 * the revealed bar will show. A wallet has no quota meter, so `hasWallet`
 * only affects the segment widths; the returned array stays one rect per
 * provider.
 */
export function haloMeterRects(
  windowWidth: number,
  providerCount: number,
  hasWallet = false,
): MeterRect[] {
  const { count, itemWidth, firstItemLeft } = shellLayout(
    windowWidth,
    providerCount,
    hasWallet,
  );
  // Seven providers use the compact item geometry in floating.css. Keep the
  // resting dock's meters aligned with that same rendered geometry.
  const inset = meterInset(count);
  const rightPad = count >= 7 ? 6 : ITEM_PAD_X;
  return Array.from({ length: count }, (_, index) => ({
    left: firstItemLeft + index * (itemWidth + SHELL_GAP) + inset,
    width: itemWidth - inset - rightPad,
  }));
}

/**
 * Where the wallet slot sits inside a revealed shell of `windowWidth` that
 * hosts `providerCount` providers: immediately right of the last provider,
 * one shell gap away, at its own fixed 44px. Pure geometry — the slot itself
 * renders in normal flex flow and never carries a quota meter.
 */
export function haloWalletRect(
  windowWidth: number,
  providerCount: number,
): MeterRect {
  const { count, itemWidth, firstItemLeft } = shellLayout(
    windowWidth,
    providerCount,
    true,
  );
  return {
    left: firstItemLeft + count * (itemWidth + SHELL_GAP),
    width: WALLET_SLOT_WIDTH,
  };
}

/**
 * Meter rects for the resting dock. The strip is a fixed 600px wide while
 * the revealed bar may be narrower (hidden providers shrink the production
 * width), so each rect is offset by half the difference: the reveal re-centers
 * the window, and with this offset every resting meter stays stationary on
 * screen while the bar fades in underneath it.
 */
export function dockMeterRects(
  providerCount: number,
  hasWallet = false,
): MeterRect[] {
  const revealedWidth = floatingBarWidth(providerCount, hasWallet);
  const offset = (DOCK_WIDTH - revealedWidth) / 2;
  return haloMeterRects(revealedWidth, providerCount, hasWallet).map((rect) => ({
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
export function dockSeparatorLefts(
  providerCount: number,
  hasWallet = false,
): number[] {
  const rects = dockMeterRects(providerCount, hasWallet);
  return rects.slice(1).map((rect) => rect.left - meterInset(providerCount));
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
 * window" means without parsing labels. Used as the countdown fallback when
 * the selected primary window carries no reset time of its own (see
 * primaryResetTarget). Ties keep the earliest window in the provider's own
 * order. Windows without a reset time never qualify.
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

// ---------- floating primary quota selection ----------

/**
 * Each provider's designated long-term quota, named by the exact label its
 * adapter composes for that window: Codex's 7-day limit, Antigravity's Gemini
 * weekly pool, Grok's weekly credits, OpenCode Go's 30-day limit, and Z.ai's
 * weekly pool. These are the plan-level budgets the floating bar answers
 * "how much long-term quota do I have left?" with — not whichever window
 * happens to be most depleted right now.
 */
const FLOATING_PRIMARY_LABELS: Readonly<Record<string, readonly string[]>> = {
  "openai-codex": ["Weekly"],
  antigravity: ["Gemini Weekly"],
  grok: ["Weekly credits"],
  "opencode-go": ["30-day"],
  zai: ["Weekly"],
  "qwen-token-plan": ["Monthly"],
  opendesign: ["7-day"],
};

const MINUTE_SECONDS = 60;
const HOUR_SECONDS = 60 * MINUTE_SECONDS;
const DAY_SECONDS = 24 * HOUR_SECONDS;
const WEEK_SECONDS = 7 * DAY_SECONDS;

/**
 * The period a window's label declares, in seconds. The label is the period
 * metadata the adapters already put on the wire (Codex composes labels from
 * upstream window seconds, OpenCode Go from its weekly/monthly fields, Z.ai
 * from the credit-pool unit, Antigravity from its weekly window key); 0 means
 * the label declares no period ("Tokens", "On-demand", a sanitized fallback),
 * ranking such windows below every real period. Reset distance is
 * deliberately not consulted: a weekly window nearing its reset stays weekly.
 */
function declaredPeriodSeconds(rawLabel: string): number {
  const label = rawLabel.trim().toLowerCase();
  const sized = /^(\d+)-minute$|^(\d+)-hour$|^(\d+)-day$/.exec(label);
  if (sized) {
    if (sized[1]) return Number(sized[1]) * MINUTE_SECONDS;
    if (sized[2]) return Number(sized[2]) * HOUR_SECONDS;
    return Number(sized[3]) * DAY_SECONDS;
  }
  if (label === "hourly") return HOUR_SECONDS;
  if (label === "daily") return DAY_SECONDS;
  if (label === "monthly" || label.startsWith("monthly ")) {
    return 30 * DAY_SECONDS;
  }
  if (label === "weekly" || label.startsWith("weekly ") || label.endsWith(" weekly")) {
    return WEEK_SECONDS;
  }
  return 0;
}

function isPreferredLabel(
  label: string,
  preferred: readonly string[] | undefined,
): boolean {
  if (!preferred) return false;
  const normalized = label.trim().toLowerCase();
  return preferred.some((candidate) => candidate.toLowerCase() === normalized);
}

/**
 * The floating bar's primary quota window, deterministic per snapshot:
 *
 * 1. The provider's designated long-term window (FLOATING_PRIMARY_LABELS),
 *    first match in provider order. Antigravity's families stay separate
 *    here: "Gemini Weekly" is the primary even when the Claude family's
 *    windows report heavier usage — never a max across families.
 * 2. Otherwise the usable window whose declared period is longest, ties kept
 *    in provider order — the closest stand-in for the designated budget.
 * 3. Malformed windows are ignored throughout (isUsableQuotaWindow), and
 *    percentages are never averaged or combined across windows.
 *
 * Deliberately scoped to the floating window: the dashboard keeps its own
 * most-depleted primary (primaryQuotaWindow), which intentionally highlights
 * the window closest to its limit.
 */
export function floatingPrimaryWindow(
  usage: ProviderUsage,
): LimitWindow | undefined {
  const preferred = FLOATING_PRIMARY_LABELS[usage.id];
  let fallback: LimitWindow | undefined;
  let fallbackPeriod = -1;
  for (const limit of usage.limits) {
    if (!isUsableQuotaWindow(limit)) continue;
    const label = limit.label.trim();
    if (isPreferredLabel(label, preferred)) return limit;
    const period = declaredPeriodSeconds(label);
    if (period > fallbackPeriod) {
      fallback = limit;
      fallbackPeriod = period;
    }
  }
  return fallback;
}

/**
 * The detail card's primary countdown target: the selected primary window's
 * own reset (floatingPrimaryWindow), so the headline number, its identity,
 * and the countdown always describe the same window — including Antigravity,
 * whose Gemini and Claude families report weekly resets days apart. When
 * that window carries no usable reset time, the countdown falls back to the
 * longest meaningful reset (primaryResetWindow) rather than disappearing.
 */
export function primaryResetTarget(usage: ProviderUsage): FloatingQuotaReset | null {
  const primary = floatingPrimaryWindow(usage);
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
 * The number comes from floatingPrimaryWindow: the provider's designated
 * long-term window, or the longest declared period when that window is
 * unavailable — malformed windows ignored, never an average or a sum. The
 * countdown target follows the same window (primaryResetTarget), falling
 * back to the longest meaningful reset only when it carries no reset time.
 * Simulated providers are omitted. A repeated id keeps the first real
 * usage. A production provider with no usable window stays on the bar as
 * unavailable.
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
    const primary = floatingPrimaryWindow(usage);
    const hasWindow = primary !== undefined;
    const windows: FloatingQuotaWindowRow[] = usage.limits
      .filter(isUsableQuotaWindow)
      .map((limit) => ({
        label: limit.label.trim(),
        usedPercent: limit.usedPercent,
        resetAt: limit.resetAt ?? null,
        ...(limit.resetText ? { resetText: limit.resetText } : {}),
      }));
    items.push({
      providerId: usage.id,
      name: usage.name,
      state: stateFor(status, hasWindow),
      percent: hasWindow ? Math.round(primary.usedPercent) : null,
      usedPercent: hasWindow ? primary.usedPercent : null,
      windowLabel: hasWindow ? primary.label.trim() : null,
      resetAt: hasWindow && primary.resetAt ? primary.resetAt : null,
      ...(primary?.resetText ? { resetText: primary.resetText } : {}),
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

// ---------- API wallets ----------

function walletStateFor(
  health: ApiWallet["health"],
  hasBalance: boolean,
): FloatingWalletState {
  switch (health) {
    case "live":
      // A "live" wallet without a balance states nothing: the only honest
      // presentation is unavailable, never a fabricated $0.00.
      return hasBalance ? "healthy" : "unavailable";
    case "stale":
      return "stale";
    case "cooldown":
      return "cooldown";
    case "error":
      return "error";
    case "unavailable":
    case "unknown":
      return "unavailable";
  }
}

function walletStatusLabel(
  state: FloatingWalletState,
  httpStatus: number | undefined,
): string {
  switch (state) {
    case "healthy":
      return "Live";
    case "stale":
      return "Stale";
    case "cooldown":
      return "Cooling down";
    case "error":
      return httpStatus === 429 ? "Rate limited" : "Refresh failed";
    case "unavailable":
      return "Unavailable";
  }
}

/**
 * Map one wire wallet entry to its floating presentation. The balance is
 * carried per currency, and only when it is a finite, non-negative upstream
 * number in a named currency; anything else is dropped and the wallet is
 * shown as unavailable. A real $0.00 survives unchanged because zero is a
 * valid balance, not missing data.
 *
 * The legacy `remainingUsd` field is honored only when the wire carried no
 * `balances` list, so an older runtime snapshot still projects one USD
 * balance. It is never used to invent a balance that `balances` contradicts.
 */
export function floatingWalletItem(wallet: ApiWallet): FloatingWalletItem {
  const balances = walletBalances(wallet);
  const checkedAt =
    wallet.checkedAt && !Number.isNaN(Date.parse(wallet.checkedAt))
      ? wallet.checkedAt
      : null;
  const state = walletStateFor(wallet.health, balances.length > 0);
  return {
    id: wallet.id,
    name: wallet.name,
    state,
    balances,
    checkedAt,
    ...(wallet.identity ? { identity: wallet.identity } : {}),
    ...(wallet.error ? { error: wallet.error } : {}),
    ...(wallet.errorCategory ? { errorCategory: wallet.errorCategory } : {}),
    ...(wallet.errorHttpStatus !== undefined
      ? { errorHttpStatus: wallet.errorHttpStatus }
      : {}),
    statusLabel: walletStatusLabel(state, wallet.errorHttpStatus),
  };
}

/**
 * The wallet's balances, in wire order, with every untrustworthy amount
 * dropped.
 *
 * An amount is kept only when it is a finite, non-negative number in a named
 * currency. The legacy `remainingUsd` field is a fallback for an older
 * runtime that sent no `balances` list at all; a `balances` list that is
 * present but empty is authoritative (no balance), never a reason to read
 * the legacy field.
 */
function walletBalances(wallet: ApiWallet): FloatingWalletBalance[] {
  const balances: FloatingWalletBalance[] = [];
  for (const balance of wallet.balances ?? []) {
    const currency = balance.currency?.trim();
    if (!currency) continue;
    if (
      typeof balance.amount !== "number" ||
      !Number.isFinite(balance.amount) ||
      balance.amount < 0
    ) {
      continue;
    }
    balances.push({
      currency,
      amount: balance.amount,
      ...(typeof balance.granted === "number" &&
      Number.isFinite(balance.granted)
        ? { granted: balance.granted }
        : {}),
      ...(typeof balance.toppedUp === "number" &&
      Number.isFinite(balance.toppedUp)
        ? { toppedUp: balance.toppedUp }
        : {}),
    });
  }
  if (balances.length > 0 || wallet.balances !== undefined) {
    return balances;
  }
  // An older runtime snapshot: one USD balance, if the legacy field carries
  // a trustworthy amount.
  const legacy = wallet.remainingUsd;
  if (
    typeof legacy === "number" &&
    Number.isFinite(legacy) &&
    legacy >= 0
  ) {
    return [{ currency: "USD", amount: legacy }];
  }
  return [];
}

/** Wallet items in snapshot order, one per id. */
export function floatingWalletItems(
  wallets: readonly ApiWallet[],
): FloatingWalletItem[] {
  const items: FloatingWalletItem[] = [];
  const seen = new Set<string>();
  for (const wallet of wallets) {
    const id = wallet.id?.trim();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    items.push(floatingWalletItem({ ...wallet, id }));
  }
  return items;
}

/**
 * The detail id of the shared API Wallets card.
 *
 * The floating bar has exactly one wallet slot, whatever the number of
 * wallets the runtime reports: the slot is a shared entry point into the
 * API Wallets card, never one segment per wallet. The card lists every
 * wallet it knows about.
 */
export const API_WALLETS_DETAIL_ID = "api-wallets";

/**
 * The aggregate state of the shared wallet slot.
 *
 * With several wallets the slot has one status dot, so it states the most
 * consequential state among them, in the order the dot's own colors read:
 * an error (red) outranks a cooldown or a stale reading (amber), which
 * outranks an unavailable wallet (muted), which outranks a live one. A
 * muted wallet therefore never hides a red error. An empty wallet list has
 * no state to show.
 */
export function floatingWalletSlotState(
  items: readonly FloatingWalletItem[],
): FloatingWalletState | null {
  if (items.length === 0) return null;
  // The cooldown key is quoted on purpose: the runtime-trigger guardrail
  // reads an unquoted `cooldown` identifier as cooldown policy leaking into
  // TS, while the quoted member is the documented wire vocabulary.
  const rank: Record<FloatingWalletState, number> = {
    healthy: 0,
    unavailable: 1,
    stale: 2,
    "cooldown": 3,
    error: 4,
  };
  return items.reduce<FloatingWalletState>(
    (worst, item) => (rank[item.state] > rank[worst] ? item.state : worst),
    "healthy",
  );
}

/**
 * The spoken label of the shared wallet slot: every wallet's amount, one
 * after another, each with its own currency.
 *
 * There is no combined total: a wallet holding both USD and CNY announces
 * two amounts, because their sum would be meaningless.
 */
export function floatingWalletSlotLabel(
  items: readonly FloatingWalletItem[],
): string {
  if (items.length === 0) return "API wallets, no wallet available";
  const parts = items.map((item) => {
    const amounts = item.balances
      .map(floatingWalletAmountText)
      .filter((text): text is string => text !== null);
    const balance = amounts.length > 0 ? amounts.join(", ") : null;
    return `${item.name} ${balance ? `${balance} remaining` : "balance unavailable"}`;
  });
  return `API wallets, ${parts.join("; ")}`;
}

/**
 * The money readout for one currency, always two decimals and always with
 * its currency named.
 *
 * The currency travels with the amount: a CNY balance reads as `CNY 110.00`,
 * never as `$110.00`. Null means there is no trustworthy amount to state —
 * the one thing the surface never does is claim $0.00.
 */
export function floatingWalletAmountText(
  balance: FloatingWalletBalance,
): string | null {
  if (
    typeof balance.amount !== "number" ||
    !Number.isFinite(balance.amount) ||
    balance.amount < 0 ||
    !balance.currency?.trim()
  ) {
    return null;
  }
  return `${balance.currency.trim()} ${balance.amount.toFixed(2)}`;
}

/**
 * The money readout of a wallet that holds exactly one currency, or null
 * when it holds none or several.
 *
 * A multi-currency wallet has no single readout: the card lists its
 * currencies separately. This helper never picks one of several and never
 * adds them up.
 */
export function floatingWalletBalanceText(
  item: FloatingWalletItem,
): string | null {
  if (item.balances.length !== 1) {
    return null;
  }
  return floatingWalletAmountText(item.balances[0]);
}

/**
 * Spoken label for the wallet slot: name, balance (or the unavailable
 * phrase), and status. The slot itself never prints a money figure, so this
 * is where assistive tech gets the number.
 *
 * A multi-currency wallet is spoken as one amount per currency: there is no
 * combined total to announce.
 */
export function floatingWalletLabel(item: FloatingWalletItem): string {
  const amounts = item.balances
    .map(floatingWalletAmountText)
    .filter((text): text is string => text !== null);
  const balance =
    amounts.length > 0 ? amounts.join(", ") : null;
  const parts = [
    `${item.name} API wallet`,
    balance ? `${balance} remaining` : "balance unavailable",
    item.statusLabel,
  ];
  return parts.join(", ");
}
