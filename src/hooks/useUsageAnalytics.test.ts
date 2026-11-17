// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { useUsageAnalytics } from "./useUsageAnalytics";
import type {
  UsageAnalytics,
  UsageAnalyticsQuery,
} from "../lib/usageAnalytics";

/**
 * The displayed analytics payload is bound to the provider / identity scope
 * it was computed for. A payload for one provider must never be presented
 * under another — not while the new scope loads, not after its query fails,
 * and not when a stale in-flight response for the old scope lands late.
 */

function payload(marker: string, range: "24h" | "7d" = "24h"): UsageAnalytics {
  return {
    schemaVersion: 1,
    range,
    rangeStart: "2026-11-14T00:00:00.000Z",
    rangeEnd: "2026-11-15T00:00:00.000Z",
    // The marker identifies which provider's projection is on screen.
    generatedAt: marker,
    timezoneOffsetMinutes: 0,
    summary: {
      observedResetCycles: {
        count: 0,
        exactness: "lowerBound",
        detection: "historyResetBoundaryTransitions",
      },
      observedDays: { count: 0, timezoneOffsetMinutes: 0 },
    },
    heatmap: [],
    trends: [],
    gapSemantics: "notObservedNeverZeroFilled",
    availabilityInference: "none",
  } as UsageAnalytics;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const ZAI: UsageAnalyticsQuery = { range: "24h", providerId: "zai" };
const GROK: UsageAnalyticsQuery = { range: "24h", providerId: "grok" };

describe("useUsageAnalytics provider scope binding", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("never presents a previous provider's payload after the new provider's query fails", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const seen: Array<{ provider: string | undefined; marker: string | null }> = [];
    const loader = vi.fn(async (query: UsageAnalyticsQuery) => {
      if (query.providerId === "grok") throw new Error("grok unavailable");
      return payload("zai-payload");
    });
    const { result, rerender } = renderHook(
      ({ query }) => {
        const view = useUsageAnalytics(query, true, 1, loader);
        seen.push({ provider: query.providerId, marker: view.data?.generatedAt ?? null });
        return view;
      },
      { initialProps: { query: ZAI } },
    );
    await waitFor(() => expect(result.current.data?.generatedAt).toBe("zai-payload"));

    rerender({ query: GROK });
    await waitFor(() => expect(result.current.error).toBe("grok unavailable"));

    expect(result.current.data).toBeNull();
    expect(result.current.loading).toBe(false);
    // No render under the grok scope ever carried the zai projection.
    const leaked = seen.filter((s) => s.provider === "grok" && s.marker === "zai-payload");
    expect(leaked).toEqual([]);
  });

  it("discards a stale in-flight response for the previous provider", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const zai = deferred<UsageAnalytics>();
    const grok = deferred<UsageAnalytics>();
    const loader = vi.fn((query: UsageAnalyticsQuery) =>
      query.providerId === "grok" ? grok.promise : zai.promise,
    );
    const { result, rerender } = renderHook(
      ({ query }) => useUsageAnalytics(query, true, 1, loader),
      { initialProps: { query: ZAI } },
    );
    rerender({ query: GROK });
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(2));

    // grok fails first, then the old zai request completes late.
    await act(async () => {
      grok.reject(new Error("grok unavailable"));
      await grok.promise.catch(() => {});
    });
    await act(async () => {
      zai.resolve(payload("zai-payload"));
      await zai.promise;
    });

    expect(result.current.data).toBeNull();
    expect(result.current.error).toBe("grok unavailable");
  });

  it("clears the previous provider's payload immediately while the new one loads", async () => {
    const grok = deferred<UsageAnalytics>();
    const loader = vi.fn((query: UsageAnalyticsQuery) =>
      query.providerId === "grok" ? grok.promise : Promise.resolve(payload("zai-payload")),
    );
    const { result, rerender } = renderHook(
      ({ query }) => useUsageAnalytics(query, true, 1, loader),
      { initialProps: { query: ZAI } },
    );
    await waitFor(() => expect(result.current.data?.generatedAt).toBe("zai-payload"));

    rerender({ query: GROK });
    expect(result.current.data).toBeNull();
    expect(result.current.loading).toBe(true);

    await act(async () => {
      grok.resolve(payload("grok-payload"));
      await grok.promise;
    });
    expect(result.current.data?.generatedAt).toBe("grok-payload");
  });

  it("does not show one account's payload under another account filter on failure", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const loader = vi.fn(async (query: UsageAnalyticsQuery) => {
      if (query.account === "b") throw new Error("account b failed");
      return payload("account-a");
    });
    const { result, rerender } = renderHook(
      ({ query }) => useUsageAnalytics(query, true, 1, loader),
      { initialProps: { query: { ...ZAI, account: "a", exactAccount: true } as UsageAnalyticsQuery } },
    );
    await waitFor(() => expect(result.current.data?.generatedAt).toBe("account-a"));
    rerender({ query: { ...ZAI, account: "b", exactAccount: true } });
    await waitFor(() => expect(result.current.error).toBe("account b failed"));
    expect(result.current.data).toBeNull();
  });

  it("still keeps the same provider's payload visible across a range change", async () => {
    const sevenDay = deferred<UsageAnalytics>();
    const loader = vi.fn((query: UsageAnalyticsQuery) =>
      query.range === "7d" ? sevenDay.promise : Promise.resolve(payload("zai-24h")),
    );
    const { result, rerender } = renderHook(
      ({ query }) => useUsageAnalytics(query, true, 1, loader),
      { initialProps: { query: ZAI } },
    );
    await waitFor(() => expect(result.current.data?.generatedAt).toBe("zai-24h"));

    rerender({ query: { ...ZAI, range: "7d" } });
    await waitFor(() => expect(result.current.refreshing).toBe(true));
    expect(result.current.data?.generatedAt).toBe("zai-24h");

    await act(async () => {
      sevenDay.resolve(payload("zai-7d", "7d"));
      await sevenDay.promise;
    });
    expect(result.current.data?.generatedAt).toBe("zai-7d");
    expect(result.current.refreshing).toBe(false);
  });
});
