// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({
  invoke: vi.fn(async () => ({}) as unknown),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { loadUsageAnalytics } from "./usageAnalytics";

describe("usageAnalytics client contract", () => {
  beforeEach(() => {
    invoke.mockClear();
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it("does not fabricate an analytics result outside Tauri", async () => {
    await expect(
      loadUsageAnalytics({ range: "24h" }),
    ).rejects.toThrow("Usage analytics requires the Tauri runtime");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("invokes one get_usage_analytics command with the normalized query", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const response = { schemaVersion: 1 };
    invoke.mockResolvedValueOnce(response);

    await expect(
      loadUsageAnalytics({
        range: "7d",
        providerId: "zai",
        account: null,
        windowLabel: "5-hour",
        exactAccount: true,
      }),
    ).resolves.toEqual(response);
    expect(invoke).toHaveBeenCalledWith("get_usage_analytics", {
      query: {
        range: "7d",
        providerId: "zai",
        account: null,
        windowLabel: "5-hour",
        exactAccount: true,
      },
    });
  });
});
