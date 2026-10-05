import { describe, expect, it } from "vitest";
import {
  canHideProvider,
  fullDisplayOrder,
  moveProviderOrder,
  orderVisibleProviders,
  resolveProviderPreferences,
  sanitizeProviderPreferences,
  toggleProviderHidden,
} from "./providerPreferences";

const REGISTRY = ["openai-codex", "zai", "opencode-go", "antigravity", "grok"];

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
  it("defaults to canonical registry order with nothing hidden", () => {
    expect(resolveProviderPreferences(REGISTRY, undefined)).toEqual({
      visibleOrderedIds: [...REGISTRY],
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
    expect(resolved.visibleOrderedIds).toEqual(["openai-codex", "opencode-go", "antigravity"]);
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

