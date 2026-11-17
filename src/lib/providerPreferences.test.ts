import { describe, expect, it } from "vitest";
import {
  DEFAULT_PROVIDER_ORDER,
  canHideProvider,
  effectiveProviderOrder,
  fullDisplayOrder,
  moveProviderOrder,
  orderVisibleProviders,
  resolveProviderPreferences,
  sanitizeProviderPreferences,
  toggleProviderHidden,
} from "./providerPreferences";

const REGISTRY = ["openai-codex", "zai", "opencode-go", "antigravity", "grok"];

// The production registry, in the order the runtime registers it.
const PRODUCTION_REGISTRY = [
  "openai-codex",
  "openai-codex:secondary",
  "antigravity",
  "zai",
  "opencode-go",
  "grok",
  "qwen-token-plan",
  "minimax-token-plan",
  "opendesign",
];

describe("sanitizeProviderPreferences", () => {
  it("defaults missing preferences to empty order and hidden", () => {
    expect(sanitizeProviderPreferences(undefined)).toEqual({ order: [], hidden: [] });
    expect(sanitizeProviderPreferences(null)).toEqual({ order: [], hidden: [] });
    expect(sanitizeProviderPreferences({})).toEqual({ order: [], hidden: [] });
  });

  it("ignores malformed saved provider ids", () => {
    expect(
      sanitizeProviderPreferences({ order: ["zai", 42, "", "  ", null], hidden: ["grok", false] }),
    ).toEqual({ order: ["zai"], hidden: ["grok"] });
  });

  it("dedupes repeated ids keeping the first position", () => {
    expect(
      sanitizeProviderPreferences({ order: ["zai", "grok", "zai"], hidden: [] }),
    ).toEqual({ order: ["zai", "grok"], hidden: [] });
  });
});

describe("resolveProviderPreferences", () => {
  it("defaults to the curated default order with nothing hidden", () => {
    expect(resolveProviderPreferences(REGISTRY, undefined)).toEqual({
      visibleOrderedIds: ["openai-codex", "antigravity", "zai", "opencode-go", "grok"],
      hiddenIds: [],
    });
  });

  it("leads with the default order for the production registry", () => {
    expect(resolveProviderPreferences(PRODUCTION_REGISTRY, undefined)).toEqual({
      visibleOrderedIds: [...PRODUCTION_REGISTRY],
      hiddenIds: [],
    });
  });

  it("applies a persisted custom order", () => {
    const resolved = resolveProviderPreferences(REGISTRY, {
      order: ["grok", "openai-codex", "zai", "opencode-go", "antigravity"],
      hidden: [],
    });
    expect(resolved.visibleOrderedIds).toEqual([
      "grok",
      "openai-codex",
      "zai",
      "opencode-go",
      "antigravity",
    ]);
  });

  it("removes hidden providers from the visible order", () => {
    const resolved = resolveProviderPreferences(REGISTRY, {
      order: [],
      hidden: ["zai", "grok"],
    });
    expect(resolved.visibleOrderedIds).toEqual([
      "openai-codex",
      "antigravity",
      "opencode-go",
    ]);
    expect(resolved.hiddenIds).toEqual(["zai", "grok"]);
  });

  it("appends a newly registered provider deterministically", () => {
    const resolved = resolveProviderPreferences(REGISTRY, {
      order: ["openai-codex", "zai"],
      hidden: [],
    });
    expect(resolved.visibleOrderedIds).toEqual([
      "openai-codex",
      "zai",
      "opencode-go",
      "antigravity",
      "grok",
    ]);
  });

  it("ignores configured providers that no longer exist", () => {
    const resolved = resolveProviderPreferences(["a", "c", "d"], {
      order: ["a", "b", "c"],
      hidden: ["b"],
    });
    expect(resolved.visibleOrderedIds).toEqual(["a", "c", "d"]);
    expect(resolved.hiddenIds).toEqual([]);
  });

  it("ignores malformed saved provider ids", () => {
    const resolved = resolveProviderPreferences(REGISTRY, {
      order: ["grok", 7, ""],
      hidden: [null],
    });
    expect(resolved.visibleOrderedIds[0]).toBe("grok");
    expect(resolved.visibleOrderedIds).toHaveLength(REGISTRY.length);
  });
});

describe("orderVisibleProviders", () => {
  const item = (id: string) => ({ id, payload: "x-" + id });

  it("orders items by preference and drops hidden ones", () => {
    const items = REGISTRY.map(item);
    const visible = orderVisibleProviders(items, (e) => e.id, REGISTRY, {
      order: ["grok", "openai-codex", "zai", "opencode-go", "antigravity"],
      hidden: ["zai"],
    });
    expect(visible.map((e) => e.id)).toEqual(["grok", "openai-codex", "opencode-go", "antigravity"]);
  });

  it("preserves item identity so history context is untouched", () => {
    const items = REGISTRY.map(item);
    const visible = orderVisibleProviders(items, (e) => e.id, REGISTRY, undefined);
    expect(visible).toHaveLength(REGISTRY.length);
    for (const entry of visible) {
      expect(items.includes(entry)).toBe(true);
    }
    const reshown = orderVisibleProviders(items, (e) => e.id, REGISTRY, {
      order: [],
      hidden: ["grok"],
    });
    expect(reshown.map((e) => e.id)).not.toContain("grok");
    const grok = items.find((e) => e.id === "grok")!;
    const restored = orderVisibleProviders(items, (e) => e.id, REGISTRY, undefined);
    expect(restored.includes(grok)).toBe(true);
  });
});

describe("fullDisplayOrder", () => {
  it("lists every registered provider with hidden ones in place", () => {
    expect(
      fullDisplayOrder(REGISTRY, { order: ["grok", "zai"], hidden: ["zai"] }),
    ).toEqual(["grok", "zai", "openai-codex", "opencode-go", "antigravity"]);
  });
});

describe("moveProviderOrder", () => {
  const saved = { order: ["a", "b", "c"], hidden: [] };

  it("moves a provider up", () => {
    expect(moveProviderOrder(["a", "b", "c"], saved, "b", -1)).toEqual(["b", "a", "c"]);
  });

  it("moves a provider down", () => {
    expect(moveProviderOrder(["a", "b", "c"], saved, "b", 1)).toEqual(["a", "c", "b"]);
  });

  it("keeps the first provider on move up and the last on move down", () => {
    expect(moveProviderOrder(["a", "b", "c"], saved, "a", -1)).toEqual(["a", "b", "c"]);
    expect(moveProviderOrder(["a", "b", "c"], saved, "c", 1)).toEqual(["a", "b", "c"]);
  });

  it("ignores unknown providers", () => {
    expect(moveProviderOrder(["a", "b"], saved, "zzz", 1)).toEqual(["a", "b"]);
  });
});

describe("canHideProvider / toggleProviderHidden", () => {
  it("cannot hide the final visible provider", () => {
    expect(canHideProvider(["a"], undefined, "a")).toBe(false);
    expect(canHideProvider(["a", "b"], { order: [], hidden: ["b"] }, "a")).toBe(false);
    const refused = toggleProviderHidden(["a", "b"], { order: [], hidden: ["b"] }, "a", true);
    expect(refused.applied).toBe(false);
    expect(refused.prefs.hidden).toEqual(["b"]);
  });

  it("hides a provider while others stay visible", () => {
    const result = toggleProviderHidden(REGISTRY, undefined, "zai", true);
    expect(result.applied).toBe(true);
    expect(result.prefs.hidden).toEqual(["zai"]);
    expect(resolveProviderPreferences(REGISTRY, result.prefs).visibleOrderedIds).not.toContain("zai");
  });

  it("re-showing restores the existing preference context", () => {
    const hidden = toggleProviderHidden(REGISTRY, { order: ["grok", "zai"], hidden: [] }, "zai", true);
    const shown = toggleProviderHidden(REGISTRY, hidden.prefs, "zai", false);
    expect(shown.applied).toBe(true);
    expect(shown.prefs.hidden).toEqual([]);
    expect(resolveProviderPreferences(REGISTRY, shown.prefs).visibleOrderedIds).toContain("zai");
  });

  it("refuses unknown provider ids", () => {
    expect(toggleProviderHidden(REGISTRY, undefined, "nope", true).applied).toBe(false);
  });
});

describe("the default provider order", () => {
  it("is the confirmed nine-provider order with both Codex accounts first", () => {
    expect(DEFAULT_PROVIDER_ORDER).toEqual([
      "openai-codex",
      "openai-codex:secondary",
      "antigravity",
      "zai",
      "opencode-go",
      "grok",
      "qwen-token-plan",
      "minimax-token-plan",
      "opendesign",
    ]);
  });

  it("applies to a configuration that has no order of its own", () => {
    expect(effectiveProviderOrder(PRODUCTION_REGISTRY, undefined)).toEqual([
      ...PRODUCTION_REGISTRY,
    ]);
    expect(effectiveProviderOrder(PRODUCTION_REGISTRY, { order: [], hidden: [] })).toEqual([
      ...PRODUCTION_REGISTRY,
    ]);
  });

  it("never hides a provider: visibility stays a separate preference", () => {
    const resolved = resolveProviderPreferences(PRODUCTION_REGISTRY, {
      order: [],
      hidden: ["zai", "grok"],
    });
    expect(resolved.visibleOrderedIds).toEqual([
      "openai-codex",
      "openai-codex:secondary",
      "antigravity",
      "opencode-go",
      "qwen-token-plan",
      "minimax-token-plan",
      "opendesign",
    ]);
    expect(resolved.hiddenIds).toEqual(["zai", "grok"]);
  });

  it("preserves a persisted custom order instead of the default", () => {
    const custom = {
      order: ["grok", "openai-codex", "zai", "opencode-go", "antigravity"],
      hidden: [],
    };
    expect(effectiveProviderOrder(REGISTRY, custom)).toEqual([
      "grok",
      "openai-codex",
      "zai",
      "opencode-go",
      "antigravity",
    ]);
    expect(resolveProviderPreferences(REGISTRY, custom).visibleOrderedIds).toEqual([
      "grok",
      "openai-codex",
      "zai",
      "opencode-go",
      "antigravity",
    ]);
  });

  it("appends a provider the default does not name, in registry order", () => {
    const registry = [...PRODUCTION_REGISTRY, "future-provider"];
    expect(effectiveProviderOrder(registry, undefined)).toEqual([
      ...PRODUCTION_REGISTRY,
      "future-provider",
    ]);
  });

  it("ignores a default entry that is not registered", () => {
    // The second Codex account is absent while it is not configured.
    expect(effectiveProviderOrder(REGISTRY, undefined)).toEqual([
      "openai-codex",
      "antigravity",
      "zai",
      "opencode-go",
      "grok",
    ]);
  });

  it("shows the default order in the settings list", () => {
    expect(fullDisplayOrder(PRODUCTION_REGISTRY, undefined)).toEqual([
      ...PRODUCTION_REGISTRY,
    ]);
    expect(fullDisplayOrder(PRODUCTION_REGISTRY, { order: [], hidden: ["zai"] })).toEqual([
      "openai-codex",
      "openai-codex:secondary",
      "antigravity",
      "zai",
      "opencode-go",
      "grok",
      "qwen-token-plan",
      "minimax-token-plan",
      "opendesign",
    ]);
  });
});

