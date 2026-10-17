import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  openDesignEffectiveReading,
  openDesignPreservedReading,
  openDesignStatusMessage,
  openDesignWindow,
  parseRefreshResult,
  refreshOpenDesignUsage,
  sanitizeOpenDesignWindows,
} from "./openDesignManual";
import type {
  OpenDesignRefreshResult,
  OpenDesignWindow,
} from "./openDesignManual";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

const OK_RESULT: OpenDesignRefreshResult & { windows: OpenDesignWindow[] } = {
  status: "ok",
  observedAt: "2026-10-17T09:12:00Z",
  windows: [
    {
      label: "5-hour",
      remainingPercent: 70.93,
      resetText: "Resets in 31 min",
    },
    {
      label: "7-day",
      remainingPercent: 90.1,
      resetText: "Resets in 6 days 14 hr",
    },
  ],
  plan: "Go",
  periodEndsText: "Current period ends Nov 15, 2026",
};

describe("sanitizeOpenDesignWindows", () => {
  it("keeps the two known windows in page order", () => {
    const windows = sanitizeOpenDesignWindows([
      OK_RESULT.windows[1],
      OK_RESULT.windows[0],
    ]);
    expect(windows.map((window) => window.label)).toEqual(["5-hour", "7-day"]);
    expect(windows[0].remainingPercent).toBe(70.93);
    expect(windows[1].remainingPercent).toBe(90.1);
  });

  it("drops unknown labels, duplicates and non-finite or out-of-range values", () => {
    expect(sanitizeOpenDesignWindows("nope")).toEqual([]);
    expect(
      sanitizeOpenDesignWindows([
        { label: "monthly", remainingPercent: 50 },
        { label: "5-hour", remainingPercent: Number.NaN },
        { label: "5-hour", remainingPercent: 101 },
        { label: "5-hour", remainingPercent: -1 },
        { label: "5-hour", remainingPercent: "70" },
        { label: "5-hour", remainingPercent: 70 },
        { label: "5-hour", remainingPercent: 12 },
        { label: "7-day", remainingPercent: 90 },
      ]).map((window) => [window.label, window.remainingPercent]),
    ).toEqual([
      ["5-hour", 70],
      ["7-day", 90],
    ]);
  });

  it("keeps a reset line verbatim and omits an empty one", () => {
    const [window] = sanitizeOpenDesignWindows([
      { label: "5-hour", remainingPercent: 70, resetText: "Resets in 31 min" },
    ]);
    expect(window.resetText).toBe("Resets in 31 min");
    const [blank] = sanitizeOpenDesignWindows([
      { label: "5-hour", remainingPercent: 70, resetText: "   " },
    ]);
    expect(blank.resetText).toBeUndefined();
  });
});

describe("parseRefreshResult", () => {
  it("accepts a complete ok answer without changing the values", () => {
    const parsed = parseRefreshResult(OK_RESULT);
    expect(parsed?.status).toBe("ok");
    expect(parsed?.windows).toHaveLength(2);
    expect(parsed?.windows?.[0].remainingPercent).toBe(70.93);
    expect(parsed?.periodEndsText).toBe("Current period ends Nov 15, 2026");
  });

  it("rejects an ok answer that does not carry both windows", () => {
    expect(
      parseRefreshResult({ ...OK_RESULT, windows: [OK_RESULT.windows[0]] }),
    ).toBeNull();
    expect(parseRefreshResult({ ...OK_RESULT, windows: undefined })).toBeNull();
  });

  it("keeps a failure status and its retained reading", () => {
    const parsed = parseRefreshResult({
      status: "dashboard_not_visible",
      observedAt: "2026-10-17T10:00:00Z",
      lastKnown: OK_RESULT,
    });
    expect(parsed?.status).toBe("dashboard_not_visible");
    expect(parsed?.windows).toBeUndefined();
    expect(parsed?.lastKnown?.observedAt).toBe("2026-10-17T09:12:00Z");
  });

  it("rejects an unknown status word instead of guessing", () => {
    expect(parseRefreshResult({ ...OK_RESULT, status: "error" })).toBeNull();
    expect(parseRefreshResult(null)).toBeNull();
  });
});

describe("openDesignEffectiveReading", () => {
  it("shows the fresh pair after a successful refresh", () => {
    const effective = openDesignEffectiveReading({
      ...OK_RESULT,
      status: "ok" as const,
    });
    expect(effective.source).toBe("fresh");
    expect(openDesignWindow(effective.reading, "5-hour")?.remainingPercent).toBe(70.93);
    expect(openDesignWindow(effective.reading, "7-day")?.remainingPercent).toBe(90.1);
  });

  it("falls back to the retained pair, keeping its original stamp", () => {
    const effective = openDesignEffectiveReading({
      status: "chrome_not_running",
      observedAt: "2026-10-17T10:00:00Z",
      lastKnown: OK_RESULT,
    });
    expect(effective.source).toBe("last-known");
    expect(effective.reading?.observedAt).toBe("2026-10-17T09:12:00Z");
  });

  it("has nothing to show for a failure without prior data", () => {
    expect(
      openDesignEffectiveReading({
        status: "unknown",
        observedAt: "2026-10-17T10:00:00Z",
      }),
    ).toEqual({ source: "none", reading: null });
    expect(openDesignEffectiveReading(null)).toEqual({
      source: "none",
      reading: null,
    });
  });

  it("never presents a malformed ok attempt as fresh data", () => {
    const effective = openDesignEffectiveReading({
      status: "ok",
      observedAt: "2026-10-17T10:00:00Z",
      windows: [{ label: "5-hour", remainingPercent: 70 }],
    });
    expect(effective.source).toBe("none");
    expect(effective.reading).toBeNull();
  });
});

describe("refreshOpenDesignUsage", () => {
  it("invokes the one manual command and returns its parsed answer", async () => {
    invoke.mockResolvedValueOnce(OK_RESULT);
    const result = await refreshOpenDesignUsage(null);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("refresh_opendesign_usage");
    expect(result.status).toBe("ok");
  });

  it("turns a rejected invoke into an unknown attempt with the prior stamp", async () => {
    invoke.mockRejectedValueOnce(new Error("no tauri internals"));
    const previous = { ...OK_RESULT, status: "ok" as const };
    const result = await refreshOpenDesignUsage(previous);
    expect(result.status).toBe("unknown");
    expect(result.lastKnown?.observedAt).toBe("2026-10-17T09:12:00Z");
    expect(openDesignPreservedReading(result)?.windows[0].remainingPercent).toBe(
      70.93,
    );
  });

  it("treats a malformed success as unknown and keeps the previous reading", async () => {
    invoke.mockResolvedValueOnce({ status: "ok", observedAt: "x" });
    const result = await refreshOpenDesignUsage({
      ...OK_RESULT,
      status: "ok" as const,
    });
    expect(result.status).toBe("unknown");
    expect(openDesignEffectiveReading(result).source).toBe("last-known");
  });

  it("never schedules a read of its own", async () => {
    vi.useFakeTimers();
    await Promise.resolve();
    vi.runAllTimers();
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe("openDesignStatusMessage", () => {
  it("guides each unavailable state and stays silent on success", () => {
    expect(openDesignStatusMessage(null)).toContain("No OpenDesign data yet");
    expect(
      openDesignStatusMessage({
        status: "chrome_not_running",
        observedAt: "2026-10-17T10:00:00Z",
      }),
    ).toBe("Chrome isn't running.");
    expect(
      openDesignStatusMessage({
        status: "dashboard_not_visible",
        observedAt: "2026-10-17T10:00:00Z",
      }),
    ).toContain("Plan & balance");
    expect(
      openDesignStatusMessage({
        status: "unknown",
        observedAt: "2026-10-17T10:00:00Z",
      }),
    ).toContain("Couldn't read");
    expect(openDesignStatusMessage({ ...OK_RESULT, status: "ok" })).toBeNull();
  });
});

describe("manual-only contract", () => {
  it("pins the module to an explicit read with no timer or scheduler", () => {
    const source = readFileSync(
      join(import.meta.dirname, "openDesignManual.ts"),
      "utf8",
    );
    const code = source
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .split("\n")
      .filter((line) => !line.trimStart().startsWith("//"))
      .join("\n");
    for (const forbidden of [
      "setInterval",
      "setTimeout",
      "requestAnimationFrame",
      "document.cookie",
      "localStorage",
      "sessionStorage",
      "Authorization",
    ]) {
      expect(code).not.toContain(forbidden);
    }
  });
});
