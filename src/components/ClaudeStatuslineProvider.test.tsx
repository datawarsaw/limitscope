// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrimaryQuotaPanel } from "./dashboard/PrimaryQuotaPanel";
import {
  chainedCommand,
  chainedSnippet,
  ClaudeStatuslineSetup,
} from "./ClaudeStatuslineSetup";
import { floatingQuotaItems } from "../lib/floatingQuota";
import { providerSourceNote, providerStatusPresentation } from "../lib/v03Integration";
import { shortProviderName } from "../lib/dashboard";
import type { ProviderUsage } from "../types";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const now = new Date("2026-11-17T12:00:00Z");
const live: ProviderUsage = {
  id: "claude-code",
  name: "Claude Code",
  health: "live",
  status: "ok",
  checkedAt: "2026-11-17T11:58:00.000Z",
  sourceUpdatedAt: "2026-11-17T11:58:00.000Z",
  dataFreshness: "fresh",
  limits: [
    { label: "5-hour", usedPercent: 23.5, resetAt: "2026-11-17T15:00:00.000Z" },
    { label: "7-day", usedPercent: 41.2, resetAt: "2026-11-20T00:00:00.000Z" },
  ],
};
const noData: ProviderUsage = {
  id: "claude-code",
  name: "Claude Code",
  health: "unavailable",
  status: "error",
  checkedAt: "2026-11-17T12:00:00.000Z",
  limits: [],
  error: "No Claude Code status line data has been received yet.",
  errorCategory: "statusline_no_data",
};

const setupInfo = {
  command: '"C:/Program Files/LimitScope/LimitScope.exe" --claude-statusline-ingest',
  powershellCommand: '& "C:/Program Files/LimitScope/LimitScope.exe" --claude-statusline-ingest',
  settingsSnippet: '{\n  "statusLine": {\n    "type": "command"\n  }\n}',
};

const container = document.createElement("div");
document.body.append(container);
let root = createRoot(container);
afterEach(async () => {
  await act(async () => root.unmount());
  root = createRoot(container);
  invoke.mockReset();
});
async function render(element: React.ReactNode) {
  await act(async () => root.render(element));
}

describe("Claude Code provider presentation", () => {
  it("uses a short name and features the 7-day window on the floating deck", () => {
    expect(shortProviderName(live)).toBe("Claude");
    const [item] = floatingQuotaItems([live]);
    expect(item.windowLabel).toBe("7-day");
    expect(item.usedPercent).toBe(41.2);
    expect(item.windows.map((w) => [w.label, w.usedPercent])).toEqual([
      ["5-hour", 23.5],
      ["7-day", 41.2],
    ]);
  });

  it("states provenance and the passive limitation, never a live-poll claim", () => {
    const note = providerSourceNote(live, now);
    expect(note).toContain("Received from Claude Code");
    expect(note).toContain("updates only while Claude Code is running");
    expect(providerSourceNote(noData, now)).toBe("Waiting for Claude Code status line data");
    expect(providerStatusPresentation(noData).label).toBe("Unavailable");
  });

  it("shows idle data as stale, not live", () => {
    const stale: ProviderUsage = { ...live, health: "stale", status: "stale", dataFreshness: "stale" };
    expect(providerStatusPresentation(stale).label).toBe("Stale");
  });

  it("a missing provider reports no quota windows and never a 0% reading", async () => {
    invoke.mockResolvedValue(setupInfo);
    expect(floatingQuotaItems([noData])[0].usedPercent).toBeNull();
    await render(
      <PrimaryQuotaPanel usage={noData} now={now} predictionFor={() => undefined} perspective="used" />,
    );
    expect(container.textContent).not.toMatch(/\b0%/);
    expect(container.textContent).toContain("No Claude Code status line data has been received yet.");
    expect(container.textContent).toContain("Passive source");
  });
});

describe("Claude Code opt-in setup", () => {
  it("reads the command once, displays it, and writes nothing", async () => {
    invoke.mockResolvedValue(setupInfo);
    await render(<ClaudeStatuslineSetup />);
    expect(invoke.mock.calls).toEqual([["get_claude_statusline_setup"]]);
    const text = container.textContent ?? "";
    expect(text).toContain("LimitScope never edits that file");
    expect(text).toContain("Already have a statusLine? Don't replace it");
    expect(text).toContain("YOUR_EXISTING_COMMAND");
    expect(container.querySelector('pre[aria-label="Claude Code statusLine settings"]')?.textContent).toBe(
      setupInfo.settingsSnippet,
    );
    // The PowerShell spelling differs, so it is offered separately.
    expect(text).toContain(setupInfo.powershellCommand);
  });

  it("offers a manual path, not a silent install, when the path cannot be quoted", async () => {
    invoke.mockResolvedValue({ command: null, powershellCommand: null, settingsSnippet: null });
    await render(<ClaudeStatuslineSetup />);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("can't be quoted safely");
    expect(container.querySelector("pre")).toBeNull();
  });

  it("fails closed when the setup command cannot be read", async () => {
    invoke.mockRejectedValue(new Error("denied"));
    await render(<ClaudeStatuslineSetup />);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("Nothing was changed");
  });

  it("builds a correctly escaped chained settings entry that keeps the existing command", () => {
    const chained = chainedCommand("C:/x/LimitScope.exe --claude-statusline-ingest");
    expect(chained).toBe(
      `input=$(cat); printf '%s' "$input" | C:/x/LimitScope.exe --claude-statusline-ingest; printf '%s' "$input" | YOUR_EXISTING_COMMAND`,
    );
    const parsed = JSON.parse(chainedSnippet("C:/x/LimitScope.exe --claude-statusline-ingest"));
    expect(parsed.statusLine.type).toBe("command");
    expect(parsed.statusLine.command).toBe(chained);
  });
});
