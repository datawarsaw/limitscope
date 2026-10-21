// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { FloatingWalletSlot } from "./FloatingWalletSlot";
import type { FloatingWalletItem } from "../lib/floatingQuota";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

function item(extra: Partial<FloatingWalletItem> = {}): FloatingWalletItem {
  return {
    id: "openrouter",
    name: "OpenRouter",
    state: "healthy",
    remainingUsd: 12.5,
    checkedAt: "2026-09-28T09:55:00.000Z",
    statusLabel: "Live",
    ...extra,
  };
}

type SlotProps = Parameters<typeof FloatingWalletSlot>[0];

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

async function renderSlot(overrides: Partial<SlotProps> = {}) {
  const props: SlotProps = {
    item: item(),
    expanded: false,
    busy: false,
    onHover: vi.fn(),
    onHoverEnd: vi.fn(),
    onOpen: vi.fn(),
    onContextMenu: vi.fn(),
    ...overrides,
  };
  await act(async () => root.render(<FloatingWalletSlot {...props} />));
  return props;
}

function slot(): HTMLButtonElement {
  const button = container.querySelector<HTMLButtonElement>("[data-wallet-id]");
  expect(button).not.toBeNull();
  return button!;
}

describe("FloatingWalletSlot", () => {
  it("is a compact 44px dialog trigger with a spoken balance and state", async () => {
    await renderSlot();
    const button = slot();
    expect(button.tagName).toBe("BUTTON");
    expect(button.className).toContain("fq-wallet");
    expect(button.className).toContain("is-healthy");
    expect(button.getAttribute("data-wallet-id")).toBe("openrouter");
    expect(button.getAttribute("data-state")).toBe("healthy");
    expect(button.getAttribute("aria-haspopup")).toBe("dialog");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(button.getAttribute("aria-busy")).toBeNull();
    expect(button.getAttribute("aria-label")).toBe(
      "OpenRouter API wallet, $12.50 remaining, Live",
    );
    // The collapsed slot carries identity and state, never money text.
    expect(button.textContent).toBe("");
    expect(button.querySelector(".fq-wallet-dot")).not.toBeNull();
  });

  it("reports an open card and a wallet-only refresh as ARIA state", async () => {
    await renderSlot({ expanded: true, busy: true });
    const button = slot();
    expect(button.getAttribute("aria-expanded")).toBe("true");
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect(button.className).toContain("is-refreshing");
  });

  it("shows the wallet's health through its data attribute and label", async () => {
    await renderSlot({
      item: item({ state: "cooldown", remainingUsd: null, statusLabel: "Cooling down" }),
    });
    const button = slot();
    expect(button.getAttribute("data-state")).toBe("cooldown");
    expect(button.className).toContain("is-cooldown");
    expect(button.getAttribute("aria-label")).toBe(
      "OpenRouter API wallet, balance unavailable, Cooling down",
    );
  });

  it("opens on click and announces hover for the shared pointer lifecycle", async () => {
    const props = await renderSlot();
    const button = slot();
    await act(async () => {
      button.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    expect(props.onHover).toHaveBeenCalledWith("openrouter");
    await act(async () => {
      button.dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    expect(props.onHoverEnd).toHaveBeenCalled();
    await act(async () => {
      button.dispatchEvent(
        new MouseEvent("click", { bubbles: true, cancelable: true }),
      );
    });
    expect(props.onOpen).toHaveBeenCalledWith("openrouter");
  });

  it("captures the context menu for the shared action menu", async () => {
    const props = await renderSlot();
    const event = new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
    });
    await act(async () => {
      slot().dispatchEvent(event);
    });
    expect(event.defaultPrevented).toBe(true);
    expect(props.onContextMenu).toHaveBeenCalledTimes(1);
  });
});

