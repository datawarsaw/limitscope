// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  dashboardLayoutFor,
  useDashboardLayout,
  MEDIUM_LAYOUT_MIN_WIDTH,
  WIDE_LAYOUT_MIN_WIDTH,
} from "./useDashboardLayout";

describe("dashboardLayoutFor", () => {
  it("maps the validated viewport widths to the stacked narrow layout", () => {
    expect(dashboardLayoutFor(360)).toBe("narrow");
    expect(dashboardLayoutFor(400)).toBe("narrow");
    expect(dashboardLayoutFor(520)).toBe("narrow");
  });

  it("starts the two-column medium layout at its breakpoint", () => {
    expect(dashboardLayoutFor(MEDIUM_LAYOUT_MIN_WIDTH)).toBe("medium");
    expect(dashboardLayoutFor(WIDE_LAYOUT_MIN_WIDTH - 1)).toBe("medium");
  });

  it("reaches the three-zone wide layout at its breakpoint and beyond", () => {
    expect(dashboardLayoutFor(WIDE_LAYOUT_MIN_WIDTH)).toBe("wide");
    expect(dashboardLayoutFor(900)).toBe("wide");
  });
});

type MediaListener = (event: { matches: boolean }) => void;

function installMatchMedia(width: number) {
  const listeners = new Set<MediaListener>();
  let currentWidth = width;
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => {
      const threshold = Number(/min-width:\s*(\d+)px/.exec(query)?.[1] ?? 0);
      const queryList = {
        get matches() {
          return currentWidth >= threshold;
        },
        addEventListener: (_: string, listener: MediaListener) => {
          listeners.add(listener);
        },
        removeEventListener: (_: string, listener: MediaListener) => {
          listeners.delete(listener);
        },
      };
      return queryList;
    }),
  );
  return {
    resize(nextWidth: number) {
      currentWidth = nextWidth;
      for (const listener of listeners) listener({ matches: currentWidth >= 0 });
    },
  };
}

describe("useDashboardLayout", () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("tracks the live window width", () => {
    const media = installMatchMedia(400);
    const { result } = renderHook(() => useDashboardLayout());
    expect(result.current).toBe("narrow");
    act(() => media.resize(700));
    expect(result.current).toBe("wide");
    act(() => media.resize(560));
    expect(result.current).toBe("medium");
  });

  it("deterministically falls back to narrow without matchMedia", () => {
    vi.stubGlobal("matchMedia", undefined);
    const { result } = renderHook(() => useDashboardLayout());
    expect(result.current).toBe("narrow");
  });
});
