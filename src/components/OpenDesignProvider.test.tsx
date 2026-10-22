// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrimaryQuotaPanel } from "./dashboard/PrimaryQuotaPanel";
import { QuotaWindowList } from "./dashboard/QuotaWindowList";
import { OpenDesignRefreshControl } from "./OpenDesignRefreshControl";
import { FloatingProviderPopover } from "./FloatingProviderPopover";
import { dockMeterRects, dockSeparatorLefts, haloMeterRects, floatingQuotaItems } from "../lib/floatingQuota";
import { attentionItems } from "../lib/dashboard";
import { orderVisibleProviders } from "../lib/providerPreferences";
import type { ProviderUsage } from "../types";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
const now = new Date("2026-10-17T12:00:00Z");
const usage: ProviderUsage = {
  id: "opendesign", name: "OpenDesign", health: "live", status: "ok",
  checkedAt: "2026-10-17T11:59:00Z", sourceUpdatedAt: "2026-10-17T11:59:00Z",
  dataFreshness: "fresh",
  limits: [
    { label: "5-hour", usedPercent: 73, resetText: "Resets in 39 min" },
    { label: "7-day", usedPercent: 9, resetText: "Resets in 6 days 14 hr" },
  ],
};
const container = document.createElement("div");
document.body.append(container);
let root = createRoot(container);
afterEach(async () => { await act(async () => root.unmount()); root = createRoot(container); invoke.mockReset(); });
async function render(element: React.ReactNode) { await act(async () => root.render(element)); }

describe("first-class OpenDesign presentation", () => {
  it("appends to existing saved provider order without hiding it", () => {
    const codex = { ...usage, id: "openai-codex" };
    expect(orderVisibleProviders([codex, usage], p => p.id, ["openai-codex", "opendesign"], { order: ["openai-codex"], hidden: [] }).map(p => p.id))
      .toEqual(["openai-codex", "opendesign"]);
  });
  it("features 7-day independently of the depleted 5-hour window", () => {
    const [item] = floatingQuotaItems([usage]);
    expect(item.windowLabel).toBe("7-day");
    expect(item.usedPercent).toBe(9);
    expect(item.resetAt).toBeNull();
    expect(item.primaryReset).toBeNull();
    expect(item.resetText).toBe("Resets in 6 days 14 hr");
    expect(item.windows.map(w => [w.label, w.usedPercent, w.resetText])).toEqual([
      ["5-hour", 73, "Resets in 39 min"], ["7-day", 9, "Resets in 6 days 14 hr"],
    ]);
  });
  it("keeps seven-provider compact dock meters aligned and inside the bar", () => {
    const meters = haloMeterRects(600, 7);
    expect(dockMeterRects(7)).toEqual(meters);
    expect(meters).toHaveLength(7);
    expect(meters[0].left).toBe(51);
    expect(meters.every(m => m.width > 40 && m.left + m.width < 600)).toBe(true);
    expect(dockSeparatorLefts(7)).toEqual(meters.slice(1).map(m => m.left - 28));
  });
  it("keeps attention messages in remaining perspective for both warning thresholds", () => {
    for (const usedPercent of [90, 97]) {
      const [attention] = attentionItems([{ ...usage, limits: [{ label: "5-hour", usedPercent }] }], () => undefined, "used");
      expect(attention.line).toContain(`${100 - usedPercent}% remaining`);
      expect(attention.line).not.toContain("% used");
    }
  });
  it("renders remaining and original relative resets on the dashboard", async () => {
    await render(<><PrimaryQuotaPanel usage={usage} now={now} predictionFor={() => undefined} perspective="used" /><QuotaWindowList usage={usage} now={now} predictionFor={() => undefined} perspective="used" /></>);
    expect(container.textContent).toContain("27% remaining");
    expect(container.textContent).toContain("91% remaining");
    expect(container.textContent).toContain("At observation: Resets in 39 min");
    expect(container.textContent).toContain("At observation: Resets in 6 days 14 hr");
    expect(invoke).not.toHaveBeenCalled();
  });
  it("renders the same long-term window and both reset strings in the floating detail", async () => {
    await render(<FloatingProviderPopover item={floatingQuotaItems([usage])[0]} pinned={false} now={now} note={null} align={0} exiting={false} onMouseEnter={() => {}} onMouseLeave={() => {}} onFocus={() => {}} onBlur={() => {}} />);
    expect(container.querySelector(".fq-value-num")?.textContent).toBe("91%");
    expect(container.querySelector(".fq-primary-reset")?.textContent).toContain("7-day · At observation: Resets in 6 days 14 hr");
    expect(container.textContent).toContain("At observation: Resets in 39 min");
    expect(container.textContent).toContain("Manual refresh only");
  });
  it("retains cached values with an error and stale marker; absent data has no percent", () => {
    const [cached] = floatingQuotaItems([{ ...usage, health: "error", status: "error", dataFreshness: "stale", error: "Chrome isn't running." }]);
    expect(cached.usedPercent).toBe(9);
    expect(cached.stale).toBe(true);
    expect(cached.state).toBe("error");
    const [empty] = floatingQuotaItems([{ ...usage, health: "unavailable", limits: [] }]);
    expect(empty.percent).toBeNull();
    const [fresh] = floatingQuotaItems([usage]);
    expect(fresh.statusLabel).toBe("Cached");
    expect(fresh.stale).toBe(false);
  });
  it("keeps an unavailable dashboard empty and does not invent a plan", async () => {
    await render(<PrimaryQuotaPanel usage={{ ...usage, health: "unavailable", limits: [], error: "Open Plan & balance in Chrome." }} now={now} predictionFor={() => undefined} perspective="used" />);
    expect(container.querySelector('[role="progressbar"]')).toBeNull();
    expect(container.textContent).not.toContain("0%");
    expect(container.textContent).not.toContain("OpenDesign Go");
    expect(invoke).not.toHaveBeenCalled();
  });
  it("leaves existing provider selection and absolute reset behavior intact", () => {
    const [codex] = floatingQuotaItems([{ ...usage, id: "openai-codex", limits: [
      { label: "5-hour", usedPercent: 90 }, { label: "Weekly", usedPercent: 20, resetAt: "2026-10-20T12:00:00Z" },
    ] }]);
    expect(codex.windowLabel).toBe("Weekly");
    expect(codex.primaryReset?.resetAt).toBe("2026-10-20T12:00:00Z");
    expect(codex.resetText).toBeUndefined();
  });
  it("coalesces repeated clicks and keeps the explicit control focused", async () => {
    let resolve!: () => void;
    invoke.mockReturnValue(new Promise<void>(r => { resolve = r; }));
    await render(<OpenDesignRefreshControl />);
    const button = container.querySelector("button")!;
    button.focus();
    await act(async () => { button.click(); button.click(); });
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("refresh_opendesign_usage");
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect(document.activeElement).toBe(button);
    await act(async () => resolve());
    expect(button.textContent).toBe("Refresh");
  });
  it("announces a transport error without constructing a replacement observation", async () => {
    invoke.mockRejectedValue(new Error("IPC unavailable"));
    await render(<OpenDesignRefreshControl />);
    await act(async () => container.querySelector("button")!.click());
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("last observation is unchanged");
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});
