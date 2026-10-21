import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { ApiWallet, LimitWindow, ProviderUsage } from "../types";
import {
  DOCK_HEIGHT,
  DOCK_METER_HEIGHT,
  DOCK_WIDTH,
  FLOATING_WINDOW_HEIGHT,
  WALLET_SLOT_WIDTH,
  clampPopoverAlign,
  dockMeterRects,
  dockSeparatorLefts,
  floatingBarWidth,
  floatingClickAction,
  floatingItemLabel,
  floatingPrimaryWindow,
  floatingQuotaItems,
  floatingWalletBalanceText,
  floatingWalletItem,
  floatingWalletItems,
  floatingWalletLabel,
  floatingWindowHeight,
  haloMeterRects,
  haloWalletRect,
  primaryResetWindow,
} from "./floatingQuota";

const CHECKED_AT = "2026-09-28T10:00:00.000Z";
const SOURCE_AT = "2026-09-28T09:50:00.000Z";
const RESET_AT = "2026-09-28T18:24:00.000Z";

function limit(label: string, usedPercent: number, resetAt?: string): LimitWindow {
  return resetAt ? { label, usedPercent, resetAt } : { label, usedPercent };
}

function usage(
  id: string,
  limits: LimitWindow[],
  extra: Partial<ProviderUsage> = {},
): ProviderUsage {
  return {
    id,
    name: extra.name ?? id,
    status: "ok",
    health: "live",
    checkedAt: CHECKED_AT,
    limits,
    ...extra,
  };
}

describe("floatingQuotaItems", () => {
  it("renders production providers in registry order and omits simulated ones", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [limit("Weekly", 68)], { name: "OpenAI / Codex" }),
      usage("claude", [limit("5-hour", 88)], { simulated: true, name: "Claude" }),
      usage("zai", [limit("Weekly", 22)], { name: "Z.ai" }),
      usage("grok", [limit("30-day", 47)], { simulated: true }),
      usage("opencode-go", [limit("5-hour", 41)], { name: "OpenCode Go" }),
      usage("antigravity", [limit("Weekly", 12)], { name: "Google Antigravity" }),
    ]);

    expect(items.map((item) => item.providerId)).toEqual([
      "openai-codex",
      "zai",
      "opencode-go",
      "antigravity",
    ]);
    expect(items.map((item) => item.state)).toEqual([
      "healthy",
      "healthy",
      "healthy",
      "healthy",
    ]);
  });

  it("keeps a real usage when a simulated one shares its id", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [limit("Weekly", 90)], { simulated: true }),
      usage("openai-codex", [limit("Weekly", 68)]),
    ]);

    expect(items).toHaveLength(1);
    expect(items[0].percent).toBe(68);
  });

  it("selects the designated long-term window and ignores malformed ones", () => {
    const items = floatingQuotaItems([
      usage("zai", [
        limit("5-hour", 90, "2026-09-28T12:00:00.000Z"),
        limit("Broken", Number.NaN),
        limit("Out of range", 140),
        limit("   ", 99),
        limit("Weekly", 63.6, RESET_AT),
      ]),
    ]);

    expect(items[0]).toMatchObject({
      percent: 64,
      windowLabel: "Weekly",
      resetAt: RESET_AT,
      state: "healthy",
    });
  });

  it("does not average or sum windows", () => {
    const items = floatingQuotaItems([
      usage("opencode-go", [limit("5-hour", 20), limit("Weekly", 80)]),
    ]);

    expect(items[0].percent).toBe(80);
  });

  it("keeps the first real usage when a provider id is repeated", () => {
    const items = floatingQuotaItems([
      usage("opencode-go", [limit("Weekly", 50)]),
      usage("opencode-go", [limit("Weekly", 90)]),
    ]);

    expect(items).toHaveLength(1);
    expect(items[0].percent).toBe(50);
  });

  it("shows a provider with no usable window as unavailable", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", []),
      usage("zai", [limit("Weekly", Number.NaN), limit("", 12)]),
      usage("opencode-go", [limit("Weekly", 41)]),
    ]);

    expect(items.map((item) => item.providerId)).toEqual([
      "openai-codex",
      "zai",
      "opencode-go",
    ]);
    expect(items[0]).toMatchObject({
      state: "unavailable",
      percent: null,
      windowLabel: null,
    });
    expect(items[1].state).toBe("unavailable");
    expect(items[2].state).toBe("healthy");
  });

  it("keeps a stale provider visibly stale, including its percentage", () => {
    const items = floatingQuotaItems([
      usage("antigravity", [limit("Weekly", 8, RESET_AT)], {
        status: "stale",
        health: "stale",
        sourceUpdatedAt: SOURCE_AT,
        dataFreshness: "stale",
        name: "Google Antigravity",
      }),
    ]);

    expect(items[0]).toMatchObject({
      state: "stale",
      stale: true,
      percent: 8,
      statusLabel: "Stale",
      statusClass: "stale",
    });
    expect(items[0].state).not.toBe("healthy");
    expect(floatingItemLabel(items[0])).toContain("stale");
  });

  it("keeps an error with last-good data from looking healthy", () => {
    const items = floatingQuotaItems([
      usage(
        "opencode-go",
        [limit("5-hour", 41), limit("Weekly", 71, RESET_AT)],
        {
          status: "error",
          health: "error",
          error: "Refresh failed: HTTP 503",
          name: "OpenCode Go",
        },
      ),
    ]);

    expect(items[0]).toMatchObject({
      state: "error",
      percent: 71,
      windowLabel: "Weekly",
      error: "Refresh failed: HTTP 503",
      statusLabel: "Refresh failed",
    });
    expect(items[0].state).not.toBe("healthy");
    expect(floatingItemLabel(items[0])).toContain("error");
  });

  it("keeps a stale marker when a failed refresh retained stale data", () => {
    const items = floatingQuotaItems([
      usage("antigravity", [limit("Weekly", 12)], {
        status: "error",
        health: "error",
        error: "Refresh failed: cache unreadable",
        dataFreshness: "stale",
        sourceUpdatedAt: SOURCE_AT,
      }),
    ]);

    expect(items[0].state).toBe("error");
    expect(items[0].stale).toBe(true);
    expect(items[0].percent).toBe(12);
  });

  it("shows an error with no retained window as an error, not a healthy dash", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [], {
        status: "error",
        health: "error",
        error: "Refresh failed: not signed in",
        name: "OpenAI / Codex",
      }),
    ]);

    expect(items[0]).toMatchObject({
      state: "error",
      percent: null,
      error: "Refresh failed: not signed in",
    });
    expect(floatingItemLabel(items[0])).toContain("unavailable");
    expect(floatingItemLabel(items[0])).toContain("error");
  });

  it("surfaces cooldown and unavailable health without reading them as healthy", () => {
    // Cooldown with retained data: the number stays, the state is a failure.
    const cooldown = floatingQuotaItems([
      usage("openai-codex", [limit("Weekly", 33, RESET_AT)], {
        status: "error",
        health: "cooldown",
        errorCategory: "unexpected_response",
        errorHttpStatus: 429,
        name: "OpenAI / Codex",
      }),
    ])[0];
    expect(cooldown).toMatchObject({
      state: "error",
      percent: 33,
      statusLabel: "Cooldown",
      statusClass: "error",
      stale: false,
    });
    expect(floatingItemLabel(cooldown)).toContain("Cooldown");

    // Unavailable (no retained data): no number is invented.
    const unavailable = floatingQuotaItems([
      usage("grok", [], {
        status: "error",
        health: "unavailable",
        errorCategory: "credential_missing",
        name: "Grok (xAI)",
      }),
    ])[0];
    expect(unavailable).toMatchObject({
      state: "error",
      percent: null,
      statusLabel: "Unavailable",
    });
    expect(floatingItemLabel(unavailable)).toContain("Unavailable");
  });

  it("opens a quick view for every compact state", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [limit("Weekly", 68)]),
      usage("zai", [], { status: "unknown", health: "unknown" }),
      usage("opencode-go", [limit("Weekly", 10)], { status: "error", health: "error", error: "nope" }),
    ]);

    expect(items.map(floatingClickAction)).toEqual([
      { type: "quick-view", providerId: "openai-codex" },
      { type: "quick-view", providerId: "zai" },
      { type: "quick-view", providerId: "opencode-go" },
    ]);
  });
});

describe("primary reset and window context", () => {
  it("counts down the selected primary window, not the farthest reset", () => {
    // The floating bar's headline number is the designated long-term window,
    // so its countdown describes that same window — even when a shorter
    // window's reset lies farther in the future.
    const items = floatingQuotaItems([
      usage("openai-codex", [
        limit("5-hour", 90, "2026-09-28T14:00:00.000Z"),
        limit("Weekly", 30, "2026-10-04T18:24:00.000Z"),
        limit("30-day", 12, "2026-10-28T18:24:00.000Z"),
      ]),
    ]);

    expect(items[0]).toMatchObject({
      percent: 30,
      windowLabel: "Weekly",
      resetAt: "2026-10-04T18:24:00.000Z",
    });
    expect(items[0].primaryReset).toEqual({
      label: "Weekly",
      resetAt: "2026-10-04T18:24:00.000Z",
    });
  });

  it("never qualifies a window without a reset time, and breaks ties by provider order", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [
        limit("5-hour", 20, "2026-09-28T12:00:00.000Z"),
        limit("Weekly", 30, RESET_AT),
        limit("Monthly", 10),
      ]),
      usage("zai", [
        limit("Weekly A", 10, RESET_AT),
        limit("Weekly B", 20, RESET_AT),
      ]),
    ]);

    expect(items[0].primaryReset).toEqual({ label: "Weekly", resetAt: RESET_AT });
    expect(items[1].primaryReset).toEqual({ label: "Weekly A", resetAt: RESET_AT });
    expect(primaryResetWindow(usage("grok", [limit("Weekly", 12)]))).toBeNull();
  });

  it("features Antigravity's primary quota window, not the farthest family reset", () => {
    // Live-shaped data (2026-10): the Gemini weekly window is the most-used
    // window but resets days before the Claude weekly window.
    const items = floatingQuotaItems([
      usage(
        "antigravity",
        [
          limit("Gemini", 0, "2026-10-02T13:26:33.000Z"),
          limit("Gemini Weekly", 43.05, "2026-10-04T21:32:12.000Z"),
          limit("Claude", 0, "2026-10-02T13:26:33.000Z"),
          limit("Claude Weekly", 38.19, "2026-10-07T15:25:06.000Z"),
        ],
        { name: "Google Antigravity" },
      ),
    ]);

    expect(items[0].primaryReset).toEqual({
      label: "Gemini Weekly",
      resetAt: "2026-10-04T21:32:12.000Z",
    });
  });

  it("falls back to the longest reset when Antigravity's primary window has no reset time", () => {
    const items = floatingQuotaItems([
      usage(
        "antigravity",
        [
          limit("Gemini Weekly", 43.05),
          limit("Claude Weekly", 38.19, "2026-10-07T15:25:06.000Z"),
        ],
        { name: "Google Antigravity" },
      ),
    ]);

    expect(items[0].primaryReset).toEqual({
      label: "Claude Weekly",
      resetAt: "2026-10-07T15:25:06.000Z",
    });
  });

  it("counts down the selected primary window for every provider, not the farthest reset", () => {
    // OpenCode Go's designated 30-day window is absent, so the longest
    // declared period wins the fallback — a tie, broken by provider order —
    // and the countdown follows it rather than the farthest reset.
    const items = floatingQuotaItems([
      usage("opencode-go", [
        limit("Gemini Weekly", 43.05, "2026-10-04T21:32:12.000Z"),
        limit("Claude Weekly", 38.19, "2026-10-07T15:25:06.000Z"),
      ]),
    ]);

    expect(items[0].percent).toBe(43);
    expect(items[0].windowLabel).toBe("Gemini Weekly");
    expect(items[0].primaryReset).toEqual({
      label: "Gemini Weekly",
      resetAt: "2026-10-04T21:32:12.000Z",
    });
  });

  it("keeps the full usable window list for the expanded view and drops malformed ones", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [
        limit("5-hour", 90, "2026-09-28T14:00:00.000Z"),
        limit("Broken", Number.NaN),
        limit("Out of range", 140),
        limit("   ", 50),
        limit("Weekly", 30, RESET_AT),
      ]),
    ]);

    expect(items[0].windows).toEqual([
      { label: "5-hour", usedPercent: 90, resetAt: "2026-09-28T14:00:00.000Z" },
      { label: "Weekly", usedPercent: 30, resetAt: RESET_AT },
    ]);
  });

  it("carries the display-safe account label only when the provider proves one", () => {
    const items = floatingQuotaItems([
      usage("grok", [limit("Weekly", 12, RESET_AT)], {
        account: { label: "key ··3456" },
      }),
      usage("zai", [limit("Weekly", 22, RESET_AT)]),
    ]);

    expect(items[0].accountLabel).toBe("key ··3456");
    expect(items[1].accountLabel).toBeUndefined();
  });

  it("speaks the remaining perspective in the compact label", () => {
    const items = floatingQuotaItems([
      usage("zai", [limit("Weekly", 22, RESET_AT)], { name: "Z.ai" }),
    ]);

    expect(floatingItemLabel(items[0])).toContain("78% remaining");
    expect(floatingItemLabel(items[0])).toContain("Weekly");
  });
});

describe("floating primary quota selection", () => {
  it("designates Codex's 7-day window even when the 5-hour window is more depleted", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [limit("5-hour", 95, RESET_AT), limit("Weekly", 30, RESET_AT)], {
        name: "OpenAI / Codex",
      }),
    ]);

    expect(items[0]).toMatchObject({ percent: 30, windowLabel: "Weekly" });
    expect(floatingItemLabel(items[0])).toContain("70% remaining");
  });

  it("designates Grok's weekly credits over the on-demand balance", () => {
    const items = floatingQuotaItems([
      usage("grok", [limit("Weekly credits", 25, RESET_AT), limit("On-demand", 90)], {
        name: "Grok (xAI)",
      }),
    ]);

    expect(items[0]).toMatchObject({ percent: 25, windowLabel: "Weekly credits" });
  });

  it("designates OpenCode Go's 30-day window over both shorter windows", () => {
    const items = floatingQuotaItems([
      usage("opencode-go", [
        limit("5-hour", 95, RESET_AT),
        limit("Weekly", 90, RESET_AT),
        limit("30-day", 40, RESET_AT),
      ]),
    ]);

    expect(items[0]).toMatchObject({ percent: 40, windowLabel: "30-day" });
  });

  it("designates Z.ai's weekly pool over the 5-hour pool", () => {
    const items = floatingQuotaItems([
      usage("zai", [limit("5-hour", 80, RESET_AT), limit("Weekly", 25, RESET_AT)], {
        name: "Z.ai",
      }),
    ]);

    expect(items[0]).toMatchObject({ percent: 25, windowLabel: "Weekly" });
  });

  it("designates the Qwen Token Plan monthly window", () => {
    const items = floatingQuotaItems([
      usage(
        "qwen-token-plan",
        [limit("Monthly", 43.78, "2026-11-16T16:00:00.000Z")],
        { name: "Qwen Token Plan" },
      ),
    ]);

    expect(items[0]).toMatchObject({
      percent: 44,
      usedPercent: 43.78,
      windowLabel: "Monthly",
    });
  });

  it("keeps Antigravity's families separate: Gemini Weekly over a depleted Claude Weekly", () => {
    // The task's example shape: the Claude family is nearly exhausted while
    // the Gemini weekly pool still has 70% left. The collapsed segment speaks
    // for Gemini Weekly — never the max across families.
    const items = floatingQuotaItems([
      usage(
        "antigravity",
        [
          limit("Gemini", 0, "2026-10-02T13:26:33.000Z"),
          limit("Gemini Weekly", 30, "2026-10-04T21:32:12.000Z"),
          limit("Claude", 0, "2026-10-02T13:26:33.000Z"),
          limit("Claude Weekly", 99, "2026-10-07T15:25:06.000Z"),
        ],
        { name: "Google Antigravity" },
      ),
    ]);

    expect(items[0]).toMatchObject({
      percent: 30,
      usedPercent: 30,
      windowLabel: "Gemini Weekly",
      resetAt: "2026-10-04T21:32:12.000Z",
    });
    // Percentage, identity, and primary countdown all describe one window.
    expect(items[0].primaryReset).toEqual({
      label: "Gemini Weekly",
      resetAt: "2026-10-04T21:32:12.000Z",
    });
    expect(floatingItemLabel(items[0])).toContain("70% remaining");
    expect(floatingItemLabel(items[0])).toContain("Gemini Weekly");
  });

  it("keeps a weekly window weekly near its reset instead of re-reading it by reset distance", () => {
    // The weekly window resets in 30 minutes — sooner than the 5-hour
    // window's reset — but selection follows the declared period, never the
    // time remaining, so the primary cannot flip to the shorter window.
    const items = floatingQuotaItems([
      usage("openai-codex", [
        limit("5-hour", 20, "2026-09-28T14:00:00.000Z"),
        limit("Weekly", 30, "2026-09-28T10:30:00.000Z"),
      ]),
    ]);

    expect(items[0]).toMatchObject({
      percent: 30,
      windowLabel: "Weekly",
      resetAt: "2026-09-28T10:30:00.000Z",
    });
    expect(items[0].primaryReset).toEqual({
      label: "Weekly",
      resetAt: "2026-09-28T10:30:00.000Z",
    });
  });

  it("falls back to the longest declared period when the preferred window is malformed", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [
        limit("Weekly", Number.NaN),
        limit("5-hour", 90, RESET_AT),
        limit("Daily", 20),
        limit("3-day", 10),
      ]),
    ]);

    // The usable window with the longest declared period — 3-day, not the
    // most depleted 5-hour window.
    expect(items[0]).toMatchObject({ percent: 10, windowLabel: "3-day" });
  });

  it("falls back deterministically when Antigravity's Gemini Weekly is absent", () => {
    // No Gemini weekly window: the usable window with the longest declared
    // period is the Claude family's weekly — a predictable stand-in, still
    // never an average across families.
    const items = floatingQuotaItems([
      usage("antigravity", [
        limit("Gemini", 0, RESET_AT),
        limit("Claude", 0, RESET_AT),
        limit("Claude Weekly", 99, RESET_AT),
      ]),
    ]);

    expect(items[0]).toMatchObject({ percent: 99, windowLabel: "Claude Weekly" });
    expect(floatingItemLabel(items[0])).toContain("1% remaining");
  });

  it("breaks fallback ties by provider order", () => {
    // Neither label matches the designated "Weekly" exactly, so both fall to
    // the longest-period rule, tie, and the provider's own order decides.
    expect(
      floatingPrimaryWindow(
        usage("zai", [limit("Weekly A", 10), limit("Weekly B", 90)]),
      )?.label,
    ).toBe("Weekly A");
  });

  it("matches the designated label case-insensitively after trimming", () => {
    expect(
      floatingPrimaryWindow(usage("zai", [limit("5-hour", 90), limit("  weekly  ", 50)]))
        ?.label,
    ).toBe("  weekly  ");
  });

  it("counts down the longest reset when the selected primary has no reset time", () => {
    const items = floatingQuotaItems([
      usage("openai-codex", [
        limit("Weekly", 30),
        limit("5-hour", 90, RESET_AT),
      ]),
    ]);

    // The headline stays on the designated window; only the countdown falls
    // back, because a countdown needs a reset time to count down to.
    expect(items[0]).toMatchObject({
      percent: 30,
      windowLabel: "Weekly",
      resetAt: null,
    });
    expect(items[0].primaryReset).toEqual({ label: "5-hour", resetAt: RESET_AT });
  });

  it("uses the longest-period rule for providers without a designated window", () => {
    const items = floatingQuotaItems([
      usage("future-provider", [limit("5-hour", 90, RESET_AT), limit("Weekly", 15)]),
    ]);

    expect(items[0]).toMatchObject({ percent: 15, windowLabel: "Weekly" });
  });

  it("selects nothing when no window is usable", () => {
    expect(floatingPrimaryWindow(usage("openai-codex", []))).toBeUndefined();
    expect(
      floatingPrimaryWindow(
        usage("openai-codex", [limit("Weekly", Number.NaN), limit("", 12)]),
      ),
    ).toBeUndefined();
  });
});

describe("floating bar geometry", () => {
  it("fits four providers at 520px and caps five at 600px", () => {
    expect(floatingBarWidth(4)).toBe(520);
    expect(floatingBarWidth(5)).toBe(600);
    expect(floatingBarWidth(1)).toBeGreaterThanOrEqual(300);
  });

  it("collapses to a 64px base; the detail card reserves the measured card plus its frame", () => {
    expect(FLOATING_WINDOW_HEIGHT).toBe(64);
    expect(floatingWindowHeight("none")).toBe(64);
    expect(floatingWindowHeight("menu")).toBe(232);
    // Fallback before the first measurement (and in layout-less tests).
    expect(floatingWindowHeight("detail")).toBe(356);
    expect(floatingWindowHeight("detail", 0)).toBe(356);
    // Measured: exact card height + 12px frame, so no card can overflow the
    // window however many quota windows the provider reports.
    expect(floatingWindowHeight("detail", 276)).toBe(352);
    expect(floatingWindowHeight("detail", 320)).toBe(396);
    expect(floatingWindowHeight("detail", 340.6)).toBe(417);
  });

  it("anchors the detail card under its segment and clamps at both bar edges", () => {
    const shell = 520;
    expect(clampPopoverAlign(0, shell, 300)).toBe(0);
    expect(clampPopoverAlign(140, shell, 300)).toBe(140);
    expect(clampPopoverAlign(400, shell, 300)).toBe(220);
    expect(clampPopoverAlign(-5, shell, 300)).toBe(0);
    expect(clampPopoverAlign(400, 0, 300)).toBe(0);
  });

  it("seeds the native floating window with the collapsed geometry", () => {
    const config = JSON.parse(
      readFileSync(
        new URL("../../src-tauri/tauri.conf.json", import.meta.url),
        "utf8",
      ),
    ) as { app: { windows: Array<Record<string, unknown>> } };
    const floating = config.app.windows.find(
      (window) => window.label === "floating-quota",
    );
    expect(floating).toBeDefined();
    expect(floating!.width).toBe(520);
    expect(floating!.height).toBe(64);
    expect(floating!.minHeight).toBe(64);
    expect(floating!.maxHeight).toBe(640);
    expect(floating!.transparent).toBe(true);
    // The webview applies the persisted pin before the bar is ever shown;
    // seeding the native window with false keeps an unpinned bar from
    // flashing always-on-top.
    expect(floating!.alwaysOnTop).toBe(false);
  });
});

describe("resting dock geometry", () => {
  it("rests at a fixed 600×16 logical strip with 3px meters", () => {
    expect(DOCK_WIDTH).toBe(600);
    expect(DOCK_HEIGHT).toBe(16);
    expect(DOCK_METER_HEIGHT).toBe(3);
  });

  it("places the five resting meters exactly on the revealed Halo meters", () => {
    // The full production registry reveals at 600 wide, so the dock's offset
    // is zero and the rects must match the bar's own layout math exactly.
    expect(floatingBarWidth(5)).toBe(600);
    const revealed = haloMeterRects(600, 5);
    expect(revealed).toEqual([
      { left: 63, width: 62 },
      { left: 177, width: 62 },
      { left: 291, width: 62 },
      { left: 405, width: 62 },
      { left: 519, width: 62 },
    ]);
    expect(dockMeterRects(5)).toEqual(revealed);
  });

  it("keeps docked meters stationary on screen when fewer providers reveal narrower", () => {
    // Two visible providers reveal at 300 wide; the reveal re-centers the
    // window, so each strip rect is the revealed rect shifted by half the
    // width difference — every meter fades into its own revealed position.
    expect(floatingBarWidth(2)).toBe(300);
    const offset = (DOCK_WIDTH - 300) / 2;
    expect(dockMeterRects(2)).toEqual(
      haloMeterRects(300, 2).map((rect) => ({
        left: offset + rect.left,
        width: rect.width,
      })),
    );
  });

  it("lays the meter band out with real bar proportions on every production width", () => {
    for (const count of [1, 2, 3, 4, 5]) {
      const width = floatingBarWidth(count);
      const rects = haloMeterRects(width, count);
      // Every meter stays inside its segment and inside the window.
      for (const rect of rects) {
        expect(rect.left).toBeGreaterThan(0);
        expect(rect.left + rect.width).toBeLessThan(width);
        expect(rect.width).toBeGreaterThan(0);
      }
      // Monotonic, evenly stepped segments — the bar's flex rhythm.
      for (let index = 1; index < rects.length; index += 1) {
        expect(rects[index].left).toBeGreaterThan(rects[index - 1].left);
        expect(rects[index].width).toBeCloseTo(rects[0].width, 6);
      }
    }
  });

  it("puts the separators on the revealed bar's segment boundaries", () => {
    // One separator per meter gap, each at the item boundary the revealed
    // bar draws its own divider at (63 − 40 → 23 first item, etc.).
    expect(dockSeparatorLefts(5)).toEqual([137, 251, 365, 479]);
    expect(dockSeparatorLefts(1)).toEqual([]);
  });
});

describe("API wallet geometry", () => {
  it("adds one fixed 44px slot and one shell gap to the revealed width", () => {
    expect(WALLET_SLOT_WIDTH).toBe(44);
    expect(floatingBarWidth(5)).toBe(600);
    expect(floatingBarWidth(5, true)).toBe(646);
    expect(floatingBarWidth(4, true)).toBe(566);
  });

  it("keeps every provider meter exactly where it was at 600px", () => {
    const expected = [
      { left: 63, width: 62 },
      { left: 177, width: 62 },
      { left: 291, width: 62 },
      { left: 405, width: 62 },
      { left: 519, width: 62 },
    ];
    expect(haloMeterRects(600, 5)).toEqual(expected);
    expect(haloMeterRects(646, 5, true)).toEqual(expected);
    expect(haloWalletRect(646, 5)).toEqual({ left: 593, width: 44 });
  });

  it("keeps the resting 600px dock meters on the same revealed positions", () => {
    // The 646px reveal re-centers over the fixed 600px strip, so each meter
    // moves 23px left on screen but lands on the same bar segment.
    expect(dockMeterRects(5, true)).toEqual([
      { left: 40, width: 62 },
      { left: 154, width: 62 },
      { left: 268, width: 62 },
      { left: 382, width: 62 },
      { left: 496, width: 62 },
    ]);
    expect(dockSeparatorLefts(5, true)).toEqual([114, 228, 342, 456]);
    // The strip itself stays meters-only and 600×16 while the wallet shows
    // only in the revealed bar.
    expect(DOCK_WIDTH).toBe(600);
    expect(DOCK_HEIGHT).toBe(16);
  });
});

function wallet(extra: Partial<ApiWallet> = {}): ApiWallet {
  return {
    id: "openrouter",
    name: "OpenRouter",
    health: "live",
    checkedAt: CHECKED_AT,
    remainingUsd: 12.5,
    ...extra,
  };
}

describe("floating wallet presentation", () => {
  it("maps a live reading to a healthy wallet with two decimals", () => {
    const item = floatingWalletItem(wallet());
    expect(item).toMatchObject({
      id: "openrouter",
      name: "OpenRouter",
      state: "healthy",
      remainingUsd: 12.5,
      checkedAt: CHECKED_AT,
      statusLabel: "Live",
    });
    expect(floatingWalletBalanceText(item)).toBe("$12.50");
    expect(floatingWalletLabel(item)).toBe(
      "OpenRouter API wallet, $12.50 remaining, Live",
    );
  });

  it("keeps a real zero balance as $0.00", () => {
    const item = floatingWalletItem(wallet({ remainingUsd: 0 }));
    expect(item.state).toBe("healthy");
    expect(item.remainingUsd).toBe(0);
    expect(floatingWalletBalanceText(item)).toBe("$0.00");
  });

  it.each([undefined, -0.01, Number.NaN, Number.POSITIVE_INFINITY])(
    "never turns an untrustworthy balance (%s) into money",
    (remainingUsd) => {
      const item = floatingWalletItem(wallet({ remainingUsd }));
      expect(item.state).toBe("unavailable");
      expect(item.remainingUsd).toBeNull();
      expect(floatingWalletBalanceText(item)).toBeNull();
      expect(floatingWalletLabel(item)).toBe(
        "OpenRouter API wallet, balance unavailable, Unavailable",
      );
      expect(floatingWalletLabel(item)).not.toContain("$0.00");
    },
  );

  it("keeps every non-live health as its own slot state", () => {
    expect(floatingWalletItem(wallet({ health: "stale" })).statusLabel).toBe(
      "Stale",
    );
    expect(
      floatingWalletItem(wallet({ health: "cooldown" })).statusLabel,
    ).toBe("Cooling down");
    expect(
      floatingWalletItem(
        wallet({ health: "error", errorHttpStatus: 429 }),
      ).statusLabel,
    ).toBe("Rate limited");
    expect(
      floatingWalletItem(wallet({ health: "error" })).statusLabel,
    ).toBe("Refresh failed");
    expect(
      floatingWalletItem(wallet({ health: "unknown" })).state,
    ).toBe("unavailable");
  });

  it("drops an invalid stamp instead of claiming a fresh check", () => {
    const item = floatingWalletItem(wallet({ checkedAt: "not-a-date" }));
    expect(item.checkedAt).toBeNull();
    expect(item.state).toBe("healthy");
  });

  it("carries identity, error details and one item per non-empty id", () => {
    const items = floatingWalletItems([
      wallet({ identity: "key:1234" }),
      wallet({ name: "duplicate" }),
      wallet({ id: "  " }),
      wallet({ id: "openrouter-2", error: "refresh failed", errorCategory: "http" }),
    ]);
    expect(items.map((item) => item.id)).toEqual([
      "openrouter",
      "openrouter-2",
    ]);
    expect(items[0].identity).toBe("key:1234");
    expect(items[1].error).toBe("refresh failed");
    expect(items[1].errorCategory).toBe("http");
  });
});
