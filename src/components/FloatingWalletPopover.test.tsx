// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { FloatingWalletPopover } from "./FloatingWalletPopover";
import type { FloatingWalletItem } from "../lib/floatingQuota";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = new Date("2026-09-28T10:00:00.000Z");
const CHECKED_AT = "2026-09-28T09:55:00.000Z";

function item(extra: Partial<FloatingWalletItem> = {}): FloatingWalletItem {
  return {
    id: "openrouter",
    name: "OpenRouter",
    state: "healthy",
    remainingUsd: 12.5,
    checkedAt: CHECKED_AT,
    statusLabel: "Live",
    ...extra,
  };
}

type PopoverProps = Parameters<typeof FloatingWalletPopover>[0];

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

async function renderPopover(overrides: Partial<PopoverProps> = {}) {
  const props: PopoverProps = {
    item: item(),
    now: NOW,
    pinned: false,
    align: 0,
    exiting: false,
    busy: false,
    onMouseEnter: vi.fn(),
    onMouseLeave: vi.fn(),
    onFocus: vi.fn(),
    onBlur: vi.fn(),
    onRefresh: vi.fn(),
    ...overrides,
  };
  await act(async () => root.render(<FloatingWalletPopover {...props} />));
  return props;
}

function dialog(): HTMLElement {
  const element = document.querySelector<HTMLElement>('[role="dialog"]');
  expect(element).not.toBeNull();
  return element!;
}

function refreshButton(): HTMLButtonElement {
  const button = dialog().querySelector<HTMLButtonElement>(".fq-wallet-refresh");
  expect(button).not.toBeNull();
  return button!;
}

async function click(element: Element) {
  await act(async () => {
    element.dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true }),
    );
  });
}

describe("FloatingWalletPopover", () => {
  it("presents the wallet balance with its status and observation age", async () => {
    await renderPopover();
    const text = dialog().textContent ?? "";
    expect(text).toContain("OpenRouter");
    expect(text).toContain("$12.50");
    expect(text).toContain("remaining");
    expect(text).toContain("Live");
    expect(text).toContain("Checked ");
    expect(text).toContain("5m ago");
    expect(dialog().getAttribute("data-wallet")).toBe("openrouter");
    expect(dialog().getAttribute("data-state")).toBe("healthy");
  });

  it("labels a retained reading as last known rather than freshly checked", async () => {
    await renderPopover({
      item: item({ state: "stale", statusLabel: "Stale" }),
    });
    expect(dialog().textContent).toContain("Last known");
    expect(dialog().textContent).toContain("Stale");
  });

  it("explains a rate-limit cooldown and disables the wallet action", async () => {
    const props = await renderPopover({
      item: item({ state: "cooldown", statusLabel: "Cooling down" }),
    });
    expect(dialog().textContent).toContain("Cooling down");
    expect(dialog().textContent).toContain("Cooling down: the last check hit");
    expect(refreshButton().disabled).toBe(true);
    expect(
      refreshButton().getAttribute("aria-describedby"),
    ).toBeTruthy();
    await click(refreshButton());
    expect(props.onRefresh).not.toHaveBeenCalled();
  });

  it("shows a 429 as rate limited and never retries the action", async () => {
    const props = await renderPopover({
      item: item({
        state: "error",
        statusLabel: "Rate limited",
        error: "OpenRouter rate limited the check",
        errorHttpStatus: 429,
      }),
    });
    const text = dialog().textContent ?? "";
    expect(text).toContain("Rate limited");
    expect(text).toContain("HTTP 429");
    expect(text).toContain("OpenRouter rate limited the check");
    expect(refreshButton().disabled).toBe(false);
    await click(refreshButton());
    expect(props.onRefresh).toHaveBeenCalledTimes(1);
  });

  it("shows a failed check with no balance recorded and no $0.00", async () => {
    await renderPopover({
      item: item({
        state: "error",
        remainingUsd: null,
        checkedAt: null,
        statusLabel: "Refresh failed",
        error: "credential missing",
      }),
    });
    const text = dialog().textContent ?? "";
    expect(text).toContain("$ —");
    expect(text).toContain("balance unavailable");
    expect(text).toContain("Last check failed");
    expect(text).toContain("credential missing");
    expect(text).not.toContain("$0.00");
    expect(dialog().querySelector(".fq-wallet-balance")!.className).toContain(
      "is-unavailable",
    );
  });

  it("describes an unavailable wallet with no observation as unchecked", async () => {
    await renderPopover({
      item: item({
        state: "unavailable",
        remainingUsd: null,
        checkedAt: null,
        statusLabel: "Unavailable",
      }),
    });
    expect(dialog().textContent).toContain("No balance recorded");
    expect(dialog().textContent).not.toContain("just now");
  });

  it("refreshes only through its own action and shows the in-flight state", async () => {
    const props = await renderPopover();
    expect(refreshButton().textContent).toBe("Refresh");
    expect(refreshButton().disabled).toBe(false);
    await click(refreshButton());
    expect(props.onRefresh).toHaveBeenCalledTimes(1);

    await renderPopover({ busy: true });
    expect(refreshButton().textContent).toBe("Refreshing…");
    expect(refreshButton().disabled).toBe(true);
    expect(refreshButton().getAttribute("aria-busy")).toBe("true");
  });

  it("keeps the masked credential identity muted in the footer", async () => {
    await renderPopover({ item: item({ identity: "key:1234" }) });
    const footer = dialog().querySelector(".fq-footer");
    expect(footer).not.toBeNull();
    expect(footer!.textContent).toContain("credential: key:1234");
  });

  it("moves focus to Refresh when pinned and to the card when it is disabled", async () => {
    await renderPopover({ pinned: true });
    expect(document.activeElement).toBe(refreshButton());

    await renderPopover({
      pinned: false,
      item: item({ state: "cooldown", statusLabel: "Cooling down" }),
    });
    await renderPopover({
      pinned: true,
      item: item({ state: "cooldown", statusLabel: "Cooling down" }),
    });
    expect(document.activeElement).toBe(dialog());
  });

  it("forwards hover, focus and the exit class to the shared lifecycle", async () => {
    const props = await renderPopover({ exiting: true });
    expect(dialog().className).toContain("is-exiting");
    await act(async () => {
      dialog().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    await act(async () => {
      dialog().dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    await act(async () => {
      dialog().dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
    });
    expect(props.onMouseEnter).toHaveBeenCalled();
    expect(props.onMouseLeave).toHaveBeenCalled();
    expect(props.onFocus).toHaveBeenCalled();
  });
});

