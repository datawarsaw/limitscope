// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { LocateRippleSurface } from "./LocateRippleSurface";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

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
  delete document.documentElement.dataset.window;
  document.documentElement.style.background = "";
  document.body.style.background = "";
});

describe("LocateRippleSurface", () => {
  it("renders three rings in an inert stage and clears the page background", async () => {
    await act(async () => root.render(<LocateRippleSurface />));

    const stage = document.querySelector(".locate-ripple-stage");
    expect(stage).not.toBeNull();
    expect(stage?.getAttribute("aria-hidden")).toBe("true");
    expect(document.querySelectorAll(".locate-ripple-ring")).toHaveLength(3);
    expect(document.querySelectorAll(".locate-ripple-ring-1")).toHaveLength(1);
    expect(document.querySelectorAll(".locate-ripple-ring-2")).toHaveLength(1);
    expect(document.querySelectorAll(".locate-ripple-ring-3")).toHaveLength(1);
    // The overlay window must be transparent: the waves paint over the
    // desktop, never over a page background.
    expect(document.documentElement.dataset.window).toBe("locate-ripple");
    expect(document.documentElement.style.background).toBe("transparent");
    expect(document.body.style.background).toBe("transparent");
  });
});
