// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import type { QuotaPrediction } from "./lib/prediction/types";
import type { ProviderUsage } from "./types";
import type { UsageAnalytics } from "./lib/usageAnalytics";

// The analytics loader is mocked at the module boundary: the app's default
// loader IS loadUsageAnalytics, so the Usage view in these tests exercises
// the production wiring.
const { loadUsageAnalytics } = vi.hoisted(() => ({
  loadUsageAnalytics: vi.fn(),
}));

vi.mock("./lib/usageAnalytics", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./lib/usageAnalytics")>();
  return { ...actual, loadUsageAnalytics };
});

vi.mock("./hooks/useSettings", () => ({
  useSettings: () => ({
    settings: {
      launchAtStartup: false,
      refreshIntervalMinutes: 15,
      theme: "graphite",
      quotaNotifications: false,
      quotaPerspective: "used",
    },
    setLaunchAtStartup: vi.fn(),
    setRefreshInterval: vi.fn(),
    setTheme: vi.fn(),
    setQuotaNotifications: vi.fn(),
    setQuotaPerspective: vi.fn(),
    startupPending: false,
    startupError: null,
  }),
}));

const mocks = vi.hoisted(() => ({
  usages: [] as ProviderUsage[],
  predictionFor: (_providerId: string, _windowLabel: string) =>
    undefined as QuotaPrediction | undefined,
  refresh: vi.fn(),
}));

vi.mock("./hooks/useProviderUsage", () => ({
  useProviderUsage: () => ({
    usages: mocks.usages,
    loading: false,
    lastUpdatedAt: new Date("2026-09-28T21:58:00"),
    refresh: mocks.refresh,
    refreshOverdue: false,
    stale: false,
    staleMinutes: 0,
    historyRevision: 1,
  }),
}));

vi.mock("./hooks/useNow", () => ({
  useNow: () => new Date("2026-09-28T22:00:00").getTime(),
}));

vi.mock("./hooks/useQuotaPredictions", () => ({
  useQuotaPredictions: () => ({
    predictionFor: mocks.predictionFor,
    historyUnavailable: false,
    clearLocalHistory: vi.fn(),
  }),
}));

function usage(overrides: Partial<ProviderUsage>): ProviderUsage {
  return {
    id: "zai",
    name: "Z.ai",
    status: "ok",
    health: "live",
    checkedAt: "2026-09-28T21:58:00Z",
    limits: [],
    ...overrides,
  };
}

function standardUsages(): ProviderUsage[] {
  return [
    usage({
      id: "openai-codex",
      name: "OpenAI / Codex",
      limits: [{ label: "Weekly credits", usedPercent: 78 }],
    }),
    usage({
      id: "grok",
      name: "Grok (xAI)",
      limits: [{ label: "Weekly", usedPercent: 96 }],
    }),
  ];
}

function analyticsPayload(): UsageAnalytics {
  return {
    schemaVersion: 1,
    range: "24h",
    rangeStart: "2026-09-27T22:00:00.000Z",
    rangeEnd: "2026-09-28T22:00:00.000Z",
    generatedAt: "2026-09-28T22:00:00.000Z",
    timezoneOffsetMinutes: 0,
    summary: {
      peakObservedUsage: {
        usedPercent: 84,
        providerId: "openai-codex",
        windowLabel: "Weekly credits",
        observedAt: "2026-09-28T18:00:00.000Z",
        exactness: "exact",
      },
      mostConstrainedWindow: {
        usedPercent: 84,
        providerId: "openai-codex",
        windowLabel: "Weekly credits",
        observedAt: "2026-09-28T18:00:00.000Z",
        exactness: "exact",
      },
      observedResetCycles: {
        count: 1,
        exactness: "lowerBound",
        detection: "historyResetBoundaryTransitions",
      },
      observedDays: { count: 1, timezoneOffsetMinutes: 0 },
      timeNearLimit: undefined,
    },
    heatmap: [
      {
        date: "2026-09-28",
        observed: true,
        peakUsedPercent: 84,
        peakObservedAt: "2026-09-28T18:00:00.000Z",
        providerId: "openai-codex",
        windowLabel: "Weekly credits",
        band: 4,
        peakExactness: "exact",
      },
      { date: "2026-09-27", observed: false },
    ],
    trends: [
      {
        providerId: "openai-codex",
        windowLabel: "Weekly credits",
        points: [
          {
            observedAt: "2026-09-28T02:00:00.000Z",
            usedPercent: 40,
            cycleId: "c1",
            cycleStart: true,
            resetBoundary: false,
            resolution: "detailed",
          },
          {
            observedAt: "2026-09-28T18:00:00.000Z",
            usedPercent: 84,
            cycleId: "c1",
            cycleStart: false,
            resetBoundary: false,
            resolution: "detailed",
          },
        ],
        coverage: {
          gapThresholdMs: 9 * 60 * 60 * 1000,
          comparableSpanMs: 16 * 60 * 60 * 1000,
          comparableSpanRatio: 0.66,
          gaps: [],
        },
      },
    ],
    gapSemantics: "notObservedNeverZeroFilled",
    availabilityInference: "none",
  };
}

describe("Overview | Usage primary navigation", () => {
  beforeEach(() => {
    mocks.usages = [];
    mocks.predictionFor = () => undefined;
    loadUsageAnalytics.mockReset();
    loadUsageAnalytics.mockResolvedValue(analyticsPayload());
  });

  afterEach(cleanup);

  it("switches to the Usage view and back, keeping Overview intact", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    // Overview is the landing view.
    expect(screen.getByRole("tabpanel")).toBeTruthy();
    expect(screen.queryByText("Peak observed")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Usage" }));
    const usageRegion = await screen.findByLabelText("Usage analytics");
    expect(within(usageRegion).getByText("Peak observed")).toBeTruthy();
    // The attention rail is Overview-only; the provider rail stays.
    expect(screen.queryByLabelText("Needs attention")).toBeNull();
    expect(screen.getByRole("tablist", { name: "Providers" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Overview" }));
    expect(screen.getByRole("tabpanel")).toBeTruthy();
    expect(within(screen.getByRole("tabpanel")).getByText("Grok (xAI)")).toBeTruthy();
    expect(screen.queryByLabelText("Usage analytics")).toBeNull();
  });

  it("queries analytics only while the Usage view is open", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    expect(loadUsageAnalytics).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    expect(loadUsageAnalytics).toHaveBeenCalledWith({ range: "24h" });
  });

  it("scopes the analytics query from the provider rail", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    fireEvent.click(screen.getByRole("tab", { name: /OpenAI \/ Codex/ }));
    await screen.findByText("All providers");
    expect(loadUsageAnalytics).toHaveBeenLastCalledWith({
      range: "24h",
      providerId: "openai-codex",
    });
  });

  it("keeps the provider rail keyboard-operable in the Usage view", async () => {
    const user = userEvent.setup();
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    // Nothing selected under "All providers"; the first entry is the tab stop.
    const codex = screen.getByRole("tab", { name: /OpenAI \/ Codex/ });
    expect(codex.tabIndex).toBe(0);
    codex.focus();
    await user.keyboard("{ArrowDown}");
    const grok = screen.getByRole("tab", { name: /Grok \(xAI\)/ });
    expect(grok.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(grok);
    await waitForLastQuery({ range: "24h", providerId: "grok" });
  });

  it("isolates an analytics failure from the live provider UI", async () => {
    loadUsageAnalytics.mockRejectedValue(new Error("analytics down"));
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Usage" }));
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/analytics down/)).toBeTruthy();
    // The provider rail and header stay fully functional…
    expect(screen.getByRole("tablist", { name: "Providers" })).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled,
    ).toBe(false);
    // …and Overview renders the live quota cards untouched by the failure.
    fireEvent.click(screen.getByRole("button", { name: "Overview" }));
    const panel = screen.getByRole("tabpanel");
    expect(within(panel).getByText("Grok (xAI)")).toBeTruthy();
    expect(panel.querySelector(".primary-percent")?.textContent).toBe(
      "96% used",
    );
  });

  it("keeps the settings drawer and utility bar in the Usage view", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Usage" }));
    await screen.findByText("Peak observed");
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      screen.getByRole("switch", { name: "Quota notifications" }),
    ).toBeTruthy();
  });
});

async function waitForLastQuery(query: Record<string, unknown>) {
  await vi.waitFor(() => {
    expect(loadUsageAnalytics).toHaveBeenLastCalledWith(query);
  });
}
