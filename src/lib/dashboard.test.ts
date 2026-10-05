import { describe, expect, it } from "vitest";
import {
  ATTENTION_CRITICAL_PERCENT,
  ATTENTION_WARN_PERCENT,
  attentionItems,
  defaultSelectedProviderId,
  providerMark,
  providerRailEntries,
  shortProviderName,
  usableQuotaWindows,
} from "./dashboard";
import type { QuotaPrediction } from "./prediction/types";
import type { ProviderUsage } from "../types";

function usage(overrides: Partial<ProviderUsage>): ProviderUsage {
  return {
    id: "zai",
    name: "Z.ai",
    status: "ok",
    health: "live",
    checkedAt: "2026-09-28T20:00:00Z",
    limits: [],
    ...overrides,
  };
}

function prediction(
  overrides: Partial<QuotaPrediction>,
): QuotaPrediction {
  return {
    providerId: "zai",
    windowLabel: "Weekly credits",
    burnRatePerHour: 2,
    projectedPercentAtReset: 96,
    confidence: "high",
    basis: {
      segmentId: "zai|Weekly credits|0",
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

const noPredictions = () => undefined;

describe("shortProviderName", () => {
  it("uses curated short names for the production registry", () => {
    expect(
      shortProviderName(usage({ id: "openai-codex", name: "OpenAI / Codex" })),
    ).toBe("Codex");
    expect(shortProviderName(usage({ id: "grok", name: "Grok (xAI)" }))).toBe("Grok");
    expect(
      shortProviderName(usage({ id: "antigravity", name: "Google Antigravity" })),
    ).toBe("Antigravity");
  });

  it("falls back to a generic rule for unknown providers", () => {
    expect(
      shortProviderName(usage({ id: "other", name: "Acme / Pro Max (EU)" })),
    ).toBe("Pro Max");
  });
});

describe("providerMark", () => {
  it("takes the short name's initial", () => {
    expect(providerMark(usage({ id: "grok", name: "Grok (xAI)" }))).toBe("G");
    expect(providerMark(usage({ id: "zai", name: "Z.ai" }))).toBe("Z");
  });
});

describe("providerRailEntries", () => {
  it("represents each provider once with its primary window", () => {
    const entries = providerRailEntries([
      usage({
        id: "openai-codex",
        name: "OpenAI / Codex",
        limits: [
          { label: "5-hour window", usedPercent: 34 },
          { label: "Weekly credits", usedPercent: 78 },
        ],
      }),
      usage({ id: "grok", name: "Grok (xAI)", limits: [{ label: "Weekly", usedPercent: 96 }] }),
      usage({ id: "grok", name: "Grok (xAI)", limits: [{ label: "Weekly", usedPercent: 96 }] }),
    ]);
    expect(entries.map((entry) => entry.usage.id)).toEqual([
      "openai-codex",
      "grok",
    ]);
    expect(entries[0].percent).toBe(78);
    expect(entries[0].primary?.label).toBe("Weekly credits");
  });

  it("reports no percent for a provider without usable windows", () => {
    const entries = providerRailEntries([
      usage({ limits: [{ label: " ", usedPercent: 50 }] }),
    ]);
    expect(entries[0].percent).toBeUndefined();
    expect(entries[0].primary).toBeUndefined();
  });
});

describe("defaultSelectedProviderId", () => {
  it("anchors on the provider with the highest usable primary percent", () => {
    expect(
      defaultSelectedProviderId([
        usage({ id: "a", limits: [{ label: "Weekly", usedPercent: 42 }] }),
        usage({ id: "b", limits: [{ label: "Weekly", usedPercent: 96 }] }),
        usage({ id: "c", limits: [{ label: "Weekly", usedPercent: 71 }] }),
      ]),
    ).toBe("b");
  });

  it("keeps registry order on ties and never averages providers", () => {
    expect(
      defaultSelectedProviderId([
        usage({ id: "a", limits: [{ label: "Weekly", usedPercent: 80 }] }),
        usage({ id: "b", limits: [{ label: "Weekly", usedPercent: 80 }] }),
      ]),
    ).toBe("a");
  });

  it("ignores simulated providers and malformed windows", () => {
    expect(
      defaultSelectedProviderId([
        usage({
          id: "mock",
          simulated: true,
          limits: [{ label: "Weekly", usedPercent: 99 }],
        }),
        usage({
          id: "real",
          limits: [
            { label: "Weekly", usedPercent: Number.NaN },
            { label: "Monthly", usedPercent: 12 },
          ],
        }),
      ]),
    ).toBe("real");
  });

  it("falls back to the first real provider, then any provider", () => {
    expect(defaultSelectedProviderId([])).toBeNull();
    expect(
      defaultSelectedProviderId([
        usage({ id: "mock", simulated: true, limits: [{ label: "W", usedPercent: 10 }] }),
      ]),
    ).toBe("mock");
    expect(defaultSelectedProviderId([usage({ id: "bare" })])).toBe("bare");
  });
});

describe("usableQuotaWindows", () => {
  it("excludes malformed windows a broken payload might carry", () => {
    const windows = usableQuotaWindows(
      usage({
        limits: [
          { label: "Weekly credits", usedPercent: 78 },
          { label: "", usedPercent: 50 },
          { label: "NaN window", usedPercent: Number.NaN },
          { label: "Negative", usedPercent: -4 },
          { label: "Over range", usedPercent: 140 },
        ],
      }),
    );
    expect(windows.map((window) => window.label)).toEqual(["Weekly credits"]);
  });
});

describe("attentionItems", () => {
  it("flags primary windows at the critical threshold", () => {
    const items = attentionItems(
      [
        usage({
          id: "grok",
          name: "Grok (xAI)",
          limits: [{ label: "Weekly", usedPercent: ATTENTION_CRITICAL_PERCENT }],
        }),
      ],
      noPredictions,
    );
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      providerId: "grok",
      providerName: "Grok",
      kind: "critical",
      line: "Critical · 95% used · Weekly",
    });
  });

  it("flags primary windows at the warn threshold", () => {
    const items = attentionItems(
      [usage({ limits: [{ label: "Weekly credits", usedPercent: ATTENTION_WARN_PERCENT }] })],
      noPredictions,
    );
    expect(items[0]?.kind).toBe("warn");
    expect(items[0]?.line).toBe("Warning · 80% used · Weekly credits");
  });

  it("derives stale and error items from runtime state", () => {
    const items = attentionItems(
      [
        usage({ id: "antigravity", name: "Google Antigravity", status: "stale", health: "stale" }),
        usage({ id: "zai", name: "Z.ai", status: "error", health: "error", error: "Z.ai usage could not be fetched." }),
      ],
      noPredictions,
    );
    expect(items).toHaveLength(2);
    expect(items.map((item) => item.kind)).toEqual(["error", "stale"]);
    expect(items.find((item) => item.providerId === "antigravity")?.line).toBe(
      "Source data stale",
    );
    expect(items.find((item) => item.providerId === "zai")?.line).toBe(
      "Refresh failed",
    );
  });

  it("names cooldown and unavailable failures honestly from the health field", () => {
    const items = attentionItems(
      [
        usage({
          id: "openai-codex",
          name: "OpenAI / Codex",
          status: "error",
          health: "cooldown",
          limits: [{ label: "Weekly credits", usedPercent: 20 }],
        }),
        usage({
          id: "grok",
          name: "Grok (xAI)",
          status: "error",
          health: "unavailable",
          limits: [],
        }),
      ],
      noPredictions,
    );
    expect(items).toHaveLength(2);
    expect(items.map((item) => item.kind)).toEqual(["error", "error"]);
    expect(items.find((item) => item.providerId === "openai-codex")?.line).toBe("Cooldown");
    expect(items.find((item) => item.providerId === "grok")?.line).toBe("Unavailable");
  });

  it("claims exhaustion only when the engine's visibility gate accepts it", () => {
    const exhaustion = prediction({
      willExhaustBeforeReset: true,
      estimatedExhaustionAt: "2026-09-29T10:00:00Z",
    });
    const items = attentionItems(
      [usage({ limits: [{ label: "Weekly credits", usedPercent: 42 }] })],
      () => exhaustion,
    );
    expect(items).toHaveLength(1);
    expect(items[0]?.kind).toBe("exhaustion");
    expect(items[0]?.line).toBe("May reach limit before reset");

    // Same prediction, but the provider is stale: the gate hides it and the
    // stale item takes the provider's single slot instead.
    const staleItems = attentionItems(
      [
        usage({
          status: "stale",
          health: "stale",
          limits: [{ label: "Weekly credits", usedPercent: 42 }],
        }),
      ],
      () => exhaustion,
    );
    expect(staleItems.map((item) => item.kind)).toEqual(["stale"]);
  });

  it("emits at most one item per provider, most severe first, and skips simulated providers", () => {
    const items = attentionItems(
      [
        usage({
          id: "a",
          simulated: true,
          status: "error",
          health: "error",
          limits: [{ label: "Weekly", usedPercent: 99 }],
        }),
        usage({
          id: "b",
          status: "stale",
          health: "stale",
          limits: [{ label: "Weekly", usedPercent: 97 }],
        }),
        usage({
          id: "c",
          status: "error",
          health: "error",
          limits: [{ label: "Weekly", usedPercent: 10 }],
        }),
        usage({ id: "d", limits: [{ label: "Weekly", usedPercent: 5 }] }),
      ],
      noPredictions,
    );
    // The simulated provider never appears; b's displayed 97% outranks its
    // staleness; c shows the refresh failure; d is healthy and silent.
    expect(items.map((item) => item.providerId)).toEqual(["b", "c"]);
    expect(items[0]?.kind).toBe("critical");
    expect(items[0]?.line).toBe("Critical · 97% used · Weekly");
    expect(items[1]?.kind).toBe("error");
  });

  it("returns nothing for healthy providers (empty state is the caller's)", () => {
    expect(
      attentionItems(
        [usage({ limits: [{ label: "Weekly", usedPercent: 42 }] })],
        noPredictions,
      ),
    ).toEqual([]);
  });
});
