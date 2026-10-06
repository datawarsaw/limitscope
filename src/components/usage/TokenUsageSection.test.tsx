// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TokenUsageSection } from "./TokenUsageSection";
import type {
  UsageIntelligence,
  UsageIntelligenceLoader,
  UsageIntelligenceQuery,
} from "../../lib/usageIntelligence";

function intelligence(
  overrides: Partial<UsageIntelligence> = {},
): UsageIntelligence {
  return {
    schemaVersion: 1,
    range: "today",
    rangeStart: "2026-10-06T00:00:00.000Z",
    rangeEnd: "2026-10-06T12:00:00.000Z",
    generatedAt: "2026-10-06T12:00:00.000Z",
    enabled: true,
    eventsInRange: 0,
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
    collectionStartedAt: "2026-10-04T09:30:00.000Z",
    incompleteHistory: false,
    sources: [],
    semantics: "reportedTokenUsageCollectedLocally",
    ...overrides,
  };
}

function withData(): UsageIntelligence {
  return intelligence({
    eventsInRange: 2,
    groups: [
      {
        provider: "zai",
        events: 2,
        inputTokens: 11_890,
        cacheReadTokens: 104_000,
        cacheWriteTokens: 0,
        outputTokens: 8_187,
        reasoningTokens: 0,
        totalTokens: 124_077,
        models: [
          {
            model: "GLM-5.3",
            events: 1,
            inputTokens: 7_089,
            cacheReadTokens: 39_040,
            cacheWriteTokens: 0,
            outputTokens: 1_377,
            reasoningTokens: 0,
            totalTokens: 47_506,
          },
          {
            model: "GLM-5.3-Flash",
            events: 1,
            inputTokens: 4_801,
            cacheReadTokens: 64_960,
            cacheWriteTokens: 0,
            outputTokens: 6_810,
            reasoningTokens: 0,
            totalTokens: 76_571,
          },
        ],
      },
    ],
    totals: {
      events: 2,
      inputTokens: 11_890,
      cacheReadTokens: 104_000,
      cacheWriteTokens: 0,
      outputTokens: 8_187,
      reasoningTokens: 0,
      totalTokens: 124_077,
    },
    sources: [
      {
        source: "zcode",
        state: "ok",
        watermarkAt: "2026-10-06T11:59:00.000Z",
        baselinedAt: "2026-10-04T09:30:00.000Z",
        lastObservedAt: "2026-10-06T12:00:00.000Z",
      },
    ],
  });
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
});

/**
 * Pins the locale that `Intl.DateTimeFormat(undefined, …)` resolves to for
 * the duration of one test (and pins the zone to UTC, so the fixture
 * timestamps render the same day in any host timezone). The product
 * defers to the ambient locale, so the exact-string disclosure assertions
 * must hold under both a day-first locale (en-GB) and a month-first one
 * (en-US, the CI environment).
 */
function pinDateTimeFormat(locale: string): void {
  const PinnedDateTimeFormat = Intl.DateTimeFormat;
  vi.spyOn(Intl, "DateTimeFormat").mockImplementation(
    ((_ignoredLocale: unknown, options?: Intl.DateTimeFormatOptions) =>
      new PinnedDateTimeFormat(locale, {
        ...options,
        timeZone: "UTC",
      })) as unknown as typeof Intl.DateTimeFormat,
  );
}

describe("token usage section", () => {
  it("renders the off explainer and never queries while disabled", () => {
    const loader = vi.fn();
    render(<TokenUsageSection enabled={false} revision={null} loader={loader} />);
    expect(
      screen.getByText("Reported token usage collected locally is off."),
    ).toBeTruthy();
    expect(
      screen.getByText(/never reads prompt or response content/i),
    ).toBeTruthy();
    expect(loader).not.toHaveBeenCalled();
  });

  it("renders the empty/no-data state with the collection start note", async () => {
    const loader = vi.fn(async () => intelligence());
    render(<TokenUsageSection enabled={true} revision={0} loader={loader} />);
    expect(
      await screen.findByText("No locally collected token usage yet."),
    ).toBeTruthy();
    // Locale-independent: the product defers to the ambient locale, and
    // CI (en-US) renders month-first, so only the year is asserted here;
    // the exact day-first and month-first strings are pinned below.
    expect(screen.getByText(/Collection started .*2026/i)).toBeTruthy();
    expect(screen.getByText(/new requests appear here as they complete/i)).toBeTruthy();
  });

  it("switches Today / 7d / 30d and queries the matching range", async () => {
    const user = userEvent.setup();
    const loader = vi.fn(async (query: UsageIntelligenceQuery) =>
      intelligence({ range: query.range }),
    );
    render(<TokenUsageSection enabled={true} revision={0} loader={loader} />);
    await screen.findByRole("group", { name: "Token usage range" });

    const today = screen.getAllByRole("button", { name: "Today" })[0];
    const seven = screen.getAllByRole("button", { name: "7d" })[0];
    const thirty = screen.getAllByRole("button", { name: "30d" })[0];
    expect(today.getAttribute("aria-pressed")).toBe("true");
    expect(seven.getAttribute("aria-pressed")).toBe("false");

    await user.click(seven);
    expect(seven.getAttribute("aria-pressed")).toBe("true");
    expect(today.getAttribute("aria-pressed")).toBe("false");
    const sevenQuery = loader.mock.calls.at(-1)![0] as UsageIntelligenceQuery;
    expect(sevenQuery.range).toBe("7d");
    expect(sevenQuery.todayStartMs).toBeUndefined();

    await user.click(thirty);
    expect(thirty.getAttribute("aria-pressed")).toBe("true");
    expect(loader.mock.calls.at(-1)![0].range).toBe("30d");
  });

  it("asks for the local midnight boundary on the Today range", async () => {
    const loader = vi.fn(async (_query: UsageIntelligenceQuery) => intelligence());
    render(<TokenUsageSection enabled={true} revision={0} loader={loader} />);
    await screen.findByRole("group", { name: "Token usage range" });
    const query = loader.mock.calls[0][0] as UsageIntelligenceQuery;
    expect(query.range).toBe("today");
    expect(typeof query.todayStartMs).toBe("number");
    const midnight = new Date(query.todayStartMs!);
    expect(midnight.getHours()).toBe(0);
    expect(midnight.getMinutes()).toBe(0);
    expect(midnight.getSeconds()).toBe(0);
  });

  it("renders provider groups, models, and the token breakdown", async () => {
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => withData())}
      />,
    );
    const table = await screen.findByRole("table", {
      name: /Token usage by provider and model \(Today\)/,
    });
    expect(within(table).getByText("Z.ai (ZCode)")).toBeTruthy();
    expect(within(table).getByText("GLM-5.3")).toBeTruthy();
    expect(within(table).getByText("GLM-5.3-Flash")).toBeTruthy();
    // GLM-5.3 row: total 47.5K, input 7.1K (non-cached), cached 39K, output 1.4K.
    expect(within(table).getByText("47.5K")).toBeTruthy();
    expect(within(table).getByText("7089")).toBeTruthy();
    expect(within(table).getByText("39K")).toBeTruthy();
    expect(within(table).getByText(/^1377$/)).toBeTruthy();
    // Column headers name the breakdown.
    for (const header of ["Total", "Input", "Cached", "Output", "Reasoning"]) {
      expect(within(table).getAllByText(header).length).toBeGreaterThan(0);
    }
  });

  it("renders Codex usage as one more provider group in the same table", async () => {
    const data = intelligence({
      eventsInRange: 3,
      groups: [
        ...withData().groups,
        {
          provider: "openai-codex",
          events: 1,
          inputTokens: 7089,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          outputTokens: 1377,
          reasoningTokens: 0,
          totalTokens: 8466,
          models: [
            {
              model: "gpt-5.4",
              events: 1,
              inputTokens: 7089,
              cacheReadTokens: 0,
              cacheWriteTokens: 0,
              outputTokens: 1377,
              reasoningTokens: 0,
              totalTokens: 8466,
            },
          ],
        },
        {
          provider: "xai",
          events: 1,
          inputTokens: 100,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          outputTokens: 50,
          reasoningTokens: 0,
          totalTokens: 150,
          models: [
            {
              model: "grok-4.6",
              events: 1,
              inputTokens: 100,
              cacheReadTokens: 0,
              cacheWriteTokens: 0,
              outputTokens: 50,
              reasoningTokens: 0,
              totalTokens: 150,
            },
          ],
        },
      ],
    });
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => data)}
      />,
    );
    const table = await screen.findByRole("table");
    // No new view, no Codex-specific card: the same table gains groups.
    expect(within(table).getByText("Z.ai (ZCode)")).toBeTruthy();
    expect(within(table).getByText("OpenAI / Codex")).toBeTruthy();
    expect(within(table).getByText("grok-4.6")).toBeTruthy();
    expect(within(table).getByText("gpt-5.4")).toBeTruthy();
  });

  it("discloses when collection started and when the range out-reaches it", async () => {
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => withData())}
      />,
    );
    expect(
      await screen.findByText(/Reported token usage collected locally since .*2026/i),
    ).toBeTruthy();

    cleanup();
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () =>
          withData(),
        )}
      />,
    );
    // (complete-history case covered by the absence assertion below)
    expect(
      screen.queryByText(/less history than the Today range/i),
    ).toBeNull();

    cleanup();
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () =>
          intelligence({ ...withData(), incompleteHistory: true }),
        )}
      />,
    );
    expect(
      await screen.findByText(/less history than the Today range/i),
    ).toBeTruthy();
  });

  it("renders the collection-start date day-first under a pinned en-GB locale", async () => {
    pinDateTimeFormat("en-GB");
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => withData())}
      />,
    );
    expect(
      await screen.findByText("Reported token usage collected locally since 4 Oct 2026"),
    ).toBeTruthy();
    cleanup();
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => intelligence())}
      />,
    );
    expect(
      await screen.findByText(
        "Collection started 4 Oct 2026; new requests appear here as they complete.",
      ),
    ).toBeTruthy();
  });

  it("renders the collection-start date month-first under a pinned en-US locale", async () => {
    // The CI environment (en-US) renders month-first; the same disclosures
    // must pass there without a day-first assumption.
    pinDateTimeFormat("en-US");
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => withData())}
      />,
    );
    expect(
      await screen.findByText("Reported token usage collected locally since Oct 4, 2026"),
    ).toBeTruthy();
    cleanup();
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => intelligence())}
      />,
    );
    expect(
      await screen.findByText(
        "Collection started Oct 4, 2026; new requests appear here as they complete.",
      ),
    ).toBeTruthy();
  });

  it("shows only the scoped provider when the view is narrowed", async () => {
    const data = intelligence({
      groups: [
        ...withData().groups,
        {
          provider: "openai-codex",
          events: 1,
          inputTokens: 1,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          outputTokens: 1,
          reasoningTokens: 0,
          totalTokens: 2,
          models: [
            {
              model: "gpt-5.4",
              events: 1,
              inputTokens: 1,
              cacheReadTokens: 0,
              cacheWriteTokens: 0,
              outputTokens: 1,
              reasoningTokens: 0,
              totalTokens: 2,
            },
          ],
        },
      ],
    });
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        scopeProviderId="zai"
        loader={vi.fn(async () => data)}
      />,
    );
    const table = await screen.findByRole("table");
    expect(within(table).getByText("Z.ai (ZCode)")).toBeTruthy();
    expect(within(table).queryByText("openai-codex")).toBeNull();
  });

  it("keeps the failure inside the section with a retry", async () => {
    const user = userEvent.setup();
    let calls = 0;
    const loader: UsageIntelligenceLoader = vi.fn(async () => {
      calls += 1;
      if (calls === 1) throw new Error("boom");
      return intelligence();
    });
    render(<TokenUsageSection enabled={true} revision={0} loader={loader} />);
    expect(await screen.findByText(/Token usage could not be loaded/i)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No locally collected token usage yet.")).toBeTruthy();
  });

  it("renders source diagnostics without cluttering the table", async () => {
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => withData())}
      />,
    );
    expect(await screen.findByText(/ZCode: collecting · last observed/i)).toBeTruthy();
  });

  it("reports each source's diagnostics independently", async () => {
    const data = intelligence({
      ...withData(),
      sources: [
        {
          source: "zcode",
          state: "ok",
          watermarkAt: "2026-10-06T11:55:00.000Z",
          baselinedAt: "2026-10-04T09:30:00.000Z",
          lastObservedAt: "2026-10-06T11:55:00.000Z",
        },
        {
          source: "codex",
          state: "sourceAbsent",
          watermarkAt: "1970-01-01T00:00:00.000Z",
          baselinedAt: "1970-01-01T00:00:00.000Z",
          detail: "the Codex home has no rollout trees",
        },
      ],
    });
    render(
      <TokenUsageSection
        enabled={true}
        revision={0}
        loader={vi.fn(async () => data)}
      />,
    );
    const sources = await screen.findByText(/ZCode: collecting · last observed/);
    expect(sources.textContent).toContain("Codex: source not found");
  });
});
