import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  grokBotEffectiveReading,
  grokBotRemainingPercent,
  grokBotStatusMessage,
  refreshGrokBotUsage,
} from "./grokBotManual";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("grokBotRemainingPercent", () => {
  it("derives the remaining quota from the exact used percentage", () => {
    expect(grokBotRemainingPercent(73)).toBe(27);
    expect(grokBotRemainingPercent(0)).toBe(100);
    expect(grokBotRemainingPercent(100)).toBe(0);
    expect(grokBotRemainingPercent(33.3)).toBe(66.7);
  });

  it("keeps unavailable data unavailable instead of zero", () => {
    expect(grokBotRemainingPercent(null)).toBeNull();
    expect(grokBotRemainingPercent(undefined)).toBeNull();
    expect(grokBotRemainingPercent(Number.NaN)).toBeNull();
  });

  it("rejects malformed percentages out of the 0-100 domain", () => {
    expect(grokBotRemainingPercent(-5)).toBeNull();
    expect(grokBotRemainingPercent(140)).toBeNull();
    expect(grokBotRemainingPercent(Number.POSITIVE_INFINITY)).toBeNull();
  });
});

describe("grokBotEffectiveReading", () => {
  it("shows the fresh reading after a successful refresh", () => {
    const result = {
      status: "ok" as const,
      observedAt: "2026-10-08T14:32:00Z",
      usedPercent: 73,
      resetText: "Resets in 3 days",
    };
    const effective = grokBotEffectiveReading(result);
    expect(effective.source).toBe("fresh");
    expect(effective.reading?.usedPercent).toBe(73);
  });

  it("falls back to the last successful reading when a refresh fails", () => {
    const result = {
      status: "screen_not_visible" as const,
      observedAt: "2026-10-08T16:00:00Z",
      lastKnown: {
        observedAt: "2026-10-08T14:32:00Z",
        usedPercent: 73,
        resetText: "Resets in 3 days",
      },
    };
    const effective = grokBotEffectiveReading(result);
    expect(effective.source).toBe("last-known");
    // The original stamp survives — the failure never re-dates the reading.
    expect(effective.reading?.observedAt).toBe("2026-10-08T14:32:00Z");
    expect(effective.reading?.usedPercent).toBe(73);
  });

  it("has nothing to show for a failure without prior data", () => {
    expect(grokBotEffectiveReading({ status: "not_running", observedAt: "2026-10-08T16:00:00Z" }))
      .toEqual({ source: "none", reading: null });
    expect(grokBotEffectiveReading(null)).toEqual({ source: "none", reading: null });
  });
});

describe("grokBotStatusMessage", () => {
  it("guides each unavailable state instead of showing values", () => {
    expect(grokBotStatusMessage(null)).toContain("No Grok Bot data yet");
    expect(grokBotStatusMessage({ status: "not_running", observedAt: "2026-10-08T16:00:00Z" }))
      .toContain("isn't running");
    expect(
      grokBotStatusMessage({ status: "screen_not_visible", observedAt: "2026-10-08T16:00:00Z" }),
    ).toContain("Usage & Billing");
    expect(grokBotStatusMessage({ status: "unknown", observedAt: "2026-10-08T16:00:00Z" }))
      .toContain("Couldn't read");
  });

  it("explains nothing on a fresh successful read", () => {
    expect(
      grokBotStatusMessage({ status: "ok", observedAt: "2026-10-08T14:32:00Z", usedPercent: 73 }),
    ).toBeNull();
  });
});

describe("refreshGrokBotUsage", () => {
  it("issues exactly one command per explicit call", async () => {
    invoke.mockResolvedValueOnce({ status: "ok", observedAt: "2026-10-08T14:32:00Z" });
    const result = await refreshGrokBotUsage();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("refresh_grok_bot_usage");
    expect(result.status).toBe("ok");
  });

  it("maps transport failure to an unknown attempt, never to values", async () => {
    invoke.mockRejectedValueOnce(new Error("no tauri internals"));
    const result = await refreshGrokBotUsage();
    expect(result.status).toBe("unknown");
    expect(result.usedPercent).toBeUndefined();
    expect(result.resetText).toBeUndefined();
    expect(Number.isNaN(Date.parse(result.observedAt))).toBe(false);
  });

  it("keeps the previous successful reading and its original stamp when invoke rejects", async () => {
    const success = {
      status: "ok" as const,
      observedAt: "2026-10-08T14:32:00Z",
      usedPercent: 73,
      resetText: "Resets in 3 days",
      appVersion: "0.68.1.0",
    };
    invoke.mockResolvedValueOnce(success);
    const first = await refreshGrokBotUsage();
    invoke.mockRejectedValueOnce(new Error("ipc closed"));
    const failed = await refreshGrokBotUsage(first);

    expect(failed.status).toBe("unknown");
    expect(failed.usedPercent).toBeUndefined();
    expect(failed.lastKnown).toEqual({
      observedAt: "2026-10-08T14:32:00Z",
      usedPercent: 73,
      resetText: "Resets in 3 days",
      appVersion: "0.68.1.0",
    });
    expect(failed.observedAt).not.toBe(success.observedAt);
    expect(grokBotEffectiveReading(failed).reading?.observedAt).toBe("2026-10-08T14:32:00Z");
    expect(grokBotStatusMessage(failed)).toContain("Couldn't read");
  });
});

describe("manual-refresh-only contract", () => {
  it("exposes no polling or scheduling of its own", async () => {
    const source = await import("node:fs").then((fs) =>
      fs.readFileSync(new URL("./grokBotManual.ts", import.meta.url), "utf8"),
    );
    for (const forbidden of ["setInterval", "setTimeout", "requestAnimationFrame", "EventSource"]) {
      expect(source, `grokBotManual.ts must stay manual-only; found ${forbidden}`).not.toContain(
        forbidden,
      );
    }
  });
});
