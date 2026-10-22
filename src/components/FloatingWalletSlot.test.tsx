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
    balances: [{ currency: "USD", amount: 12.5 }],
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
    items: [item()],
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
    expect(button.getAttribute("data-wallet-id")).toBe("api-wallets");
    expect(button.getAttribute("data-state")).toBe("healthy");
    expect(button.getAttribute("aria-haspopup")).toBe("dialog");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(button.getAttribute("aria-busy")).toBeNull();
    expect(button.getAttribute("aria-label")).toBe(
      "API wallets, OpenRouter USD 12.50 remaining",
    );
    // The collapsed slot carries identity and state, never money text.
    expect(button.textContent).toBe("");
    expect(button.querySelector(".fq-wallet-dot")).not.toBeNull();
  });

  it("is one shared slot for every wallet, never one segment per wallet", async () => {
    await renderSlot({
      items: [
        item(),
        item({
          id: "deepseek",
          name: "DeepSeek",
          balances: [{ currency: "CNY", amount: 110 }],
        }),
      ],
    });
    const slots = container.querySelectorAll("[data-wallet-id]");
    expect(slots).toHaveLength(1);
    expect(slots[0].getAttribute("aria-label")).toBe(
      "API wallets, OpenRouter USD 12.50 remaining; DeepSeek CNY 110.00 remaining",
    );
  });

  it("never states a combined total across currencies", async () => {
    await renderSlot({
      items: [
        item({
          balances: [
            { currency: "USD", amount: 12.5 },
            { currency: "CNY", amount: 110 },
          ],
        }),
      ],
    });
    const label = slot().getAttribute("aria-label") ?? "";
    expect(label).toContain("USD 12.50");
    expect(label).toContain("CNY 110.00");
    expect(label).not.toContain("122.50");
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
      items: [
        item({ state: "cooldown", balances: [], statusLabel: "Cooling down" }),
      ],
    });
    const button = slot();
    expect(button.getAttribute("data-state")).toBe("cooldown");
    expect(button.className).toContain("is-cooldown");
    expect(button.getAttribute("aria-label")).toBe(
      "API wallets, OpenRouter balance unavailable",
    );
  });

  it("states the most consequential state among the wallets it covers", async () => {
    await renderSlot({
      items: [
        item(),
        item({ id: "deepseek", name: "DeepSeek", state: "error" }),
      ],
    });
    expect(slot().getAttribute("data-state")).toBe("error");
  });

  it("opens on click and announces hover for the shared pointer lifecycle", async () => {
    const props = await renderSlot();
    const button = slot();
    await act(async () => {
      button.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    expect(props.onHover).toHaveBeenCalledWith("api-wallets");
    await act(async () => {
      button.dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    expect(props.onHoverEnd).toHaveBeenCalled();
    await act(async () => {
      button.dispatchEvent(
        new MouseEvent("click", { bubbles: true, cancelable: true }),
      );
    });
    expect(props.onOpen).toHaveBeenCalledWith("api-wallets");
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
