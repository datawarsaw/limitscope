// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { useQuotaPredictions } from "./useQuotaPredictions";
import { LEGACY_QUOTA_HISTORY_KEY, type QuotaObservation } from "../lib/quotaHistory";
import * as quotaHistoryModule from "../lib/quotaHistory";
import type { ProviderUsage } from "../types";

/**
 * Pins the v0.5 phase 2 history-ownership rules: Rust is the single history
 * owner and the hook is a read-only consumer. No view — main or floating —
 * writes quota history anymore, the legacy localStorage blob migrates to
 * Rust exactly once, and a clear goes to Rust and updates the UI
 * immediately.
 */

const windowLabel = { current: "main" };

const mocks = vi.hoisted(() => ({
  loadRuntimeHistory: vi.fn(async () => [] as QuotaObservation[]),
  clearRuntimeHistory: vi.fn(async () => {}),
  importLegacyHistory: vi.fn(async () => ({ accepted: 2, rejected: 0 })),
  readLegacyHistory: vi.fn((): { present: boolean; observations: unknown[] } => ({
    present: false,
    observations: [],
  })),
  removeLegacyHistory: vi.fn(() => {}),
}));

vi.mock("../lib/historyClient", () => ({
  loadRuntimeHistory: mocks.loadRuntimeHistory,
  clearRuntimeHistory: mocks.clearRuntimeHistory,
  importLegacyHistory: mocks.importLegacyHistory,
  readLegacyHistory: mocks.readLegacyHistory,
  removeLegacyHistory: mocks.removeLegacyHistory,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: windowLabel.current }),
}));

function okUsage(checkedAt: string): ProviderUsage {
  return {
    id: "opencode-go",
    name: "OpenCode Go",
    status: "ok",
    health: "live",
    checkedAt,
    limits: [{ label: "5-hour", usedPercent: 42, resetAt: "2026-09-29T13:00:00.000Z" }],
  };
}

const CHECKED_AT = "2026-09-29T12:00:00.000Z";
const NOW_MS = Date.parse(CHECKED_AT);

function seededUsage(): ProviderUsage {
  return okUsage(CHECKED_AT);
}

beforeEach(() => {
  // Simulate the Tauri runtime so the view-role guard actually consults the
  // window label (without the marker it must treat the context as main).
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  localStorage.clear();
  vi.spyOn(Date, "now").mockReturnValue(NOW_MS);
  windowLabel.current = "main";
  mocks.loadRuntimeHistory.mockClear().mockResolvedValue([]);
  mocks.clearRuntimeHistory.mockClear();
  mocks.importLegacyHistory.mockClear().mockResolvedValue({ accepted: 2, rejected: 0 });
  mocks.readLegacyHistory.mockClear().mockReturnValue({ present: false, observations: [] });
  mocks.removeLegacyHistory.mockClear();
});

afterEach(() => {
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("quota history ownership (Rust-owned, read-only TS)", () => {
  it("reads history from the Rust runtime on attach", async () => {
    mocks.loadRuntimeHistory.mockResolvedValue([
      {
        providerId: "opencode-go",
        windowLabel: "5-hour",
        usedPercent: 42,
        observedAt: CHECKED_AT,
        account: "key:3456",
      },
    ]);
    const usages = [seededUsage()];
    const { result } = renderHook(() => useQuotaPredictions(usages, NOW_MS, 3));
    await waitFor(() => expect(mocks.loadRuntimeHistory).toHaveBeenCalled());
    // The Rust read result is consumed as-is; no crash, no unavailable note.
    expect(result.current.historyUnavailable).toBe(false);
  });

  it("never writes localStorage history, across cycles and views", async () => {
    const usages = [seededUsage()];
    const { rerender } = renderHook(
      ({ revision }) => useQuotaPredictions(usages, NOW_MS, revision),
      { initialProps: { revision: null as number | null } },
    );
    await waitFor(() => expect(mocks.loadRuntimeHistory).toHaveBeenCalled());
    // New cycles re-render the hook — with the retired interim ownership
    // the main view recorded into localStorage exactly here.
    rerender({ revision: 1 });
    rerender({ revision: 2 });
    await new Promise((resolve) => setTimeout(resolve, 0));
    // The legacy key must never (re-)appear: there is no TS write path.
    expect(localStorage.getItem(LEGACY_QUOTA_HISTORY_KEY)).toBeNull();
  });

  it("keeps the legacy module write-free (no dual-write surface remains)", () => {
    // The retired store exposed recordObservations/loadHistory/clearHistory
    // etc.; the surviving module carries only the type contract and the
    // legacy key.
    const exports = Object.keys(quotaHistoryModule).sort();
    expect(exports).toEqual(["LEGACY_QUOTA_HISTORY_KEY"]);
  });

  it("does not run the migration from a non-main (floating) window view", async () => {
    windowLabel.current = "floating-quota";
    const usages = [seededUsage()];
    renderHook(() => useQuotaPredictions(usages, NOW_MS, 1));
    await new Promise((resolve) => setTimeout(resolve, 0));
    // The floating view is a pure snapshot/history consumer: it reads Rust
    // history but never imports, so a floating-only session can never race
    // the main window's one-time migration.
    expect(mocks.importLegacyHistory).not.toHaveBeenCalled();
    expect(mocks.removeLegacyHistory).not.toHaveBeenCalled();
    expect(mocks.loadRuntimeHistory).toHaveBeenCalled();
  });

  it("re-reads Rust history when the snapshot's history revision moves", async () => {
    const usages = [seededUsage()];
    const { rerender } = renderHook(
      ({ revision }) => useQuotaPredictions(usages, NOW_MS, revision),
      { initialProps: { revision: null as number | null } },
    );
    await waitFor(() => expect(mocks.loadRuntimeHistory).toHaveBeenCalledTimes(1));
    // A moved revision (new observations / prune / import) pulls once more.
    rerender({ revision: 1 });
    await waitFor(() => expect(mocks.loadRuntimeHistory).toHaveBeenCalledTimes(2));
    // An unchanged revision after the next cycle: no extra pull.
    rerender({ revision: 1 });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(mocks.loadRuntimeHistory).toHaveBeenCalledTimes(2);
    rerender({ revision: 2 });
    await waitFor(() => expect(mocks.loadRuntimeHistory).toHaveBeenCalledTimes(3));
  });

  it("clears through the Rust runtime and refreshes predictions immediately", async () => {
    mocks.loadRuntimeHistory.mockResolvedValue([
      {
        providerId: "opencode-go",
        windowLabel: "5-hour",
        usedPercent: 42,
        observedAt: CHECKED_AT,
      },
    ]);
    const usages = [seededUsage()];
    const { result } = renderHook(() => useQuotaPredictions(usages, NOW_MS, 1));
    await waitFor(() =>
      expect(result.current.predictionFor("opencode-go", "5-hour")).toBeDefined(),
    );
    // The clear is an owned-data operation now: it reports its outcome, so
    // the test awaits the same operation the UI awaits.
    await act(async () => {
      await result.current.clearLocalHistory();
    });
    // Rust clear_history is the backend; the legacy blob cannot re-import
    // behind the clear.
    await waitFor(() => expect(mocks.clearRuntimeHistory).toHaveBeenCalled());
    expect(mocks.removeLegacyHistory).toHaveBeenCalled();
    // The in-memory history drops immediately: predictions are gone from
    // the UI without waiting for a runtime cycle.
    expect(result.current.predictionFor("opencode-go", "5-hour")).toBeUndefined();
    expect(result.current.historyUnavailable).toBe(false);
  });

  it("surfaces historyUnavailable when the Rust read fails, without crashing", async () => {
    mocks.loadRuntimeHistory.mockRejectedValue(new Error("bridge down"));
    const usages = [seededUsage()];
    const { result } = renderHook(() => useQuotaPredictions(usages, NOW_MS, 1));
    await waitFor(() => expect(result.current.historyUnavailable).toBe(true));
  });
});

describe("legacy localStorage migration", () => {
  it("imports the legacy blob once and removes the key on success", async () => {
    // Model the real storage: once the key is removed, later mounts read
    // `present: false` and never import again.
    let legacyPresent = true;
    mocks.readLegacyHistory.mockImplementation(() => ({
      present: legacyPresent,
      observations: legacyPresent
        ? [{ providerId: "zai", windowLabel: "5-hour", usedPercent: 30, observedAt: CHECKED_AT }]
        : [],
    }));
    mocks.removeLegacyHistory.mockImplementation(() => {
      legacyPresent = false;
    });
    const usages = [seededUsage()];
    renderHook(() => useQuotaPredictions(usages, NOW_MS, 1));
    await waitFor(() => expect(mocks.importLegacyHistory).toHaveBeenCalledTimes(1));
    expect(mocks.importLegacyHistory).toHaveBeenCalledWith([
      { providerId: "zai", windowLabel: "5-hour", usedPercent: 30, observedAt: CHECKED_AT },
    ]);
    await waitFor(() => expect(mocks.removeLegacyHistory).toHaveBeenCalled());
    expect(localStorage.getItem(LEGACY_QUOTA_HISTORY_KEY)).toBeNull();

    // Remount (another attach of the main view): the key is gone, so the
    // migration never runs again — even with revisions still arriving.
    const { rerender } = renderHook(
      ({ revision }) => useQuotaPredictions(usages, NOW_MS, revision),
      { initialProps: { revision: 2 } },
    );
    rerender({ revision: 3 });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(mocks.importLegacyHistory).toHaveBeenCalledTimes(1);
  });

  it("leaves the legacy key in place when the import fails, without crashing", async () => {
    mocks.readLegacyHistory.mockReturnValue({
      present: true,
      observations: [{ providerId: "zai", windowLabel: "5-hour", usedPercent: 30, observedAt: CHECKED_AT }],
    });
    mocks.importLegacyHistory.mockRejectedValue(new Error("import failed"));
    const usages = [seededUsage()];
    const { result } = renderHook(() => useQuotaPredictions(usages, NOW_MS, 1));
    await waitFor(() => expect(mocks.importLegacyHistory).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    // Failed import: the key stays for the next launch, the key removal is
    // skipped, and the hook keeps serving its API.
    expect(mocks.removeLegacyHistory).not.toHaveBeenCalled();
    expect(typeof result.current.clearLocalHistory).toBe("function");
    expect(result.current.predictionFor("opencode-go", "5-hour")).toBeUndefined();
  });
});
