// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import type { QuotaPrediction } from "./lib/prediction/types";
import type { ProviderUsage } from "./types";

// Hook mocks: the dashboard consumes runtime state; tests inject snapshots.
const mocks = vi.hoisted(() => ({
  usages: [] as ProviderUsage[],
  predictionFor: (_providerId: string, _windowLabel: string) =>
    undefined as QuotaPrediction | undefined,
  refresh: vi.fn(),
  quotaPerspective: "used" as "used" | "remaining",
}));

vi.mock("./hooks/useSettings", () => ({
  useSettings: () => ({
    settings: {
      launchAtStartup: false,
      refreshIntervalMinutes: 15,
      theme: "graphite",
      quotaNotifications: false,
      providerPreferences: { order: [], hidden: [] },
      quotaPerspective: mocks.quotaPerspective,
    },
    setLaunchAtStartup: vi.fn(),
    setRefreshInterval: vi.fn(),
    setTheme: vi.fn(),
    setQuotaNotifications: vi.fn(),
    setProviderHidden: vi.fn(),
    moveProvider: vi.fn(),
    setQuotaPerspective: vi.fn(),
    startupPending: false,
    startupError: null,
  }),
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
    usageIntelligenceRevision: 0,
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

function prediction(
  overrides: Partial<QuotaPrediction>,
): QuotaPrediction {
  return {
    providerId: "openai-codex",
    windowLabel: "Weekly credits",
    burnRatePerHour: 2,
    projectedPercentAtReset: 96,
    confidence: "high",
    basis: {
      segmentId: "openai-codex|Weekly credits|0",
      segmentCount: 1,
      segmentSampleCount: 5,
      fitSampleCount: 5,
      fitSpanMinutes: 300,
      fitMeanGapMinutes: 60,
      usedWholeSegment: false,
      isStale: false,
      resetExpired: false,
    },
    ...overrides,
  };
}

/** The fixture the structural tests build on: five production providers. */
function standardUsages(): ProviderUsage[] {
  return [
    usage({
      id: "openai-codex",
      name: "OpenAI / Codex",
      limits: [
        { label: "5-hour window", usedPercent: 34, resetAt: "2026-09-29T02:00:00Z" },
        { label: "Weekly credits", usedPercent: 78, resetAt: "2026-10-01T09:15:00Z" },
      ],
    }),
    usage({
      id: "zai",
      name: "Z.ai",
      account: { label: "key:3456", note: "Primary workspace" },
      limits: [{ label: "30-day credits", usedPercent: 42 }],
    }),
    usage({
      id: "opencode-go",
      name: "OpenCode Go",
      limits: [{ label: "Weekly requests", usedPercent: 12 }],
    }),
    usage({
      id: "antigravity",
      name: "Google Antigravity",
      status: "stale",
      health: "stale",
      sourceUpdatedAt: "2026-09-28T18:00:00Z",
      dataFreshness: "stale",
      limits: [{ label: "Weekly requests", usedPercent: 20 }],
    }),
    usage({
      id: "grok",
      name: "Grok (xAI)",
      limits: [{ label: "Weekly", usedPercent: 96, resetAt: "2026-09-30T12:00:00Z" }],
    }),
  ];
}

function renderApp(usages: ProviderUsage[]) {
  mocks.usages = usages;
  return render(<App />);
}

function railTab(name: RegExp | string) {
  return screen.getByRole("tab", { name });
}

function primaryPanel() {
  return screen.getByRole("tabpanel");
}

describe("main-window dashboard information architecture", () => {
  beforeEach(() => {
    mocks.usages = [];
    mocks.predictionFor = () => undefined;
    mocks.quotaPerspective = "used";
  });
  afterEach(() => {
    cleanup();
  });

  it("anchors Zone A on the provider with the highest usable percent", () => {
    renderApp(standardUsages());
    // Grok's 96% Weekly window is the most constrained quota in the fixture.
    const panel = primaryPanel();
    expect(within(panel).getByText("Grok (xAI)")).toBeTruthy();
    expect(primaryPercent(panel)).toBe("96% used");
    expect(
      railTab(/Grok \(xAI\).*96% used in the Weekly window/).getAttribute("aria-selected"),
    ).toBe("true");
  });

  it("updates Zone A and Zone B when the user selects another provider", () => {
    renderApp(standardUsages());
    fireEvent.click(railTab(/OpenAI \/ Codex/));
    const panel = primaryPanel();
    expect(within(panel).getByText("OpenAI / Codex")).toBeTruthy();
    // Zone B follows the selection: Codex's two windows, not Grok's one.
    expect(
      Array.from(panel.querySelectorAll(".limit .limit-label")).map(
        (label) => label.textContent,
      ),
    ).toEqual(["5-hour window", "Weekly credits"]);
  });

  it("keeps an explicit selection across refreshes and falls back when it disappears", () => {
    const { rerender } = renderApp(standardUsages());
    fireEvent.click(railTab(/OpenCode Go/));
    mocks.usages = standardUsages().filter(
      (provider) => provider.id !== "opencode-go",
    );
    rerender(<App />);
    // The selected provider is gone; the deterministic default takes over.
    expect(within(primaryPanel()).getByText("Grok (xAI)")).toBeTruthy();
  });

  it("uses the provider's highest valid window as the primary presentation", () => {
    renderApp([
      usage({
        id: "openai-codex",
        name: "OpenAI / Codex",
        limits: [
          { label: "5-hour window", usedPercent: 34 },
          { label: "Weekly credits", usedPercent: 78 },
        ],
      }),
    ]);
    const panel = primaryPanel();
    expect(primaryWindowLabel(panel)).toBe("Weekly credits");
    expect(primaryPercent(panel)).toBe("78% used");
  });

  it("excludes malformed windows from the primary panel and the detail list", () => {
    renderApp([
      usage({
        id: "openai-codex",
        name: "OpenAI / Codex",
        limits: [
          { label: "", usedPercent: 55 },
          { label: "Broken NaN", usedPercent: Number.NaN },
          { label: "Negative", usedPercent: -3 },
          { label: "Over-full", usedPercent: 140 },
          { label: "Weekly credits", usedPercent: 78 },
        ],
      }),
    ]);
    const panel = primaryPanel();
    expect(primaryWindowLabel(panel)).toBe("Weekly credits");
    expect(panel.querySelectorAll(".limit .limit-label")).toHaveLength(1);
    expect(within(panel).queryByText("Broken NaN")).toBeNull();
    expect(within(panel).queryByText("Negative")).toBeNull();
    expect(within(panel).queryByText("Over-full")).toBeNull();
    expect(primaryPercent(panel)).toBe("78% used");
  });

  it("shows every valid window as a compact detail row", () => {
    renderApp([
      usage({
        id: "openai-codex",
        name: "OpenAI / Codex",
        limits: [
          { label: "5-hour window", usedPercent: 34, resetAt: "2026-09-29T02:00:00Z" },
          { label: "Weekly credits", usedPercent: 78, resetAt: "2026-10-01T09:15:00Z" },
        ],
      }),
    ]);
    const rows = screen.getAllByRole("progressbar");
    expect(rows).toHaveLength(3); // one big meter + two detail rows
    expect(screen.getByText("Quota windows")).toBeTruthy();
    // The countdown is timezone-independent; the absolute time is not. The
    // line appears twice by design: Zone A's big meter and the detail row.
    expect(screen.getAllByText(/Resets .+ · in 2d 13h/).length).toBeGreaterThan(0);
  });

  it("lists threshold, stale, and error cases in Needs attention", () => {
    renderApp(standardUsages());
    const rail = screen.getByLabelText("Needs attention");
    // Grok 96% (critical), Antigravity stale — Z.ai's 42% stays quiet.
    expect(within(rail).getByText("Critical · 96% used · Weekly")).toBeTruthy();
    expect(within(rail).getByText("Source data stale")).toBeTruthy();
    expect(within(rail).queryByText(/Z\.ai/)).toBeNull();
  });

  it("shows the calm empty state when nothing needs attention", () => {
    renderApp([
      usage({ id: "zai", name: "Z.ai", limits: [{ label: "30-day credits", usedPercent: 42 }] }),
    ]);
    expect(screen.getByText("All providers look healthy")).toBeTruthy();
  });

  it("renders Remaining mode from canonical values without color-only severity", () => {
    mocks.quotaPerspective = "remaining";
    renderApp([
      usage({
        id: "grok",
        name: "Grok (xAI)",
        limits: [{ label: "Weekly", usedPercent: 95 }],
      }),
    ]);

    const panel = primaryPanel();
    expect(primaryPercent(panel)).toBe("5% remaining");
    const meter = panel.querySelector('[role="progressbar"]')!;
    expect(meter.getAttribute("aria-valuenow")).toBe("5");
    expect(meter.getAttribute("aria-valuetext")).toBe("5% remaining");
    expect(meter.querySelector(".bar-fill")!.getAttribute("style")).toContain(
      "width: 5%",
    );
    expect(
      within(screen.getByLabelText("Needs attention")).getByText(
        "Critical · 5% remaining · Weekly",
      ),
    ).toBeTruthy();
  });

  it("represents an unavailable provider without inventing a number", () => {
    renderApp([
      usage({
        id: "zai",
        name: "Z.ai",
        status: "error",
        health: "error",
        error: "Z.ai usage could not be fetched.",
        limits: [],
      }),
      usage({
        id: "grok",
        name: "Grok (xAI)",
        limits: [{ label: "Weekly", usedPercent: 10 }],
      }),
    ]);
    // Zone A defaults to Grok (highest usable); Z.ai is reachable via rail…
    fireEvent.click(railTab(/Z\.ai/));
    const panel = primaryPanel();
    expect(within(panel).getByText("No saved provider data")).toBeTruthy();
    expect(within(panel).getByText("Z.ai usage could not be fetched.")).toBeTruthy();
    // …the rail shows no percentage for it…
    expect(railTab(/Z\.ai: refresh failed/).textContent).not.toContain("%");
    // …and the attention rail carries the failure.
    expect(within(screen.getByLabelText("Needs attention")).getByText("Refresh failed")).toBeTruthy();
  });

  it("shows the pace line only when the prediction engine's gate accepts it", () => {
    const eligible = prediction({
      willExhaustBeforeReset: true,
      estimatedExhaustionAt: "2026-09-29T10:00:00Z",
    });
    const usages = [
      usage({
        id: "openai-codex",
        name: "OpenAI / Codex",
        limits: [{ label: "Weekly credits", usedPercent: 78 }],
      }),
    ];

    mocks.predictionFor = (_id, label) =>
      label === "Weekly credits" ? eligible : undefined;
    const { unmount } = renderApp(usages);
    expect(
      screen.getByText("At current pace · may reach the limit before reset"),
    ).toBeTruthy();
    unmount();

    // Same prediction, but the provider is stale: the gate suppresses it.
    mocks.predictionFor = () => eligible;
    renderApp([
      usage({
        id: "openai-codex",
        name: "OpenAI / Codex",
        status: "stale",
        health: "stale",
        dataFreshness: "stale",
        limits: [{ label: "Weekly credits", usedPercent: 78 }],
      }),
    ]);
    expect(screen.queryByText(/At current pace/)).toBeNull();
  });

  it("keeps account and source attribution visible", () => {
    renderApp([
      usage({
        id: "zai",
        name: "Z.ai",
        account: { label: "key:3456", note: "Additional accounts may exist locally" },
        checkedAt: "2026-09-28T21:58:00Z",
        limits: [{ label: "30-day credits", usedPercent: 42 }],
      }),
    ]);
    const panel = primaryPanel();
    expect(within(panel).getByText(/account key:3456/)).toBeTruthy();
    expect(within(panel).getByText(/Additional accounts may exist locally/)).toBeTruthy();
    expect(within(panel).getByText(/Checked/)).toBeTruthy();
  });

  it("still refreshes from the header", () => {
    renderApp(standardUsages());
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(mocks.refresh).toHaveBeenCalledTimes(1);
  });

  it("moves provider selection with the keyboard and keeps tab semantics", async () => {
    const user = userEvent.setup();
    renderApp(standardUsages());
    const grok = railTab(/Grok \(xAI\)/);
    // Grok is selected by default and is the roving tab stop.
    expect(elementTabIndex(grok)).toBe(0);

    await user.click(grok);
    await user.keyboard("{ArrowUp}");
    // Registry order: Antigravity is one above Grok; selection and focus move.
    const antigravity = railTab(/Google Antigravity/);
    expect(antigravity.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(antigravity);
    expect(within(primaryPanel()).getByText("Google Antigravity")).toBeTruthy();

    await user.keyboard("{Home}");
    expect(railTab(/OpenAI \/ Codex/).getAttribute("aria-selected")).toBe("true");
    await user.keyboard("{End}");
    expect(grok.getAttribute("aria-selected")).toBe("true");

    // Only the selected tab is a tab stop.
    expect(elementTabIndex(railTab(/OpenAI \/ Codex/))).toBe(-1);
  });

  it("selects the provider from an attention item", () => {
    renderApp(standardUsages());
    fireEvent.click(
      screen.getByRole("button", { name: /Antigravity: Source data stale/ }),
    );
    expect(within(primaryPanel()).getByText("Google Antigravity")).toBeTruthy();
  });

  it("applies the structural layout class and follows the rail orientation", () => {
    // jsdom has no matchMedia: the deterministic fallback is the stacked
    // narrow layout with a horizontal chip rail.
    const first = renderApp(standardUsages());
    const app = first.container.querySelector(".app") as HTMLElement;
    expect(app.dataset.layout).toBe("narrow");
    expect(
      screen.getByRole("tablist", { name: "Providers" }).getAttribute("aria-orientation"),
    ).toBe("horizontal");
    first.unmount();

    const media = vi.fn().mockReturnValue({
      matches: true,
      addEventListener: () => {},
      removeEventListener: () => {},
    });
    vi.stubGlobal("matchMedia", media);
    const second = renderApp(standardUsages());
    const wideApp = second.container.querySelector(".app") as HTMLElement;
    expect(wideApp.dataset.layout).toBe("wide");
    expect(
      screen.getByRole("tablist", { name: "Providers" }).getAttribute("aria-orientation"),
    ).toBe("vertical");
    second.unmount();
    vi.unstubAllGlobals();
  });

  it("keeps the notification setting reachable in the settings drawer", () => {
    renderApp(standardUsages());
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      screen.getByRole("switch", { name: "Quota notifications" }),
    ).toBeTruthy();
  });

  it("shows a loading state before the first snapshot", () => {
    renderApp([]);
    expect(screen.getByText("Loading provider data…")).toBeTruthy();
    expect(screen.queryByLabelText("Needs attention")).toBeNull();
  });

  it("carries the LimitScope product identity in the main header", () => {
    renderApp(standardUsages());
    expect(screen.getByRole("heading", { name: "LimitScope" })).toBeTruthy();
  });
});

function primaryPercent(panel: HTMLElement): string | null {
  return panel.querySelector(".primary-percent")?.textContent ?? null;
}

function primaryWindowLabel(panel: HTMLElement): string | null {
  return panel.querySelector(".primary-window")?.textContent ?? null;
}

function elementTabIndex(element: HTMLElement): number {
  const value = element.tabIndex;
  return Number.isNaN(value) ? -1 : value;
}
