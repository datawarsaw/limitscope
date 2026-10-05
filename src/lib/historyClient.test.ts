// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({
  invoke: vi.fn(async () => [] as unknown),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  clearRuntimeHistory,
  loadRuntimeHistory,
  loadRuntimeHistoryRange,
} from "./historyClient";

describe("historyClient", () => {
  beforeEach(() => {
    invoke.mockClear();
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it("returns empty array when outside Tauri runtime", async () => {
    const result = await loadRuntimeHistoryRange({ range: "7d" });
    expect(result).toEqual([]);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("invokes get_history_range with defaults when in Tauri runtime", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    invoke.mockResolvedValueOnce([
      {
        providerId: "zai",
        windowLabel: "5-hour",
        usedPercent: 42,
        observedAt: "2026-09-28T12:00:00.000Z",
      },
    ]);

    const result = await loadRuntimeHistoryRange();
    expect(invoke).toHaveBeenCalledWith("get_history_range", {
      providerId: null,
      account: null,
      windowLabel: null,
      range: "24h",
      exactAccount: false,
    });
    expect(result).toHaveLength(1);
    expect(result[0].providerId).toBe("zai");
  });

  it("forwards range, provider, and account parameters correctly", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    invoke.mockResolvedValueOnce([]);

    await loadRuntimeHistoryRange({
      providerId: "opencode-go",
      account: "key:1234",
      windowLabel: "weekly",
      range: "7d",
    });

    expect(invoke).toHaveBeenCalledWith("get_history_range", {
      providerId: "opencode-go",
      account: "key:1234",
      windowLabel: "weekly",
      range: "7d",
      exactAccount: true,
    });
  });

  it("sets exactAccount to true when account is explicitly null (unattributed query)", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    invoke.mockResolvedValueOnce([]);

    await loadRuntimeHistoryRange({
      providerId: "antigravity",
      account: null,
      range: "7d",
    });

    expect(invoke).toHaveBeenCalledWith("get_history_range", {
      providerId: "antigravity",
      account: null,
      windowLabel: null,
      range: "7d",
      exactAccount: true,
    });
  });

  it("invokes loadRuntimeHistory for prediction queries", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    invoke.mockResolvedValueOnce([]);

    await loadRuntimeHistory();
    expect(invoke).toHaveBeenCalledWith("get_history");
  });

  it("invokes clear_history on clearRuntimeHistory", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    invoke.mockResolvedValueOnce(undefined);

    await clearRuntimeHistory();
    expect(invoke).toHaveBeenCalledWith("clear_history");
  });
});
