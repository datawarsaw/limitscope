// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import { ExecutionRunPanel } from "./components/dashboard/ExecutionRunPanel";
import {
  beginExecutionRun,
  EMPTY_EXECUTION_RUNS_STATE,
  saveExecutionRunsState,
  type StartRunOutcome,
} from "./lib/executionRuns";
import type { QuotaPrediction } from "./lib/prediction/types";
import type { ProviderUsage } from "./types";

// Hook mocks: the dashboard consumes runtime state; tests inject snapshots.
const mocks = vi.hoisted(() => ({
  usages: [] as ProviderUsage[],
  predictionFor: (_providerId: string, _windowLabel: string) =>
    undefined as QuotaPrediction | undefined,
  refresh: vi.fn(),
}));

vi.mock("./hooks/useSettings", () => ({
  useSettings: () => ({
    settings: {
      launchAtStartup: false,
      refreshIntervalMinutes: 15,
      theme: "graphite",
      quotaNotifications: false,
    },
    setLaunchAtStartup: vi.fn(),
    setRefreshInterval: vi.fn(),
    setTheme: vi.fn(),
    setQuotaNotifications: vi.fn(),
    startupPending: false,
    startupError: null,
  }),
}));

vi.mock("./hooks/useProviderUsage", () => ({
  useProviderUsage: () => ({
    usages: mocks.usages,
    loading: false,
    lastUpdatedAt: new Date(),
    refresh: mocks.refresh,
    refreshOverdue: false,
    stale: false,
    staleMinutes: 0,
    historyRevision: 1,
  }),
}));

vi.mock("./hooks/useNow", () => ({
  useNow: () => Date.now(),
}));

vi.mock("./hooks/useQuotaPredictions", () => ({
  useQuotaPredictions: () => ({
    predictionFor: mocks.predictionFor,
    historyUnavailable: false,
    clearLocalHistory: vi.fn(),
  }),
}));

// The workflow demands a fresh baseline, so fixtures are stamped relative to
// the real clock (start/finish resolve snapshots against Date.now()).
const checkedAt = (minutesAgo: number) =>
  new Date(Date.now() - minutesAgo * 60000).toISOString();
const hoursAhead = (hours: number) =>
  new Date(Date.now() + hours * 3600000).toISOString();

function codexUsage(overrides: Partial<ProviderUsage> = {}): ProviderUsage {
  return {
    id: "openai-codex",
    name: "OpenAI / Codex",
    status: "ok",
    health: "live",
    checkedAt: checkedAt(1),
    limits: [
      { label: "5-hour", usedPercent: 23, resetAt: hoursAhead(5) },
      { label: "Weekly", usedPercent: 38, resetAt: hoursAhead(100) },
    ],
    account: { label: "key:3456", identity: "key:3456" },
    planType: "team",
    ...overrides,
  };
}

function renderApp(usages: ProviderUsage[]) {
  mocks.usages = usages;
  return render(<App />);
}

async function openRunPanel(user: ReturnType<typeof userEvent.setup>) {
  // The toolbar button's accessible name changes while a run is active
  // ("Execution run in progress, running for …"), so match the prefix.
  await user.click(screen.getByRole("button", { name: /^Execution run/ }));
}

beforeEach(() => {
  localStorage.clear();
  mocks.usages = [];
  mocks.refresh = vi.fn();
});

afterEach(() => {
  cleanup();
});

describe("execution run workflow: start to finish", () => {
  it("runs start -> active -> finish -> bounded summary with disclaimer (cases 1, 5, 6, 16)", async () => {
    const user = userEvent.setup();
    // Reset boundaries are pinned so the before/after snapshots carry the
    // exact same provider values (a drifting timestamp would legitimately
    // read as a reset change).
    const reset5 = hoursAhead(5);
    const resetWeekly = hoursAhead(100);
    const view = renderApp([
      codexUsage({
        limits: [
          { label: "5-hour", usedPercent: 23, resetAt: reset5 },
          { label: "Weekly", usedPercent: 38, resetAt: resetWeekly },
        ],
      }),
    ]);
    await openRunPanel(user);

    // Focus lands inside the opened panel (keyboard-accessible open).
    expect(document.activeElement?.textContent).toBe("Execution run");

    await user.click(screen.getByRole("button", { name: "Start run" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /Execution run in progress, running for/ }),
    ).toBeTruthy();

    // Quota moved during the "execution": 23 -> 28 on the 5-hour window.
    mocks.usages = [
      codexUsage({
        checkedAt: checkedAt(0),
        limits: [
          { label: "5-hour", usedPercent: 28, resetAt: reset5 },
          { label: "Weekly", usedPercent: 40, resetAt: resetWeekly },
        ],
      }),
    ];
    view.rerender(<App />);
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    const summary = within(screen.getByLabelText("Execution summary"));
    summary.getByText("23% → 28%");
    summary.getByText("+5 pp observed");
    summary.getByText("38% → 40%");
    summary.getByText("+2 pp observed");
    // Confidence is explicit and textual.
    summary.getByText("Bounded");
    // The mandatory non-task-cost disclaimer is present with the deltas.
    summary.getByText("Observed quota delta during execution (not exact task cost).");
    // The finished run lands in the bounded recent history.
    screen.getByText("Recent runs (1)");
    // No EXACT chip is ever rendered (case 17).
    expect(screen.queryByText("Exact")).toBeNull();
    // The header button is back to the idle start affordance.
    expect(screen.getByRole("button", { name: "Execution run" })).toBeTruthy();
  });

  it("cannot start without a usable current provider snapshot", async () => {
    const user = userEvent.setup();
    renderApp([]);
    await openRunPanel(user);
    expect(screen.getByText(/Baseline: no usable current provider snapshot/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Start run" }));
    expect(screen.getByRole("alert").textContent).toContain(
      "Cannot start: No usable current provider snapshot",
    );
  });

  it("cannot finish when the after snapshot is unavailable; run stays active", async () => {
    const user = userEvent.setup();
    const view = renderApp([codexUsage()]);
    await openRunPanel(user);
    await user.click(screen.getByRole("button", { name: "Start run" }));

    mocks.usages = [];
    view.rerender(<App />);
    await user.click(screen.getByRole("button", { name: "Finish run" }));
    expect(screen.getByRole("alert").textContent).toContain("After snapshot unavailable");
    expect(screen.getByText(/Running ·/)).toBeTruthy();
  });

  it("account change during the run shows the blocked state, never compared percentages (case 7)", async () => {
    const user = userEvent.setup();
    const reset5 = hoursAhead(5);
    const view = renderApp([
      codexUsage({ limits: [{ label: "5-hour", usedPercent: 23, resetAt: reset5 }] }),
    ]);
    await openRunPanel(user);
    await user.click(screen.getByRole("button", { name: "Start run" }));

    mocks.usages = [
      codexUsage({
        checkedAt: checkedAt(0),
        account: { label: "key:9999", identity: "key:9999" },
        limits: [{ label: "5-hour", usedPercent: 61, resetAt: reset5 }],
      }),
    ];
    view.rerender(<App />);
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    expect(screen.getAllByText(/Account changed during execution/).length).toBeGreaterThan(0);
    expect(screen.getByText("Unavailable")).toBeTruthy();
    expect(screen.queryByText(/pp observed/)).toBeNull();
    screen.getByText("Observed quota delta during execution (not exact task cost).");
  });

  it("reset crossing reports incomparable instead of a nonsensical negative delta (case 9)", async () => {
    const user = userEvent.setup();
    const view = renderApp([codexUsage()]);
    await openRunPanel(user);
    await user.click(screen.getByRole("button", { name: "Start run" }));

    // The 5-hour window reset mid-run: 23% used -> 4% used. That must never
    // read as a negative consumption. The after snapshot carries a new reset
    // boundary for that window.
    mocks.usages = [
      codexUsage({
        checkedAt: checkedAt(0),
        limits: [
          { label: "5-hour", usedPercent: 4, resetAt: hoursAhead(6) },
          { label: "Weekly", usedPercent: 40, resetAt: hoursAhead(100) },
        ],
      }),
    ];
    view.rerender(<App />);
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    expect(screen.getAllByText(/Reset occurred during this run/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/not comparable/i).length).toBeGreaterThan(0);
    expect(screen.queryByText(/pp observed/)).toBeNull();
    expect(screen.queryByText(/-19 pp/)).toBeNull();
    screen.getByText("Observed quota delta during execution (not exact task cost).");
  });

  it("discarding removes the active run without recording a summary (case 4)", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await openRunPanel(user);
    await user.click(screen.getByRole("button", { name: "Start run" }));
    await user.click(screen.getByRole("button", { name: "Discard" }));
    expect(screen.getByRole("button", { name: "Start run" })).toBeTruthy();
    expect(screen.queryByText("Execution summary")).toBeNull();
    expect(screen.queryByText(/Recent runs/)).toBeNull();
  });

  it("recovers an active run after an app restart (case 12)", async () => {
    // Simulate the previous session persisting an active bracket.
    const before = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: codexUsage(),
      harness: "Codex",
    });
    expect(before.status).toBe("started");
    if (before.status !== "started") return;
    saveExecutionRunsState(before.state);

    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await openRunPanel(user);

    expect(screen.getByText(/Running ·/)).toBeTruthy();
    expect(screen.getByText(/Started/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Finish run" }));
    screen.getByText("Execution summary");
  });

  it("a recovered run can be discarded (case 13)", async () => {
    const before = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: codexUsage(),
      harness: "Codex",
    });
    if (before.status !== "started") throw new Error("fixture failed");
    saveExecutionRunsState(before.state);

    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await openRunPanel(user);
    await user.click(screen.getByRole("button", { name: "Discard" }));
    expect(screen.getByRole("button", { name: "Start run" })).toBeTruthy();
    // The cleared state is persisted.
    expect(localStorage.getItem("limitscope.execution-runs.v1")).not.toContain("activeRun\":{");
  });
});

describe("execution run panel: bounded conflict resolution (case 3)", () => {
  const activeFixture = (() => {
    const outcome = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: codexUsage(),
      harness: "Codex",
    });
    return outcome.status === "started" ? outcome.run : undefined;
  })();

  it("a second start offers finish / discard / cancel and cancel keeps the run", async () => {
    const user = userEvent.setup();
    const onStart = vi.fn(
      (): StartRunOutcome => ({ status: "conflict", state: EMPTY_EXECUTION_RUNS_STATE, activeRun: activeFixture! }),
    );
    const onFinish = vi.fn();
    const onDiscard = vi.fn();
    render(
      <ExecutionRunPanel
        activeRun={null}
        recentRuns={[]}
        selectedUsage={codexUsage()}
        usages={[codexUsage()]}
        nowMs={Date.now()}
        onStart={onStart}
        onFinish={onFinish}
        onDiscard={onDiscard}
        onRefresh={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    await user.click(screen.getByRole("button", { name: "Start run" }));

    expect(screen.getByRole("alert").textContent).toContain("A run is already active");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    // Cancel only dismisses the resolution; the active run is untouched.
    expect(onFinish).not.toHaveBeenCalled();
    expect(onDiscard).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Start run" })).toBeTruthy();
  });
});

describe("execution run workflow: keyboard accessibility (case 18)", () => {
  it("opens with Enter, moves focus into the panel, starts via keyboard, closes with Escape back to the toolbar", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    const runButton = screen.getByRole("button", { name: "Execution run" });
    runButton.focus();
    await user.keyboard("{Enter}");
    expect(screen.getByText("Execution run", { selector: "h2" })).toBeTruthy();
    expect(document.activeElement?.textContent).toBe("Execution run");

    await user.keyboard("{Tab}");
    expect(document.activeElement?.getAttribute("aria-label")).toBe(
      "Close execution run panel",
    );

    const startButton = screen.getByRole("button", { name: "Start run" });
    startButton.focus();
    await user.keyboard("{Enter}");
    expect(screen.getByText(/Running ·/)).toBeTruthy();

    await user.keyboard("{Escape}");
    expect(screen.queryByText("Execution run", { selector: "h2" })).toBeNull();
    expect(document.activeElement).toBe(runButton);
  });
});
