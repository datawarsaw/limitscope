// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ClaudeRefreshControl,
  claudeRefreshMessage,
} from "./ClaudeRefreshControl";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

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
function button(): HTMLButtonElement {
  return container.querySelector("button") as HTMLButtonElement;
}

describe("Claude Code on-demand ping", () => {
  it("states the request cost and that nothing runs in the background", async () => {
    invoke.mockResolvedValue({ status: "launched" });
    await render(<ClaudeRefreshControl />);
    expect(button().textContent).toBe("Ping Claude (uses 1 request)");
    expect(container.textContent).toContain("sends one tiny request");
    expect(container.textContent).toContain("Nothing runs in the background");
    // Nothing is invoked before the user acts.
    expect(invoke).not.toHaveBeenCalled();
  });

  it("invokes the command exactly once and reports the attempt, not a measurement", async () => {
    invoke.mockResolvedValue({ status: "launched" });
    await render(<ClaudeRefreshControl />);
    await act(async () => button().click());
    expect(invoke.mock.calls).toEqual([["refresh_claude_usage"]]);
    const result = container.querySelector('[role="status"]')?.textContent ?? "";
    expect(result).toContain("Claude launched; quota pending");
    // The attempt never claims a fresh or updated quota reading.
    expect(result).not.toMatch(/fresh/i);
    expect(result).not.toMatch(/updated/i);
    expect(container.textContent ?? "").not.toMatch(/fresh/i);
  });

  it("reports every non-success attempt honestly", async () => {
    for (const status of [
      "notFound",
      "spawnFailed",
      "scratchUnavailable",
      "alreadyRunning",
    ] as const) {
      invoke.mockResolvedValue({ status });
      await render(<ClaudeRefreshControl />);
      await act(async () => button().click());
      const result = container.querySelector('[role="status"]')?.textContent ?? "";
      expect(result).toBe(claudeRefreshMessage(status));
      expect(result).not.toMatch(/quota pending/);
    }
    expect(claudeRefreshMessage("notFound")).toContain("Claude CLI not found");
    expect(claudeRefreshMessage("spawnFailed")).toContain("could not be started");
    expect(claudeRefreshMessage("scratchUnavailable")).toContain(
      "working directory",
    );
    // The refusal of a second paid request says so, and claims no quota.
    expect(claudeRefreshMessage("alreadyRunning")).toContain(
      "already running",
    );
    expect(claudeRefreshMessage("alreadyRunning")).toContain(
      "no second request was sent",
    );
  });

  it("keeps a second click from starting a second Claude session", async () => {
    invoke.mockImplementation(() => new Promise(() => {}));
    await render(<ClaudeRefreshControl />);
    await act(async () => {
      button().click();
      button().click();
    });
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(button().getAttribute("aria-busy")).toBe("true");
  });

  it("fails closed when the command cannot be reached", async () => {
    invoke.mockRejectedValue(new Error("denied"));
    await render(<ClaudeRefreshControl />);
    await act(async () => button().click());
    const alert = container.querySelector('[role="alert"]')?.textContent ?? "";
    expect(alert).toContain("Couldn't reach the Claude CLI");
    expect(container.querySelector('[role="status"]')).toBeNull();
  });
});
