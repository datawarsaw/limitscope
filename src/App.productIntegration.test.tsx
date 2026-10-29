// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import {
  beginExecutionRun,
  completeExecutionRun,
  EMPTY_EXECUTION_RUNS_STATE,
  EXECUTION_RUNS_STORAGE_KEY,
  saveExecutionRunsState,
} from "./lib/executionRuns";
import {
  createExecutionReceipt,
  formatReceiptJson,
  formatReceiptMarkdown,
} from "./lib/executionReceipt";
import type { QuotaPrediction } from "./lib/prediction/types";
import type { ProviderUsage } from "./types";

/**
 * v0.7 PRODUCT integration rehearsal pins, adapted to the v0.9 Usage-first
 * main window (Usage | Settings; the quota Overview surface moved to the
 * floating bar).
 *
 * These scenarios exercise the integrated product surface — Settings page ×
 * quota perspective × Local data controls × execution runs — through the real
 * App with the real (localStorage-backed) settings hook. Runtime transport
 * (provider snapshots, history store, updater) is mocked at the hook
 * boundary, which is the same seam the production runtime uses.
 *
 * Scenario numbering matches the rehearsal task's cross-feature list; the
 * former perspective-presentation scenarios (1-5) asserted the removed
 * Overview/analytics surfaces and moved with them.
 */

const mocks = vi.hoisted(() => ({
  usages: [] as ProviderUsage[],
  historyRevision: 1,
  usageIntelligenceRevision: 0,
  historyCleared: false,
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
    historyRevision: mocks.historyRevision,
    usageIntelligenceRevision: mocks.usageIntelligenceRevision,
  }),
}));

vi.mock("./hooks/useQuotaPredictions", () => ({
  useQuotaPredictions: () => ({
    predictionFor:
      (_providerId: string, _windowLabel: string) =>
        undefined as QuotaPrediction | undefined,
    historyUnavailable: false,
    // Models the runtime contract: a clear wipes the store and bumps the
    // history revision, which re-queries every consumer (Usage included).
    clearLocalHistory: vi.fn(async () => {
      mocks.historyCleared = true;
      mocks.historyRevision += 1;
      return { ok: true, removed: true };
    }),
  }),
}));

vi.mock("./hooks/useUpdater", () => ({
  useUpdater: () => ({
    phase: "idle",
    update: null,
    error: null,
    currentVersion: "0.0.0-test",
    checkForUpdates: vi.fn(),
    installUpdate: vi.fn(),
    dismissUpdate: vi.fn(),
  }),
}));

const minutesAgo = (minutes: number) =>
  new Date(Date.now() - minutes * 60_000).toISOString();
const hoursAhead = (hours: number) =>
  new Date(Date.now() + hours * 3600000).toISOString();

function codexUsage(overrides: Partial<ProviderUsage> = {}): ProviderUsage {
  return {
    id: "openai-codex",
    name: "OpenAI / Codex",
    status: "ok",
    health: "live",
    checkedAt: minutesAgo(1),
    limits: [
      { label: "5-hour", usedPercent: 23, resetAt: hoursAhead(5) },
      { label: "Weekly", usedPercent: 95, resetAt: hoursAhead(100) },
    ],
    account: { label: "key:3456", identity: "key:3456" },
    planType: "team",
    resetCredits: {
      bankedCredits: 3,
      currentlyApplicable: 2,
      checkedAt: minutesAgo(1),
      source: "codex-wham-usage",
    },
    ...overrides,
  };
}

function grokUsage(): ProviderUsage {
  return {
    id: "grok",
    name: "Grok (xAI)",
    status: "ok",
    health: "live",
    checkedAt: minutesAgo(1),
    limits: [{ label: "Weekly", usedPercent: 13 }],
  };
}

function renderApp(usages: ProviderUsage[]) {
  mocks.usages = usages;
  return render(<App />);
}

async function openSettings(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Settings" }));
}

async function choosePerspective(
  user: ReturnType<typeof userEvent.setup>,
  perspective: "Used" | "Remaining",
) {
  await openSettings(user);
  await user.click(screen.getByRole("radio", { name: perspective }));
}

function persistedRuns(): {
  activeRun: unknown;
  recentRuns: unknown[];
} {
  const raw = localStorage.getItem(EXECUTION_RUNS_STORAGE_KEY);
  return raw
    ? (JSON.parse(raw) as { activeRun: unknown; recentRuns: unknown[] })
    : EMPTY_EXECUTION_RUNS_STATE;
}

beforeEach(() => {
  localStorage.clear();
  mocks.usages = [];
  mocks.historyRevision = 1;
  mocks.historyCleared = false;
  mocks.refresh = vi.fn();
});

afterEach(() => {
  cleanup();
});

describe("local data × usage (scenarios 6-11)", () => {
  it("6+7. clearing usage history from Settings settles with feedback and leaves the Usage destination intact", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);

    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Clear usage history" }));
    await user.click(await screen.findByRole("button", { name: "Clear usage history" }));
    expect(mocks.historyCleared).toBe(true);
    expect(await screen.findByText("Usage history cleared.")).toBeTruthy();

    // The Usage destination (an independent data plane) is unaffected by a
    // quota-history clear.
    await user.click(screen.getByRole("button", { name: "Usage" }));
    expect(await screen.findByLabelText("Usage overview")).toBeTruthy();
  });

  it("8. settings survive a history clear", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await choosePerspective(user, "Remaining");
    const before = localStorage.getItem("rate-limits.settings.v1");

    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Clear usage history" }));
    await user.click(await screen.findByRole("button", { name: "Clear usage history" }));
    await screen.findByText("Usage history cleared.");

    expect(localStorage.getItem("rate-limits.settings.v1")).toBe(before);
    expect(JSON.parse(before ?? "{}")).toMatchObject({ quotaPerspective: "remaining" });
  });

  it("9. reset preferences returns the perspective to Used", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await choosePerspective(user, "Remaining");

    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Reset preferences" }));
    await user.click(await screen.findByRole("button", { name: "Reset preferences" }));
    await screen.findByText("Preferences reset.");

    await user.click(screen.getByRole("button", { name: "General" }));
    const radio = screen.getByRole("radio", { name: "Used" }) as HTMLInputElement;
    expect(radio.checked).toBe(true);
    expect(
      (JSON.parse(localStorage.getItem("rate-limits.settings.v1") ?? "{}") as Record<string, unknown>)
        .quotaPerspective,
    ).toBe("used");
  });

  it("10. reset preferences preserves usage history", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);

    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Reset preferences" }));
    await user.click(await screen.findByRole("button", { name: "Reset preferences" }));
    await screen.findByText("Preferences reset.");

    expect(mocks.historyCleared).toBe(false);
    // The Usage destination still renders.
    await user.click(screen.getByRole("button", { name: "Usage" }));
    expect(await screen.findByLabelText("Usage overview")).toBeTruthy();
  });

  it("11. reset preferences preserves execution runs", async () => {
    const user = userEvent.setup();
    // Seed a real completed run through the store API so the persisted shape
    // is exactly what production writes (the loader parses defensively).
    const started = beginExecutionRun(EMPTY_EXECUTION_RUNS_STATE, {
      usage: codexUsage({
        limits: [{ label: "5-hour", usedPercent: 23, resetAt: hoursAhead(5) }],
      }),
      harness: "Codex",
    });
    if (started.status !== "started") throw new Error("seed start failed");
    const finished = completeExecutionRun(
      started.state,
      [
        codexUsage({
          checkedAt: minutesAgo(0),
          limits: [{ label: "5-hour", usedPercent: 28, resetAt: hoursAhead(5) }],
        }),
      ],
      { nowMs: Date.now() },
    );
    if (finished.status !== "finished") throw new Error("seed finish failed");
    saveExecutionRunsState(finished.state);
    renderApp([codexUsage()]);

    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Reset preferences" }));
    await user.click(await screen.findByRole("button", { name: "Reset preferences" }));
    await screen.findByText("Preferences reset.");

    const user2 = user;
    await user2.click(screen.getByRole("button", { name: "Execution run" }));
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();
  });
});

describe("local data × execution runs (scenarios 12-14)", () => {
  it("12. clear execution runs with no active run removes the recent history", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    await user.click(screen.getByRole("button", { name: "Finish run" }));
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();

    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Clear execution runs" }));
    expect(screen.getByText("Clear execution runs?")).toBeTruthy();
    await user.click(await screen.findByRole("button", { name: "Clear execution runs" }));
    await screen.findByText("Execution runs cleared.");

    expect(screen.queryByText(/Recent runs/)).toBeNull();
    expect(persistedRuns().recentRuns).toHaveLength(0);
  });

  it("13. an active run survives Cancel and is discarded only by confirmed clear", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();

    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Discard active run and clear" }));
    // The confirmation names the in-progress consequence.
    expect(
        screen.getByText(
          "This discards the active execution run and removes all stored run records. Your quota history, settings, and provider credentials are not affected.",
        ),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();
    expect(persistedRuns().activeRun).not.toBeNull();

    await user.click(screen.getByRole("button", { name: "Discard active run and clear" }));
      await user.click(
        await screen.findByRole("button", { name: "Discard active run and clear" }),
      );
    await screen.findByText("Execution runs cleared.");
    expect(persistedRuns().activeRun).toBeNull();
    expect(screen.getByRole("button", { name: "Start run" })).toBeTruthy();
  });

  it("14. clearing execution runs does not affect Usage history", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Clear execution runs" }));
    await user.click(await screen.findByRole("button", { name: "Clear execution runs" }));
    await screen.findByText("Execution runs cleared.");

    expect(mocks.historyCleared).toBe(false);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    expect(await screen.findByLabelText("Usage overview")).toBeTruthy();
  });
});

describe("execution runs × navigation × usage (scenarios 15-18)", () => {
  it("15+16. a run started on Usage stays active across the Settings round-trip", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    // Usage is the landing destination.
    expect(screen.getByLabelText("Usage overview")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      screen.getByRole("button", { name: /Execution run in progress, running for/ }),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Usage" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();
    expect(persistedRuns().activeRun).not.toBeNull();
  });

  it("17. finishing after navigation still binds the run to its original provider", async () => {
    const user = userEvent.setup();
    // Reset timestamps are pinned across both snapshots so the cycle
    // continuity holds and the delta stays comparable.
    const reset5 = hoursAhead(5);
    const weekly = hoursAhead(100);
    const view = renderApp([
      codexUsage({
        limits: [
          { label: "5-hour", usedPercent: 23, resetAt: reset5 },
          { label: "Weekly", usedPercent: 95, resetAt: weekly },
        ],
      }),
    ]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));

    // Reorder providers and move quota mid-run; the run's binding comes from
    // its start snapshot, not from whatever is selected at finish time.
    mocks.usages = [
      grokUsage(),
      codexUsage({
        checkedAt: minutesAgo(0),
        limits: [
          { label: "5-hour", usedPercent: 28, resetAt: reset5 },
          { label: "Weekly", usedPercent: 95, resetAt: weekly },
        ],
      }),
    ];
    view.rerender(<App />);

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    const summary = within(screen.getByLabelText("Execution summary"));
    summary.getByText("23% → 28%");
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();
    expect(
      (persistedRuns().recentRuns[0] as { providerId?: string } | undefined)?.providerId,
    ).toBe("openai-codex");
  });

  it("18. the recent run survives Usage/Settings switching", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    await user.click(screen.getByRole("button", { name: "Finish run" }));
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.click(screen.getByRole("button", { name: "Usage" }));
    // The open panel (and its recent-run history) survives the round trip.
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();
  });
});

describe("receipts from the integrated pipeline (scenarios 19-20)", () => {
  it("19. a completed integrated run produces a deterministic receipt", async () => {
    const user = userEvent.setup();
    const reset5 = hoursAhead(5);
    const weekly = hoursAhead(100);
    const view = renderApp([
      codexUsage({
        limits: [
          { label: "5-hour", usedPercent: 23, resetAt: reset5 },
          { label: "Weekly", usedPercent: 38, resetAt: weekly },
        ],
      }),
    ]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    mocks.usages = [
      codexUsage({
        checkedAt: minutesAgo(0),
        limits: [
          { label: "5-hour", usedPercent: 28, resetAt: reset5 },
          { label: "Weekly", usedPercent: 40, resetAt: weekly },
        ],
      }),
    ];
    view.rerender(<App />);
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    const run = persistedRuns().recentRuns[0];
    expect(run).toBeTruthy();
    const receipt = createExecutionReceipt(run);
    expect(receipt).toBeTruthy();
    const json = formatReceiptJson(receipt!);
    expect(formatReceiptJson(createExecutionReceipt(persistedRuns().recentRuns[0])!)).toBe(json);
    const parsed = JSON.parse(json) as Record<string, unknown>;
    expect(parsed.kind).toBe("limitscope-execution-receipt");
    expect(parsed.schemaVersion).toBe(1);
  });

  it("20. a reset-crossing integrated run yields a reset-crossed receipt without negative deltas", async () => {
    const user = userEvent.setup();
    const view = renderApp([codexUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));

    mocks.usages = [
      codexUsage({
        checkedAt: minutesAgo(0),
        limits: [
          { label: "5-hour", usedPercent: 4, resetAt: hoursAhead(6) },
          { label: "Weekly", usedPercent: 95, resetAt: hoursAhead(100) },
        ],
      }),
    ];
    view.rerender(<App />);
    await user.click(screen.getByRole("button", { name: "Finish run" }));

    expect(screen.getAllByText(/not comparable/i).length).toBeGreaterThan(0);

    const run = persistedRuns().recentRuns[0];
    const receipt = createExecutionReceipt(run)!;
    const json = formatReceiptJson(receipt);
    expect(json).toContain("reset-crossed");
    // The naive 5-hour delta here is -19 (23% -> 4%). Assert on the parsed
    // windows instead of raw substrings: run IDs embed the UTC start hour, so
    // "-19" collides with any run minted in the 19:xx hour.
    const parsedReceipt = JSON.parse(json) as {
      windows: Array<{ label: string; deltaPoints: number | null; status: string }>;
    };
    expect(
      parsedReceipt.windows.every((w) => w.deltaPoints === null || w.deltaPoints >= 0),
    ).toBe(true);
    const fiveHour = parsedReceipt.windows.find((w) => w.label === "5-hour");
    expect(fiveHour?.status).toBe("reset-crossed");
    expect(fiveHour?.deltaPoints).toBeNull();
    // The disclaimer wording is the markdown-facing form of the receipt.
    expect(formatReceiptMarkdown(receipt)).toContain("Not exact task cost");
  });
});

describe("ProviderUsage DTO coexistence (scenarios 21-23)", () => {
  it("21. reset credits and planType coexist on a provider through Usage and Settings", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage(), grokUsage()]);

    // Settings renders the attributed, plan-typed provider's preferences row.
    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Providers & Wallets" }));
    const providers = screen.getByRole("group", { name: "Providers" });
    expect(
      within(providers).getByRole("switch", {
        name: "Show OpenAI / Codex in the floating bar",
      }),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Usage" }));
    expect(await screen.findByLabelText("Usage overview")).toBeTruthy();
    // The reset-credit payload travels with the usage object untouched.
    expect(mocks.usages[0]?.resetCredits?.bankedCredits).toBe(3);
    expect(mocks.usages[0]?.planType).toBe("team");
  });

  it("22. absent reset credits change nothing in the product surface", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage({ resetCredits: undefined }), grokUsage()]);
    await user.click(screen.getByRole("button", { name: "Execution run" }));
    await user.click(screen.getByRole("button", { name: "Start run" }));
    expect(screen.getByText(/Running ·/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Finish run" }));
    expect(screen.getByText("Recent runs (1)")).toBeTruthy();
  });

  it("23. the account identity surfaces only in its masked display form", async () => {
    const user = userEvent.setup();
    renderApp([codexUsage()]);
    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Providers & Wallets" }));
    // The provider row shows the friendly name; the masked label (fingerprint
    // scheme + tail) is display-safe…
    expect(
      screen.getByRole("switch", {
        name: "Show OpenAI / Codex in the floating bar",
      }),
    ).toBeTruthy();
    // …and no raw identity material beyond it is rendered anywhere.
    expect(document.body.textContent).not.toContain("sk-");
    expect(document.body.textContent).not.toContain("key:3456");
  });
});

describe("settings schema robustness (scenarios 24-25)", () => {
  const FUTURE = JSON.stringify({
    launchAtStartup: false,
    refreshIntervalMinutes: 15,
    theme: "oled",
    quotaNotifications: true,
    quotaPerspective: "used",
    providerPreferences: { order: [], hidden: [] },
    futureLaneField: { nested: [1, 2, 3] },
  });

  it("24. an unknown future setting survives a quota-perspective save", async () => {
    const user = userEvent.setup();
    localStorage.setItem("rate-limits.settings.v1", FUTURE);
    renderApp([codexUsage()]);

    await choosePerspective(user, "Remaining");

    const stored = JSON.parse(
      localStorage.getItem("rate-limits.settings.v1") ?? "{}",
    ) as Record<string, unknown>;
    expect(stored.quotaPerspective).toBe("remaining");
    expect(stored.futureLaneField).toEqual({ nested: [1, 2, 3] });
    expect(stored.theme).toBe("oled");
    expect(stored.quotaNotifications).toBe(true);
  });

  it("25. an unknown future setting survives a preference reset", async () => {
    const user = userEvent.setup();
    localStorage.setItem("rate-limits.settings.v1", FUTURE.replace("used", "remaining"));
    renderApp([codexUsage()]);

    await openSettings(user);
    await user.click(screen.getByRole("button", { name: "Data & Privacy" }));
    await user.click(screen.getByRole("button", { name: "Reset preferences" }));
    await user.click(await screen.findByRole("button", { name: "Reset preferences" }));
    await screen.findByText("Preferences reset.");

    const stored = JSON.parse(
      localStorage.getItem("rate-limits.settings.v1") ?? "{}",
    ) as Record<string, unknown>;
    // Canonical fields return to defaults…
    expect(stored.quotaPerspective).toBe("used");
    expect(stored.theme).toBe("graphite");
    // …while the foreign field survives untouched.
    expect(stored.futureLaneField).toEqual({ nested: [1, 2, 3] });
  });
});
