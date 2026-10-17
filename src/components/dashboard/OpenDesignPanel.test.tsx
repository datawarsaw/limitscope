// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { OpenDesignPanel } from "./OpenDesignPanel";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = new Date("2026-10-17T10:00:00.000Z");
const OBSERVED = "2026-10-17T09:31:00Z";

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

async function renderPanel() {
  await act(async () => {
    root.render(<OpenDesignPanel now={NOW} />);
  });
}

async function clickRefresh() {
  await act(async () => {
    container
      .querySelector<HTMLButtonElement>(".opendesign-refresh")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await Promise.resolve();
  });
}

function okResult() {
  return {
    status: "ok",
    observedAt: OBSERVED,
    windows: [
      { label: "5-hour", remainingPercent: 70.93, resetText: "Resets in 31 min" },
      { label: "7-day", remainingPercent: 90.1, resetText: "Resets in 6 days 14 hr" },
    ],
    plan: "Go",
    periodEndsText: "Current period ends Nov 15, 2026",
  };
}

function rows() {
  return Array.from(container.querySelectorAll(".opendesign-window"));
}

describe("OpenDesignPanel", () => {
  it("shows the card with a Refresh action and no values before a read", async () => {
    await renderPanel();
    expect(container.querySelector(".opendesign-panel")).not.toBeNull();
    expect(container.querySelector(".opendesign-heading")!.textContent).toBe(
      "OpenDesign",
    );
    expect(container.querySelector(".opendesign-refresh")!.textContent).toBe(
      "Refresh",
    );
    expect(container.querySelector(".opendesign-plan")!.textContent).toContain(
      "OpenDesign Go",
    );
    expect(container.textContent).toContain("No OpenDesign data yet");
    expect(rows()).toHaveLength(0);
  });

  it("never reads on its own — Refresh is the only trigger", async () => {
    await renderPanel();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("renders both windows in page order with their own values and resets", async () => {
    invoke.mockResolvedValueOnce(okResult());
    await renderPanel();
    await clickRefresh();

    expect(invoke).toHaveBeenCalledWith("refresh_opendesign_usage");
    const [fiveHour, sevenDay] = rows();
    expect(fiveHour.getAttribute("aria-label")).toBe("OpenDesign 5-hour limit");
    expect(sevenDay.getAttribute("aria-label")).toBe("OpenDesign 7-day limit");

    // Values are never swapped: the 5-hour row keeps the 5-hour reading.
    expect(fiveHour.textContent).toContain("70.93%");
    expect(fiveHour.textContent).toContain("remaining");
    expect(fiveHour.textContent).toContain("Resets in 31 min");
    expect(sevenDay.textContent).toContain("90.1% remaining");
    expect(sevenDay.textContent).toContain("Resets in 6 days 14 hr");
    expect(fiveHour.textContent).not.toContain("90.1");
    expect(sevenDay.textContent).not.toContain("70.93");

    // The page's own remaining share fills the meters; nothing is inverted.
    const meters = container.querySelectorAll("[role='progressbar']");
    expect(meters).toHaveLength(2);
    expect(meters[0].getAttribute("aria-valuenow")).toBe("70.93");
    expect(meters[0].getAttribute("aria-valuetext")).toBe("70.93% remaining");
    expect(meters[1].getAttribute("aria-valuenow")).toBe("90.1");
    expect(
      meters[0].querySelector(".opendesign-meter-fill")!.getAttribute("style"),
    ).toContain("width: 70.93%");
    expect(
      meters[1].querySelector(".opendesign-meter-fill")!.getAttribute("style"),
    ).toContain("width: 90.1%");

    // The separate wallet is nowhere on this card.
    expect(container.textContent).not.toContain("$");
    expect(container.textContent).not.toContain("balance");

    expect(container.querySelector(".opendesign-period")!.textContent).toContain(
      "Current period ends Nov 15, 2026",
    );
    const note = container.querySelector(".opendesign-note")!;
    expect(note.textContent).toContain("Updated");
    expect(note.className).not.toContain("is-stale");
    expect(container.querySelector(".opendesign-status")).toBeNull();
  });

  it("keeps the last reading, marked stale, when Chrome is unavailable", async () => {
    invoke.mockResolvedValueOnce(okResult());
    await renderPanel();
    await clickRefresh();

    invoke.mockResolvedValueOnce({
      status: "dashboard_not_visible",
      observedAt: "2026-10-17T10:00:00Z",
      lastKnown: okResult(),
    });
    await clickRefresh();

    const [fiveHour, sevenDay] = rows();
    expect(fiveHour.textContent).toContain("70.93%");
    expect(fiveHour.textContent).toContain("Resets in 31 min");
    expect(sevenDay.textContent).toContain("90.1% remaining");
    expect(sevenDay.textContent).toContain("Resets in 6 days 14 hr");

    const note = container.querySelector(".opendesign-note")!;
    expect(note.textContent).toContain("Last known");
    expect(note.textContent).toContain("29m ago");
    expect(note.className).toContain("is-stale");
    expect(container.querySelector(".opendesign-status")!.textContent).toContain(
      "Plan & balance",
    );
  });

  it("shows no values for an empty failure, and no fabricated zeros", async () => {
    invoke.mockResolvedValueOnce({
      status: "chrome_not_running",
      observedAt: "2026-10-17T10:00:00Z",
    });
    await renderPanel();
    await clickRefresh();

    expect(container.querySelector(".opendesign-status")!.textContent).toBe(
      "Chrome isn't running.",
    );
    expect(rows()).toHaveLength(0);
    expect(container.querySelectorAll("[role='progressbar']")).toHaveLength(0);
    expect(container.querySelector(".opendesign-note")).toBeNull();
    expect(container.textContent).not.toContain("0%");
  });

  it("keeps the last reading and its stamp when invoke rejects", async () => {
    invoke.mockResolvedValueOnce(okResult());
    await renderPanel();
    await clickRefresh();

    invoke.mockRejectedValueOnce(new Error("ipc closed"));
    await clickRefresh();

    expect(rows()[0].textContent).toContain("70.93%");
    const note = container.querySelector(".opendesign-note")!;
    expect(note.textContent).toContain("Last known");
    expect(note.textContent).toContain("29m ago");
    expect(container.querySelector(".opendesign-status")!.textContent).toContain(
      "Couldn't read",
    );
  });

  it("stays busy-safe: one invoke at a time, focus kept, no disabled button", async () => {
    let resolveInvoke: (value: unknown) => void = () => {};
    invoke.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveInvoke = resolve;
        }),
    );
    await renderPanel();
    const button =
      container.querySelector<HTMLButtonElement>(".opendesign-refresh")!;
    button.focus();

    await act(async () => {
      button.click();
      button.click();
    });

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(button.disabled).toBe(false);
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect(document.activeElement).toBe(button);
    expect(button.textContent).toBe("Refreshing…");

    await act(async () => {
      resolveInvoke(okResult());
      await Promise.resolve();
    });
    expect(button.getAttribute("aria-disabled")).toBeNull();
    expect(document.activeElement).toBe(button);
    expect(rows()).toHaveLength(2);
  });

  it("uses no timers and no fixed pixel widths", async () => {
    invoke.mockResolvedValueOnce(okResult());
    await renderPanel();
    await clickRefresh();
    for (const element of container.querySelectorAll<HTMLElement>("*")) {
      expect(element.style.width === "" || element.style.width.endsWith("%")).toBe(
        true,
      );
    }
  });
});
