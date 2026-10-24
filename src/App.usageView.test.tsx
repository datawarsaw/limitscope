// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";
import type { UsageIntelligence } from "./lib/usageIntelligence";

// The intelligence loader is mocked at the module boundary: the app's default
// loader IS loadUsageIntelligence, so the Usage overview in these tests
// exercises the production wiring.
const { loadUsageIntelligence } = vi.hoisted(() => ({
  loadUsageIntelligence: vi.fn(),
}));

vi.mock("./lib/usageIntelligence", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("./lib/usageIntelligence")>();
  return { ...actual, loadUsageIntelligence };
});

const mocks = vi.hoisted(() => ({
  usageIntelligence: true,
  refresh: vi.fn(),
}));

vi.mock("./hooks/useSettings", () => ({
  useSettings: () => ({
    settings: {
      launchAtStartup: false,
      refreshIntervalMinutes: 15,
      theme: "graphite",
      quotaNotifications: false,
      quotaPerspective: "used",
      providerPreferences: { order: [], hidden: [] },
      usageIntelligence: mocks.usageIntelligence,
    },
    setLaunchAtStartup: vi.fn(),
    setRefreshInterval: vi.fn(),
    setTheme: vi.fn(),
    setQuotaNotifications: vi.fn(),
    setUsageIntelligence: vi.fn(),
    setProviderHidden: vi.fn(),
    moveProvider: vi.fn(),
    setQuotaPerspective: vi.fn(),
    resetPreferences: vi.fn(),
    startupPending: false,
    startupError: null,
  }),
}));

vi.mock("./hooks/useProviderUsage", () => ({
  useProviderUsage: () => ({
    usages: [],
    loading: false,
    lastUpdatedAt: new Date("2026-09-28T21:58:00"),
    refresh: mocks.refresh,
    refreshOverdue: false,
    stale: false,
    staleMinutes: 0,
    historyRevision: 1,
    usageIntelligenceRevision: 0,
  }),
}));

vi.mock("./hooks/useNow", () => ({
  useNow: () => new Date("2026-09-28T22:00:00").getTime(),
}));

vi.mock("./hooks/useQuotaPredictions", () => ({
  useQuotaPredictions: () => ({
    predictionFor: () => undefined,
    historyUnavailable: false,
    clearLocalHistory: vi.fn(),
  }),
}));

function intelligencePayload(
  range: "today" | "7d" | "30d" = "today",
): UsageIntelligence {
  return {
    schemaVersion: 1,
    range,
    rangeStart: "2026-09-28T00:00:00.000Z",
    rangeEnd: "2026-09-28T22:00:00.000Z",
    generatedAt: "2026-09-28T22:00:00.000Z",
    enabled: true,
    eventsInRange: 12,
    groups: [
      {
        provider: "zai",
        models: [
          {
            model: "GLM-5.3",
            events: 9,
            inputTokens: 412_000,
            cacheReadTokens: 3_150_000,
            cacheWriteTokens: 148_000,
            outputTokens: 530_000,
            reasoningTokens: 302_000,
            totalTokens: 4_542_000,
          },
        ],
        events: 9,
        inputTokens: 412_000,
        cacheReadTokens: 3_150_000,
        cacheWriteTokens: 148_000,
        outputTokens: 530_000,
        reasoningTokens: 302_000,
        totalTokens: 4_542_000,
      },
    ],
    totals: {
      events: 12,
      inputTokens: 500_000,
      cacheReadTokens: 3_200_000,
      cacheWriteTokens: 160_000,
      outputTokens: 600_000,
      reasoningTokens: 310_000,
      totalTokens: 4_770_000,
    },
    collectionStartedAt: "2026-09-19T08:00:00.000Z",
    incompleteHistory: false,
    sources: [
      {
        source: "zcode",
        state: "ok",
        watermarkAt: "2026-09-28T21:20:00.000Z",
        baselinedAt: "2026-09-19T08:00:00.000Z",
        lastObservedAt: "2026-09-28T21:48:00.000Z",
      },
    ],
    semantics: "reportedTokenUsageCollectedLocally",
  };
}

describe("Usage | Settings primary navigation", () => {
  beforeEach(() => {
    mocks.usageIntelligence = true;
    loadUsageIntelligence.mockReset();
    loadUsageIntelligence.mockImplementation(async (query: { range: string }) =>
      intelligencePayload(query.range as "today"),
    );
  });

  afterEach(cleanup);

  it("lands on the Usage overview with exactly two destinations", async () => {
    render(<App />);
    const usageMain = await screen.findByLabelText("Usage overview");
    expect(usageMain).toBeTruthy();
    // The removed Status/Overview destination is gone from navigation.
    expect(screen.queryByRole("button", { name: "Overview" })).toBeNull();
    const nav = screen.getByRole("group", { name: "View" });
    const buttons = within(nav).getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual([
      "Usage",
      "Settings",
    ]);
    expect(
      within(nav).getByRole("button", { name: "Usage" }).getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("queries Usage Intelligence on mount with the Today range", async () => {
    render(<App />);
    await screen.findByText("Reported tokens");
    expect(loadUsageIntelligence).toHaveBeenCalledWith(
      expect.objectContaining({ range: "today" }),
    );
  });

  it("re-queries when the range changes", async () => {
    render(<App />);
    await screen.findByText("Reported tokens");
    fireEvent.click(screen.getByRole("button", { name: "7d" }));
    await vi.waitFor(() =>
      expect(loadUsageIntelligence).toHaveBeenLastCalledWith({ range: "7d" }),
    );
  });

  it("never queries while the opt-in is off and routes to Settings", async () => {
    mocks.usageIntelligence = false;
    render(<App />);
    expect(
      await screen.findByText("Usage Intelligence is off."),
    ).toBeTruthy();
    expect(loadUsageIntelligence).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    expect(
      await screen.findByRole("switch", { name: "Quota notifications" }),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Usage overview")).toBeNull();
  });

  it("isolates an intelligence failure from the shell", async () => {
    loadUsageIntelligence.mockRejectedValue(new Error("store unavailable"));
    render(<App />);
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/store unavailable/)).toBeTruthy();
    // The shell stays fully functional…
    expect(
      (screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
    // …and Settings still opens.
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      await screen.findByRole("switch", { name: "Quota notifications" }),
    ).toBeTruthy();
  });

  it("keeps the utility bar and manual refresh across both views", async () => {
    render(<App />);
    expect(screen.getByText(/Updated/)).toBeTruthy();
    expect(screen.getByText("Auto-refresh · 15 min")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(mocks.refresh).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByText(/Updated/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(mocks.refresh).toHaveBeenCalledTimes(2);
    // Round trip back to the usage overview.
    fireEvent.click(screen.getByRole("button", { name: "Usage" }));
    expect(await screen.findByLabelText("Usage overview")).toBeTruthy();
  });
});
