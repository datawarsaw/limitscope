// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { GrokBotSection } from "./GrokBotSection";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = new Date("2026-10-08T16:00:00.000Z");

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  invoke.mockReset();
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

async function renderSection() {
  await act(async () => {
    root.render(<GrokBotSection now={NOW} />);
  });
}

async function clickRefresh() {
  await act(async () => {
    container
      .querySelector<HTMLButtonElement>(".fq-grokbot-refresh")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    // Drain the invoke → setState chain while still inside act.
    await Promise.resolve();
  });
}

const OK_RESULT = {
  status: "ok",
  observedAt: "2026-10-08T15:30:00Z",
  usedPercent: 73,
  resetText: "Resets in 3 days",
  appVersion: "0.68.1.0",
};

describe("GrokBotSection", () => {
  it("shows the section header and Refresh action, with no data yet", async () => {
    await renderSection();
    expect(container.querySelector(".fq-grokbot")).not.toBeNull();
    expect(container.querySelector(".fq-grokbot-label")!.textContent).toBe("Grok Bot");
    expect(container.querySelector(".fq-grokbot-refresh")!.textContent).toBe("Refresh");
    expect(container.textContent).toContain("No Grok Bot data yet");
  });

  it("never invokes the command on its own — refresh is the only trigger", async () => {
    await renderSection();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("renders the derived remaining quota, verbatim reset text, and stamp", async () => {
    invoke.mockResolvedValueOnce(OK_RESULT);
    await renderSection();
    await clickRefresh();
    expect(invoke).toHaveBeenCalledWith("refresh_grok_bot_usage");

    const line = container.querySelector(".fq-grokbot-line")!;
    // The whole value line, verbatim: derived remaining quota plus the
    // source's own countdown text — nothing re-derived into a timestamp.
    expect(line.textContent).toBe("27% remaining · Resets in 3 days");

    const meter = container.querySelector(".fq-grokbot .fq-meter-row")!;
    expect(meter).not.toBeNull();
    expect(meter.getAttribute("aria-label")).toContain("Grok Bot weekly quota");
    expect(meter.querySelector(".fq-meter-fill")!.getAttribute("style")).toContain("width: 27%");

    const note = container.querySelector(".fq-grokbot-note")!;
    expect(note.textContent).toContain("Updated");
    expect(note.textContent).toContain("Grok Bot 0.68.1.0");
    expect(note.className).not.toContain("is-stale");
  });

  it("keeps the last successful reading, marked stale, when a refresh fails", async () => {
    invoke.mockResolvedValueOnce(OK_RESULT);
    await renderSection();
    await clickRefresh();

    invoke.mockResolvedValueOnce({
      status: "screen_not_visible",
      observedAt: "2026-10-08T16:00:00Z",
      lastKnown: {
        observedAt: "2026-10-08T15:30:00Z",
        usedPercent: 73,
        resetText: "Resets in 3 days",
        appVersion: "0.68.1.0",
      },
    });
    await clickRefresh();

    const line = container.querySelector(".fq-grokbot-line")!;
    expect(line.textContent).toContain("27% remaining");
    expect(line.textContent).toContain("Resets in 3 days");
    const note = container.querySelector(".fq-grokbot-note")!;
    expect(note.textContent).toContain("Last known");
    expect(note.textContent).toContain("30m ago");
    expect(note.className).toContain("is-stale");
    expect(container.querySelector(".fq-grokbot-status")!.textContent).toContain(
      "Usage & Billing",
    );
  });

  it("shows the not_running state without inventing a quota", async () => {
    invoke.mockResolvedValueOnce({
      status: "not_running",
      observedAt: "2026-10-08T16:00:00Z",
    });
    await renderSection();
    await clickRefresh();

    expect(container.querySelector(".fq-grokbot-status")!.textContent).toContain(
      "isn't running",
    );
    expect(container.querySelector(".fq-grokbot-line")).toBeNull();
    expect(container.querySelector(".fq-grokbot .fq-meter-row")).toBeNull();
    expect(container.textContent).not.toContain("0% remaining");
    expect(container.textContent).not.toContain("100% remaining");
  });

  it("shows the unknown state without values when the read cannot happen", async () => {
    invoke.mockRejectedValueOnce(new Error("no tauri internals"));
    await renderSection();
    await clickRefresh();

    expect(container.querySelector(".fq-grokbot-status")!.textContent).toContain(
      "Couldn't read",
    );
    expect(container.querySelector(".fq-grokbot-line")).toBeNull();
    expect(container.querySelector(".fq-grokbot .fq-meter-row")).toBeNull();
  });
});
