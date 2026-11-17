// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import type { QuotaPrediction } from "./lib/prediction/types";
import type { ProviderUsage } from "./types";
import type { CodexScope, UsageAnalytics } from "./lib/usageAnalytics";
import type { UsageIntelligence } from "./lib/usageIntelligence";

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

// The Usage Intelligence loader is mocked the same way: TokenUsageSection's
// production default is loadUsageIntelligence.
const { loadUsageIntelligence } = vi.hoisted(() => ({
  loadUsageIntelligence: vi.fn(),
}));

vi.mock("./lib/usageIntelligence", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("./lib/usageIntelligence")>();
  return { ...actual, loadUsageIntelligence };
});

vi.mock("./hooks/useSettings", () => ({
  useSettings: () => ({
    settings: {
      launchAtStartup: false,
      refreshIntervalMinutes: 15,
      theme: "graphite",
      quotaNotifications: false,
      quotaPerspective: "used",
      usageIntelligence: mocks.usageIntelligence,
      codexAccounts: {
        accounts: [
          { accountId: "openai-codex", alias: "", codexHome: "", monitored: true },
        ],
      },
    },
    setLaunchAtStartup: mocks.settingsWrite,
    setRefreshInterval: mocks.settingsWrite,
    setTheme: mocks.settingsWrite,
    setQuotaNotifications: mocks.settingsWrite,
    setUsageIntelligence: mocks.settingsWrite,
    setProviderHidden: mocks.settingsWrite,
    moveProvider: mocks.settingsWrite,
    setWalletMonitored: mocks.settingsWrite,
    setWalletInFloatingBar: mocks.settingsWrite,
    setQuotaPerspective: mocks.settingsWrite,
    setCodexAccount: mocks.settingsWrite,
    resetPreferences: mocks.settingsWrite,
    syncRefreshInterval: mocks.syncRefreshInterval,
    startupPending: false,
    startupError: null,
    unverifiedPreferences: [],
  }),
}));

const mocks = vi.hoisted(() => ({
  usages: [] as ProviderUsage[],
  usageIntelligence: false,
  // Mirrors useProviderUsage: [] until the runtime's first snapshot — never
  // undefined, so the consumer cannot mistake "unobserved" for "no Codex
  // slot expected".
  codexIdentityStates: [] as CodexScope,
  predictionFor: (_providerId: string, _windowLabel: string) =>
    undefined as QuotaPrediction | undefined,
  refresh: vi.fn(),
  settingsWrite: vi.fn(),
  syncRefreshInterval: vi.fn(),
  providerUsageArgs: [] as unknown[],
}));

vi.mock("./hooks/useProviderUsage", () => ({
  useProviderUsage: (...args: unknown[]) => {
    mocks.providerUsageArgs = args;
    return {
    usages: mocks.usages,
    wallets: [],
    codexIdentityStates: mocks.codexIdentityStates,
    loading: false,
    lastUpdatedAt: new Date("2026-09-28T21:58:00"),
    refresh: mocks.refresh,
    refreshOverdue: false,
    stale: false,
    staleMinutes: 0,
    historyRevision: 1,
    usageIntelligenceRevision: 0,
    };
  },
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
    // The projection's Codex session scope: generation 1 reads as a newer
    // observed session than any generation-0 expectation derived from the
    // visible usages, so the session fence accepts the payload.
    codexIdentityScope: [
      { providerId: "openai-codex", generation: 1, identity: null },
    ],
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

function intelligencePayload(): UsageIntelligence {
  const sums = {
    events: 2,
    inputTokens: 7089,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    outputTokens: 1377,
    reasoningTokens: 0,
    totalTokens: 8466,
  };
  return {
    schemaVersion: 1,
    range: "today",
    rangeStart: "2026-09-28T00:00:00.000Z",
    rangeEnd: "2026-09-28T22:00:00.000Z",
    generatedAt: "2026-09-28T22:00:00.000Z",
    enabled: true,
    eventsInRange: 2,
    groups: [
      { provider: "openai-codex", ...sums, models: [{ model: "gpt-5.4", ...sums }] },
    ],
    totals: sums,
    collectionStartedAt: "2026-09-20T10:00:00.000Z",
    incompleteHistory: true,
    sources: [
      {
        source: "codex",
        state: "collecting",
        watermarkAt: "2026-09-28T21:00:00.000Z",
        baselinedAt: "2026-09-20T10:00:00.000Z",
        lastObservedAt: "2026-09-28T21:00:00.000Z",
      },
    ],
    sourceCoverage: [
      {
        source: "codex",
        coverage: "incomplete",
        reason: "lateBaseline",
        coveredFrom: "2026-09-20T10:00:00.000Z",
      },
    ],
    daily: [
      { date: "2026-09-28", observed: true, events: 2, totalTokens: 8466, beforeCollection: false },
      { date: "2026-09-27", observed: false, events: 0, beforeCollection: false },
    ],
    semantics: "reportedTokenUsageCollectedLocally",
  };
}

describe("Status | Insights | Settings primary navigation", () => {
  beforeEach(() => {
    mocks.usages = [];
    mocks.predictionFor = () => undefined;
    loadUsageAnalytics.mockReset();
    loadUsageAnalytics.mockResolvedValue(analyticsPayload());
  });

  afterEach(cleanup);

  it("switches to the Insights view and back, keeping Status intact", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    // Status is the landing view.
    expect(screen.getByRole("tabpanel")).toBeTruthy();
    expect(screen.queryByText("Most constrained")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    const usageRegion = await screen.findByLabelText("Usage analytics");
    expect(within(usageRegion).getByText("Most constrained")).toBeTruthy();
    // The attention rail is Status-only; the provider rail stays.
    expect(screen.queryByLabelText("Needs attention")).toBeNull();
    expect(screen.getByRole("tablist", { name: "Providers" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Status" }));
    expect(screen.getByRole("tabpanel")).toBeTruthy();
    expect(within(screen.getByRole("tabpanel")).getByText("Grok (xAI)")).toBeTruthy();
    expect(screen.queryByLabelText("Usage analytics")).toBeNull();
  });

  it("queries analytics on landing for the selected provider, then all providers in Insights", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    // The Status history chart scopes to the deterministic default selection
    // (Grok's 96% Weekly window is the most constrained quota in the fixture).
    await waitFor(() =>
      expect(loadUsageAnalytics).toHaveBeenCalledWith({
        range: "24h",
        providerId: "grok",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    await screen.findByText("Most constrained");
    expect(loadUsageAnalytics).toHaveBeenLastCalledWith({ range: "24h" });
  });

  it("re-scopes the Status history chart when another provider is selected", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    await waitFor(() =>
      expect(loadUsageAnalytics).toHaveBeenCalledWith({
        range: "24h",
        providerId: "grok",
      }),
    );
    fireEvent.click(screen.getByRole("tab", { name: /OpenAI \/ Codex/ }));
    await waitFor(() =>
      expect(loadUsageAnalytics).toHaveBeenLastCalledWith({
        range: "24h",
        providerId: "openai-codex",
      }),
    );
  });

  it("scopes the analytics query from the provider rail in Insights", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    await screen.findByText("Most constrained");
    fireEvent.click(screen.getByRole("tab", { name: /OpenAI \/ Codex/ }));
    await screen.findByText("All providers");
    expect(loadUsageAnalytics).toHaveBeenLastCalledWith({
      range: "24h",
      providerId: "openai-codex",
    });
  });

  it("keeps the provider rail keyboard-operable in the Insights view", async () => {
    const user = userEvent.setup();
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    await screen.findByText("Most constrained");
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
    // The landing Status view owns the failure: its history card alerts…
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/analytics down/)).toBeTruthy();
    // …while the provider rail and header stay fully functional…
    expect(screen.getByRole("tablist", { name: "Providers" })).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled,
    ).toBe(false);
    // …and the live status card is untouched by the failure.
    const panel = screen.getByRole("tabpanel");
    expect(within(panel).getByText("Grok (xAI)")).toBeTruthy();
    expect(panel.querySelector(".primary-percent")?.textContent).toBe(
      "96% used",
    );
  });

  it("resets the persistent dashboard scroll position on a view change only", async () => {
    mocks.usages = standardUsages();
    const { container } = render(<App />);
    const dashboard = container.querySelector(".dashboard") as HTMLElement;
    // jsdom reports a zero scroll box, so the position is assigned directly:
    // this pins the reset contract without depending on layout measurement.
    dashboard.scrollTop = 480;
    expect(dashboard.scrollTop).toBe(480);

    // A data refresh inside Status is not navigation and must not move the
    // reader — the reset is bound to the view value alone.
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(dashboard.scrollTop).toBe(480);

    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    await screen.findByText("Most constrained");
    expect(dashboard.scrollTop).toBe(0);

    dashboard.scrollTop = 320;
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByLabelText("Settings")).toBeTruthy();
    expect(dashboard.scrollTop).toBe(0);

    dashboard.scrollTop = 240;
    fireEvent.click(screen.getByRole("button", { name: "Status" }));
    expect(screen.getByRole("tabpanel")).toBeTruthy();
    expect(dashboard.scrollTop).toBe(0);
  });

  it("keeps the utility bar in the Insights view and Settings one click away", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    await screen.findByText("Most constrained");
    expect(screen.getByText(/Updated/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      screen.getByRole("switch", { name: "Quota notifications" }),
    ).toBeTruthy();
  });
});

describe("Insights carries the v0.9 Usage Intelligence features", () => {
  beforeEach(() => {
    mocks.usages = [];
    mocks.usageIntelligence = false;
    mocks.codexIdentityStates = [];
    loadUsageAnalytics.mockReset();
    loadUsageAnalytics.mockResolvedValue(analyticsPayload());
    loadUsageIntelligence.mockReset();
    loadUsageIntelligence.mockResolvedValue(intelligencePayload());
  });

  afterEach(cleanup);

  it("never queries Usage Intelligence while the opt-in is off and routes to Settings", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    expect(
      await screen.findByText("Reported token usage collected locally is off."),
    ).toBeTruthy();
    expect(loadUsageIntelligence).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Enable in Settings" }));
    expect(
      await screen.findByRole("switch", { name: "Quota notifications" }),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Usage analytics")).toBeNull();
  });

  it("queries Usage Intelligence on opening Insights with the Today range", async () => {
    mocks.usageIntelligence = true;
    mocks.usages = standardUsages();
    render(<App />);
    // Status — the landing view — does not mount the intelligence surface.
    expect(screen.queryByLabelText("Usage Intelligence")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    await screen.findByText("Reported token usage collected locally");
    expect(loadUsageIntelligence).toHaveBeenCalledWith(
      expect.objectContaining({ range: "today" }),
    );
  });

  it("re-queries when the token-usage range changes", async () => {
    mocks.usageIntelligence = true;
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    await screen.findByText("Reported token usage collected locally");
    fireEvent.click(
      within(screen.getByRole("group", { name: "Token usage range" })).getByRole(
        "button",
        { name: "7d" },
      ),
    );
    await vi.waitFor(() =>
      expect(loadUsageIntelligence).toHaveBeenLastCalledWith(
        expect.objectContaining({ range: "7d" }),
      ),
    );
  });

  it("shows the per-source History coverage banner when a source's history is incomplete", async () => {
    mocks.usageIntelligence = true;
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    const banner = await screen.findByLabelText("History coverage");
    expect(within(banner).getByText(/history is only covered since/)).toBeTruthy();
    expect(within(banner).getByText(/Codex/)).toBeTruthy();
  });

  it("renders the local-days Daily activity heatmap beside the token table", async () => {
    mocks.usageIntelligence = true;
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    expect(await screen.findByLabelText("Daily activity")).toBeTruthy();
    expect(
      screen.getByLabelText("Daily reported token activity"),
    ).toBeTruthy();
    expect(screen.getByRole("table", { name: /Token usage by provider/ })).toBeTruthy();
  });

  it("isolates an intelligence failure inside Insights", async () => {
    mocks.usageIntelligence = true;
    mocks.usages = standardUsages();
    loadUsageIntelligence.mockRejectedValue(new Error("store unavailable"));
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/store unavailable/)).toBeTruthy();
    // The shell and the quota analytics stay fully functional…
    expect(
      (screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
    expect(await screen.findByText("Most constrained")).toBeTruthy();
  });

  it("re-scopes the Status history when the runtime Codex identity changes", async () => {
    mocks.codexIdentityStates = [
      { providerId: "openai-codex", generation: 5, identity: "account-a" },
    ];
    mocks.usages = standardUsages();
    const view = render(<App />);
    // Select the Codex provider so its history is fenced to the observed
    // session identity.
    fireEvent.click(screen.getByRole("tab", { name: /OpenAI \/ Codex/ }));
    await waitFor(() =>
      expect(loadUsageAnalytics).toHaveBeenCalledWith({
        range: "24h",
        providerId: "openai-codex",
      }),
    );
    const callsBefore = loadUsageAnalytics.mock.calls.length;
    // The runtime observes a new session for that slot: the history read
    // must be re-scoped, not served from the previous session's payload.
    mocks.codexIdentityStates = [
      { providerId: "openai-codex", generation: 6, identity: "account-b" },
    ];
    view.rerender(<App />);
    await vi.waitFor(() => {
      expect(loadUsageAnalytics.mock.calls.length).toBeGreaterThan(callsBefore);
    });
  });
});

describe("Codex session fence fails closed on an unobserved runtime scope", () => {
  beforeEach(() => {
    mocks.usages = [];
    mocks.predictionFor = () => undefined;
    loadUsageAnalytics.mockReset();
    loadUsageIntelligence.mockReset();
    loadUsageIntelligence.mockResolvedValue(intelligencePayload());
  });

  afterEach(cleanup);

  it("rejects an equal-generation response from a different identity while the runtime scope is unobserved", async () => {
    mocks.usages = standardUsages();
    mocks.codexIdentityStates = [];
    loadUsageAnalytics.mockResolvedValue({
      ...analyticsPayload(),
      codexIdentityScope: [
        { providerId: "openai-codex", generation: 0, identity: "prior-session" },
      ],
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    // The unobserved runtime scope fences to the visible identity
    // (generation 0): an equal generation under a different identity is a
    // prior account's payload and must never land.
    await waitFor(() => expect(loadUsageAnalytics).toHaveBeenCalled());
    await vi.waitFor(() =>
      expect(screen.queryByText("Loading usage…")).toBeNull(),
    );
    expect(screen.queryByText("Most constrained")).toBeNull();
  });

  it("rejects an unstamped stale projection while the runtime scope is unobserved", async () => {
    mocks.usages = standardUsages();
    mocks.codexIdentityStates = [];
    loadUsageAnalytics.mockResolvedValue({
      ...analyticsPayload(),
      codexIdentityScope: undefined,
    });
    render(<App />);
    // The landing Status view's history card is fenced the same way: first
    // the non-Codex provider renders (no session fence), then the Codex
    // provider's unstamped history must be discarded.
    fireEvent.click(screen.getByRole("tab", { name: /OpenAI \/ Codex/ }));
    await waitFor(() =>
      expect(loadUsageAnalytics).toHaveBeenCalledWith({
        range: "24h",
        providerId: "openai-codex",
      }),
    );
    await vi.waitFor(() =>
      expect(screen.queryByText("Loading history…")).toBeNull(),
    );
    expect(screen.queryByText("Codex · Weekly credits")).toBeNull();
  });

  it("accepts a matching response once the runtime observes the Codex scope", async () => {
    mocks.usages = standardUsages();
    mocks.codexIdentityStates = [
      { providerId: "openai-codex", generation: 2, identity: "account-a" },
    ];
    loadUsageAnalytics.mockResolvedValue({
      ...analyticsPayload(),
      codexIdentityScope: [
        { providerId: "openai-codex", generation: 2, identity: "account-a" },
      ],
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    // Same generation and same identity as the observed scope: the
    // projection belongs to the current session and renders.
    expect(await screen.findByText("Most constrained")).toBeTruthy();
  });
});

describe("dev fixture wiring for Usage Intelligence", () => {
  beforeEach(() => {
    mocks.usages = [];
    mocks.usageIntelligence = false;
    mocks.codexIdentityStates = [];
    loadUsageAnalytics.mockReset();
    loadUsageIntelligence.mockReset();
  });

  afterEach(() => {
    window.history.pushState({}, "", "/");
  });

  afterEach(cleanup);

  it("renders fixture intelligence data in the dev fixture flow without the production loader", async () => {
    // Dev fixtures activate via `?fixture=…` in the dev server; vitest runs
    // with import.meta.env.DEV true, so the same seam is exercised here.
    window.history.pushState({}, "", "?fixture=usage-first");
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    // The fixture's opt-in (not the setting) enables the section, and its
    // hand-authored intelligence payload renders.
    const table = await screen.findByRole("table", {
      name: /Token usage by provider/,
    });
    expect(within(table).getByText("Z.ai (ZCode)")).toBeTruthy();
    expect(within(table).getByText("GLM-5.3")).toBeTruthy();
    // The injected fixture loader served it; the production source was
    // never read — also with the persisted setting off.
    expect(loadUsageIntelligence).not.toHaveBeenCalled();
    expect(loadUsageAnalytics).not.toHaveBeenCalled();
  });
});

async function waitForLastQuery(query: Record<string, unknown>) {
  await vi.waitFor(() => {
    expect(loadUsageAnalytics).toHaveBeenLastCalledWith(query);
  });
}

describe("0.8.24 Settings authority under the Status | Insights shell", () => {
  beforeEach(() => {
    mocks.usages = [];
    mocks.usageIntelligence = false;
    mocks.codexIdentityStates = [];
    mocks.refresh.mockReset();
    mocks.settingsWrite.mockReset();
    mocks.syncRefreshInterval.mockReset();
    loadUsageAnalytics.mockReset();
    loadUsageAnalytics.mockResolvedValue(analyticsPayload());
    loadUsageIntelligence.mockReset();
    loadUsageIntelligence.mockResolvedValue(intelligencePayload());
  });

  afterEach(cleanup);

  it("hands the refresh cadence to the runtime only through the settings lane", () => {
    mocks.usages = standardUsages();
    render(<App />);
    // The hook receives the lane's hand-off, never a raw interval number
    // that would let the view push a stale localStorage copy to Rust.
    expect(mocks.providerUsageArgs).toEqual([
      { onAttach: mocks.syncRefreshInterval },
    ]);
    expect(mocks.syncRefreshInterval).not.toHaveBeenCalled();
  });

  it("never writes a preference while navigating Status, Insights and Settings", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    await screen.findByText("Most constrained");
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", { name: "Quota notifications" });
    fireEvent.click(screen.getByRole("button", { name: "Status" }));
    await screen.findByRole("region", { name: "Quota history" });
    expect(mocks.settingsWrite).not.toHaveBeenCalled();
    expect(mocks.syncRefreshInterval).not.toHaveBeenCalled();
  });

  it("keeps the utility bar and manual refresh across all three views", async () => {
    mocks.usages = standardUsages();
    render(<App />);
    expect(screen.getByText(/Updated/)).toBeTruthy();
    expect(screen.getByText("Auto-refresh · 15 min")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(mocks.refresh).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Insights" }));
    expect(screen.getByText("Auto-refresh · 15 min")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(mocks.refresh).toHaveBeenCalledTimes(2);

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByText(/Updated/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(mocks.refresh).toHaveBeenCalledTimes(3);

    fireEvent.click(screen.getByRole("button", { name: "Status" }));
    expect(
      await screen.findByRole("region", { name: "Quota history" }),
    ).toBeTruthy();
  });

  it("keeps the three primary destinations keyboard-reachable buttons", () => {
    mocks.usages = standardUsages();
    render(<App />);
    const nav = screen.getByRole("group", { name: "View" });
    const buttons = within(nav).getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual([
      "Status",
      "Insights",
      "Settings",
    ]);
    for (const button of buttons) {
      expect(button.tagName).toBe("BUTTON");
      expect(button.getAttribute("type")).toBe("button");
      expect(button.hasAttribute("aria-pressed")).toBe(true);
      expect(button.tabIndex).toBe(0);
    }
    expect(buttons[0].getAttribute("aria-pressed")).toBe("true");
  });
});
