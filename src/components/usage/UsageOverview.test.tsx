// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  UsageOverview,
} from "./UsageOverview";
import type {
  UsageDailyDay,
  UsageIntelligence,
  UsageIntelligenceRange,
  UsageIntelligenceLoader,
} from "../../lib/usageIntelligence";
import { localMidnightMs } from "../../lib/usageIntelligence";

/**
 * Focused presentation pins for the v0.9 Usage-first overview. The loader is
 * injected, so every number on screen is traceable to the payload below —
 * the tests assert exactly that consistency, plus the honesty states
 * (off, empty, incomplete, failed source) and the Daily activity heatmap.
 */

function model(
  name: string,
  events: number,
  input: number,
  cacheRead: number,
  cacheWrite: number,
  output: number,
  reasoning: number,
) {
  return {
    model: name,
    events,
    inputTokens: input,
    cacheReadTokens: cacheRead,
    cacheWriteTokens: cacheWrite,
    outputTokens: output,
    reasoningTokens: reasoning,
    totalTokens: input + cacheRead + cacheWrite + output + reasoning,
  };
}

function payload(
  overrides: Partial<UsageIntelligence> = {},
): UsageIntelligence {
  const zaiModels = [
    model("GLM-5.3", 96, 412_000, 3_150_000, 148_000, 530_000, 302_000),
    model("GLM-5.3-Air", 41, 120_000, 640_000, 22_000, 141_000, 34_000),
  ];
  const codexModels = [
    model("gpt-5.1-codex", 58, 980_000, 2_240_000, 96_000, 410_000, 0),
  ];
  const zaiTotal = {
    events: 137,
    inputTokens: 532_000,
    cacheReadTokens: 3_790_000,
    cacheWriteTokens: 170_000,
    outputTokens: 671_000,
    reasoningTokens: 336_000,
    totalTokens: 5_499_000,
  };
  const codexTotal = {
    events: 58,
    inputTokens: 980_000,
    cacheReadTokens: 2_240_000,
    cacheWriteTokens: 96_000,
    outputTokens: 410_000,
    reasoningTokens: 0,
    totalTokens: 3_726_000,
  };
  const totals = {
    events: 195,
    inputTokens: 1_512_000,
    cacheReadTokens: 6_030_000,
    cacheWriteTokens: 266_000,
    outputTokens: 1_081_000,
    reasoningTokens: 336_000,
    totalTokens: 9_225_000,
  };
  return {
    schemaVersion: 1,
    range: "today",
    rangeStart: "2026-10-24T00:00:00.000Z",
    rangeEnd: "2026-10-24T22:00:00.000Z",
    generatedAt: "2026-10-24T22:00:00.000Z",
    enabled: true,
    eventsInRange: totals.events,
    groups: [
      { provider: "zai", models: zaiModels, ...zaiTotal },
      { provider: "openai-codex", models: codexModels, ...codexTotal },
    ],
    totals,
    collectionStartedAt: "2026-10-15T08:00:00.000Z",
    incompleteHistory: false,
    sources: [
      {
        source: "zcode",
        state: "ok",
        watermarkAt: "2026-10-24T21:20:00.000Z",
        baselinedAt: "2026-10-15T08:00:00.000Z",
        lastObservedAt: "2026-10-24T21:48:00.000Z",
        lastScan: { accepted: 12, duplicates: 0, rejected: 0 },
      },
      {
        source: "opencode",
        state: "ok",
        watermarkAt: "2026-10-24T21:20:00.000Z",
        baselinedAt: "2026-10-15T08:00:00.000Z",
        lastObservedAt: "2026-10-24T20:00:00.000Z",
        lastScan: { accepted: 5, duplicates: 0, rejected: 1, usageNotReported: 23 },
      },
    ],
    semantics: "reportedTokenUsageCollectedLocally",
    daily: dailyDefault(),
    ...overrides,
  };
}

function loaderWith(
  respond: (range: UsageIntelligenceRange) => UsageIntelligence,
): UsageIntelligenceLoader {
  return vi.fn(async (query) => respond(query.range));
}

function renderOverview(
  loader: UsageIntelligenceLoader,
  enabled = true,
  onOpenSettings = () => {},
) {
  return render(
    <UsageOverview
      enabled={enabled}
      revision={0}
      loader={loader}
      onOpenSettings={onOpenSettings}
    />,
  );
}

/** The component's own date wording, computed the same way (locale-stable). */
function sinceDate(iso: string) {
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  }).format(new Date(iso));
}

const STARTED_AT = "2026-10-15T08:00:00.000Z";

function dailyDay(overrides: Partial<UsageDailyDay>): UsageDailyDay {
  return {
    date: "2026-10-24",
    observed: true,
    events: 3,
    totalTokens: 1_000,
    beforeCollection: false,
    ...overrides,
  };
}

/** A small, honest default projection: four days, one missing. */
function dailyDefault(): UsageDailyDay[] {
  return [
    dailyDay({ date: "2026-10-21", totalTokens: 2_000_000, events: 20 }),
    dailyDay({ date: "2026-10-22", totalTokens: 4_000_000, events: 40 }),
    dailyDay({
      date: "2026-10-23",
      observed: false,
      events: 0,
      totalTokens: undefined,
    }),
    dailyDay({ date: "2026-10-24", totalTokens: 1_000_000, events: 10 }),
  ];
}

describe("UsageOverview", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });
  afterEach(cleanup);

  it("renders KPI totals exactly as the payload reports them", async () => {
    renderOverview(loaderWith(() => payload()));
    const kpis = await screen.findByLabelText("Usage summary");
    // 9_225_000 tokens format as 9.2M; events and model count verbatim.
    expect(within(kpis).getByText("9.2M")).toBeTruthy();
    expect(within(kpis).getByText("195")).toBeTruthy();
    expect(within(kpis).getByText("3")).toBeTruthy();
    // Events are named as recorded completions, not API requests.
    expect(
      within(kpis).getByText(/recorded completions — not every API request/),
    ).toBeTruthy();
    // Observed usage is explicitly not total usage.
    expect(
      within(kpis).getByText(/observed as reported — not your total usage/),
    ).toBeTruthy();
    expect(within(kpis).getByText(`since ${sinceDate(STARTED_AT)}`)).toBeTruthy();
    expect(within(kpis).getByText(/covers the Today range/)).toBeTruthy();
  });

  it("counts distinct model identities, not provider-qualified rows", async () => {
    // The same normalized identity (`gpt-5.4`) observed under two providers
    // (bare Codex vs OpenCode's separate provider axis) is one distinct
    // model — while both provider-qualified rows stay in the ranking.
    const crossProvider = payload({
      groups: [
        {
          provider: "openai-codex",
          models: [model("gpt-5.4", 10, 1000, 0, 0, 500, 0)],
          events: 10,
          inputTokens: 1000,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          outputTokens: 500,
          reasoningTokens: 0,
          totalTokens: 1500,
        },
        {
          provider: "openai",
          models: [
            model("gpt-5.4", 5, 2000, 0, 0, 1000, 0),
            model("gpt-5.6-sol", 2, 100, 0, 0, 50, 0),
          ],
          events: 7,
          inputTokens: 2100,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          outputTokens: 1050,
          reasoningTokens: 0,
          totalTokens: 3150,
        },
      ],
    });
    renderOverview(loaderWith(() => crossProvider));
    const kpis = await screen.findByLabelText("Usage summary");
    // Three provider-qualified rows, two distinct identities.
    expect(within(kpis).getByText("2")).toBeTruthy();
    expect(
      within(kpis).getByText("distinct models across providers"),
    ).toBeTruthy();
    const models = screen.getByLabelText("Models");
    expect(within(models).getAllByRole("button")).toHaveLength(3);
  });

  it("ranks models and sources with totals consistent with the payload", async () => {
    renderOverview(loaderWith(() => payload()));
    const models = await screen.findByLabelText("Models");
    const modelValues = within(models).getAllByRole("button");
    // Heaviest first: GLM-5.3, gpt-5.1-codex, GLM-5.3-Air.
    expect(modelValues.map((row) => row.textContent)).toEqual([
      "GLM-5.34.5M",
      "gpt-5.1-codex3.7M",
      "GLM-5.3-Air957K",
    ]);

    const sources = screen.getByLabelText("Usage sources");
    const sourceRows = within(sources).getAllByRole("listitem");
    expect(sourceRows[0].textContent).toContain("Z.ai (ZCode)");
    expect(sourceRows[0].textContent).toContain("5.5M");
    expect(sourceRows[0].textContent).toContain("2 models");
    expect(sourceRows[0].textContent).toContain("137 events");
    expect(sourceRows[1].textContent).toContain("OpenAI / Codex");
    expect(sourceRows[1].textContent).toContain("3.7M");
  });

  it("keeps a model row's dimensions hidden until it is expanded", async () => {
    const user = userEvent.setup();
    renderOverview(loaderWith(() => payload()));
    const models = await screen.findByLabelText("Models");
    expect(within(models).queryByText("Reasoning")).toBeNull();

    const top = within(models).getAllByRole("button")[0];
    expect(top.getAttribute("aria-expanded")).toBe("false");
    await user.click(top);
    expect(top.getAttribute("aria-expanded")).toBe("true");
    const detail = within(models).getAllByRole("button")[0].parentElement!;
    // The five payload dimensions for GLM-5.3 (cached = read + write).
    expect(within(detail).getByText("Input")).toBeTruthy();
    expect(within(detail).getByText("412K")).toBeTruthy();
    expect(within(detail).getByText("3.3M")).toBeTruthy(); // cached
    expect(within(detail).getByText("530K")).toBeTruthy(); // output
    expect(within(detail).getByText("302K")).toBeTruthy(); // reasoning
    expect(within(detail).getByText(/96 recorded events/)).toBeTruthy();
    expect(within(detail).getByText(/Z\.ai \(ZCode\)/)).toBeTruthy();

    await user.click(top);
    expect(top.getAttribute("aria-expanded")).toBe("false");
  });

  it("keeps the default ranking compact and reveals the rest on demand", async () => {
    const user = userEvent.setup();
    const many = payload();
    many.groups = [
      {
        ...many.groups[0],
        models: Array.from({ length: 9 }, (_, index) =>
          model(`model-${index + 1}`, 1, 1000 * (10 - index), 0, 0, 0, 0),
        ),
      },
    ];
    renderOverview(loaderWith(() => many));
    const models = await screen.findByLabelText("Models");
    const modelButtons = () =>
      within(models)
        .getAllByRole("button")
        .filter((button) => button.hasAttribute("aria-expanded")).length;
    expect(modelButtons()).toBe(6);
    const more = within(models).getByRole("button", { name: /Show 3 more/ });
    await user.click(more);
    expect(modelButtons()).toBe(9);
    expect(
      within(models).queryByRole("button", { name: /Show \d+ more/ }),
    ).toBeNull();
  });

  it("discloses incomplete history when the range predates collection", async () => {
    renderOverview(
      loaderWith((range) => payload({ range, incompleteHistory: true })),
    );
    fireEvent.click(screen.getByRole("button", { name: "7d" }));
    expect(
      await screen.findByText(/— less history than the 7 days range/),
    ).toBeTruthy();
    expect(
      screen.getByText(/range reaches before collection started/),
    ).toBeTruthy();
  });

  it("keeps failed sources and unreported usage visible", async () => {
    const partial = payload({
      sources: [
        {
          source: "codex",
          state: "readFailure",
          watermarkAt: "2026-10-24T21:20:00.000Z",
          baselinedAt: "2026-10-15T08:00:00.000Z",
          lastObservedAt: "2026-10-23T20:00:00.000Z",
          detail: "history file locked by another process",
        },
        {
          source: "opencode",
          state: "ok",
          watermarkAt: "2026-10-24T21:20:00.000Z",
          baselinedAt: "2026-10-15T08:00:00.000Z",
          lastScan: { accepted: 5, duplicates: 0, rejected: 0, usageNotReported: 23 },
        },
      ],
    });
    renderOverview(loaderWith(() => partial));
    const sources = await screen.findByLabelText("Usage sources");
    const list = within(sources).getByLabelText("Collection sources");
    const rows = within(list).getAllByRole("listitem");
    expect(rows[0].textContent).toContain("Codex: could not read source");
    expect(rows[0].querySelector(".status-error")).toBeTruthy();
    expect(rows[1].textContent).toContain(
      "23 completions reported no usage",
    );
    expect(rows[1].querySelector(".status-ok")).toBeTruthy();
  });

  it("states the collection start on the empty view instead of implying a measured-empty range", async () => {
    renderOverview(
      loaderWith(() =>
        payload({
          groups: [],
          totals: {
            events: 0,
            inputTokens: 0,
            cacheReadTokens: 0,
            cacheWriteTokens: 0,
            outputTokens: 0,
            reasoningTokens: 0,
            totalTokens: 0,
          },
        }),
      ),
    );
    expect(
      await screen.findByText("No locally collected token usage yet."),
    ).toBeTruthy();
    expect(
      screen.getByText(new RegExp(`Collection started ${sinceDate(STARTED_AT)}`)),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Usage summary")).toBeNull();
  });

  it("never queries while the opt-in is off and routes to Settings", () => {
    const loader = vi.fn();
    const onOpenSettings = vi.fn();
    renderOverview(loader, false, onOpenSettings);
    expect(loader).not.toHaveBeenCalled();
    expect(screen.getByText("Usage Intelligence is off.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("isolates a failed first load with a retry that can recover", async () => {
    const loader = vi
      .fn()
      .mockRejectedValueOnce(new Error("store unavailable"))
      .mockResolvedValueOnce(payload());
    renderOverview(loader);
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/store unavailable/)).toBeTruthy();
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(await screen.findByLabelText("Usage summary")).toBeTruthy();
  });

  it("renders the Daily activity heatmap from the payload's daily projection", async () => {
    renderOverview(loaderWith(() => payload()));
    const grid = await screen.findByRole("list", {
      name: "Daily reported token activity",
    });
    const cells = within(grid).getAllByRole("listitem");
    expect(cells).toHaveLength(4);
    // Intensity scales to the busiest day of the payload (4.0M).
    expect(cells[0].className).toContain("usage-daily-l2"); // 2.0M = 50%
    expect(cells[1].className).toContain("usage-daily-l4"); // 4.0M = peak
    expect(cells[2].className).toContain("usage-daily-cell-none");
    expect(cells[3].className).toContain("usage-daily-l1"); // 1.0M = 25%
    // The missing day is labeled missing — never a zero.
    expect(cells[2].getAttribute("aria-label")).toContain(
      "no recorded events — not confirmed zero usage",
    );
    // The placeholder is gone for good.
    expect(screen.queryByText("Daily token activity will chart here")).toBeNull();
  });

  it("re-renders the heatmap when the range switches to a different projection", async () => {
    const thirtyDays = [
      dailyDay({ date: "2026-09-25", totalTokens: 900_000, events: 9 }),
      ...Array.from({ length: 28 }, (_, index) =>
        dailyDay({
          date: `2026-10-${String(index + 1).padStart(2, "0")}`,
          observed: false,
          events: 0,
          totalTokens: undefined,
          beforeCollection: index < 10,
        }),
      ),
      dailyDay({ date: "2026-10-29", totalTokens: 500_000, events: 5 }),
    ];
    const loader = vi.fn(async (query: { range: UsageIntelligenceRange }) =>
      payload({ range: query.range, daily: query.range === "30d" ? thirtyDays : dailyDefault() }),
    );
    renderOverview(loader);
    await screen.findByRole("list", { name: "Daily reported token activity" });
    fireEvent.click(screen.getByRole("button", { name: "30d" }));
    const grid = await screen.findByRole("list", {
      name: "Daily reported token activity",
    });
    expect(within(grid).getAllByRole("listitem")).toHaveLength(30);
    const preCollection = within(grid)
      .getAllByRole("listitem")
      .filter((cell) =>
        (cell.getAttribute("aria-label") ?? "").includes(
          "before local collection started",
        ),
      );
    expect(preCollection).toHaveLength(10);
  });

  it("shows no Daily activity at all while the opt-in is off", () => {
    const loader = vi.fn();
    renderOverview(loader, false);
    expect(loader).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Daily activity")).toBeNull();
    expect(
      screen.queryByRole("list", { name: "Daily reported token activity" }),
    ).toBeNull();
  });

  it("re-queries with the local midnight anchor when the range changes", async () => {
    const loader = loaderWith((range) => payload({ range }));
    renderOverview(loader);
    await screen.findByLabelText("Usage summary");
    const calls = vi.mocked(loader).mock.calls;
    expect(calls[0][0].range).toBe("today");
    expect(typeof calls[0][0].todayStartMs).toBe("number");

    fireEvent.click(screen.getByRole("button", { name: "30d" }));
    await vi.waitFor(() => {
      const last = calls[calls.length - 1][0];
      expect(last).toEqual({ range: "30d" });
    });
  });

  it("re-anchors the Today query when the local day rolls over while the app stays open", async () => {
    const loader = loaderWith((range) => payload({ range }));
    // Local (no Z suffix) timestamps keep the test timezone-agnostic.
    const beforeMidnight = new Date("2026-10-24T23:59:00");
    const afterMidnight = new Date("2026-10-25T00:01:00");
    const view = render(
      <UsageOverview
        enabled
        revision={0}
        loader={loader}
        nowMs={beforeMidnight.getTime()}
      />,
    );
    await screen.findByLabelText("Usage summary");
    const calls = vi.mocked(loader).mock.calls;
    expect(calls[0][0].todayStartMs).toBe(localMidnightMs(beforeMidnight));

    // The app's existing wall-clock tick crosses local midnight.
    view.rerender(
      <UsageOverview
        enabled
        revision={0}
        loader={loader}
        nowMs={afterMidnight.getTime()}
      />,
    );
    await vi.waitFor(() => {
      const last = calls[calls.length - 1][0];
      expect(last.range).toBe("today");
      expect(last.todayStartMs).toBe(localMidnightMs(afterMidnight));
    });
    // Exactly one re-query: the rollover itself, no per-tick refetches.
    expect(calls).toHaveLength(2);
  });

  it("keeps the Today anchor stable across same-day clock ticks", async () => {
    const loader = loaderWith((range) => payload({ range }));
    const noon = new Date("2026-10-24T12:00:00").getTime();
    const view = render(
      <UsageOverview enabled revision={0} loader={loader} nowMs={noon} />,
    );
    await screen.findByLabelText("Usage summary");

    view.rerender(
      <UsageOverview
        enabled
        revision={0}
        loader={loader}
        nowMs={noon + 60_000}
      />,
    );
    view.rerender(
      <UsageOverview
        enabled
        revision={0}
        loader={loader}
        nowMs={noon + 3_600_000}
      />,
    );
    // Same local day: the query key never changes, so no re-query fires.
    expect(vi.mocked(loader).mock.calls).toHaveLength(1);
  });
});
