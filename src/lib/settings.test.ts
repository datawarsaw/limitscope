import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  resolveProviderPreferences,
  toggleProviderHidden,
  type ProviderPreferences,
} from "./providerPreferences";
import {
  DEFAULT_SETTINGS,
  loadSettings,
  mergeSettingsWithRaw,
  resetSettings,
  saveSettings,
  type Settings,
} from "./settings";

// loadSettings/saveSettings talk to localStorage, which node's test
// environment does not provide; a Map-backed stub keeps the tests honest
// about what round-trips through JSON.
function stubStorage() {
  const map = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
    removeItem: (key: string) => void map.delete(key),
    clear: () => void map.clear(),
  });
  return map;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("settings persistence", () => {
  it("falls back to defaults when nothing is stored", () => {
    stubStorage();
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS);
  });

  it("round-trips a saved theme", () => {
    const map = stubStorage();
    saveSettings({ ...DEFAULT_SETTINGS, theme: "glass" });
    expect(loadSettings().theme).toBe("glass");

    map.clear();
    saveSettings({ ...DEFAULT_SETTINGS, theme: "oled" });
    expect(loadSettings().theme).toBe("oled");
  });

  it("keeps an unknown theme value from reaching the UI", () => {
    stubStorage().set(
      "rate-limits.settings.v1",
      JSON.stringify({ ...DEFAULT_SETTINGS, theme: "neon" }),
    );
    expect(loadSettings().theme).toBe("graphite");
  });

  it("defaults the theme for settings written before themes existed", () => {
    stubStorage().set(
      "rate-limits.settings.v1",
      JSON.stringify({ launchAtStartup: true, refreshIntervalMinutes: 15 }),
    );
    expect(loadSettings()).toEqual({
      launchAtStartup: true,
      refreshIntervalMinutes: 15,
      theme: "graphite",
      quotaNotifications: false,
      providerPreferences: { order: [], hidden: [] },
      quotaPerspective: "used",
    });
  });

  it("round-trips the quota notifications toggle", () => {
    stubStorage();
    saveSettings({ ...DEFAULT_SETTINGS, quotaNotifications: true });
    expect(loadSettings().quotaNotifications).toBe(true);
  });

  it("defaults quota notifications for settings written before the toggle existed", () => {
    stubStorage().set(
      "rate-limits.settings.v1",
      JSON.stringify({
        launchAtStartup: true,
        refreshIntervalMinutes: 15,
        theme: "glass",
      }),
    );
    expect(loadSettings().quotaNotifications).toBe(false);
  });

  it("keeps a non-boolean quota notifications value from reaching the UI", () => {
    stubStorage().set(
      "rate-limits.settings.v1",
      JSON.stringify({ ...DEFAULT_SETTINGS, quotaNotifications: "yes" }),
    );
    expect(loadSettings().quotaNotifications).toBe(false);
  });

  it("falls back to defaults on corrupted storage", () => {
    stubStorage().set("rate-limits.settings.v1", "{not json");
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS);
  });
});

// Fixtures ported from research/v0.6-settings-migration-audit; the
// assertions below pin the same contract against the real production
// settings code rather than a research-only parallel implementation.
const FIXTURES_DIR = join(import.meta.dirname, "../../fixtures/settings-migration");

function readFixture(name: string): string {
  return readFileSync(join(FIXTURES_DIR, name), "utf8");
}

function withPrefs(providerPreferences: ProviderPreferences): Settings {
  return { ...DEFAULT_SETTINGS, providerPreferences };
}

describe("v0.6 settings migration contract", () => {
  it("loads legacy v0.3 settings and defaults every later field", () => {
    stubStorage().set("rate-limits.settings.v1", readFixture("legacy-minimal.json"));
    expect(loadSettings()).toEqual({
      launchAtStartup: false,
      refreshIntervalMinutes: 5,
      theme: "graphite",
      quotaNotifications: false,
      providerPreferences: { order: [], hidden: [] },
      quotaPerspective: "used",
    });
  });

  it("loads legacy v0.5 settings and defaults the v0.6 fields", () => {
    stubStorage().set("rate-limits.settings.v1", readFixture("v0.5-full.json"));
    expect(loadSettings()).toEqual({
      launchAtStartup: true,
      refreshIntervalMinutes: 15,
      theme: "glass",
      quotaNotifications: true,
      providerPreferences: { order: [], hidden: [] },
      quotaPerspective: "used",
    });
  });

  it("keeps providerPreferences when a quotaPerspective save lands", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      readFixture("partial-v0.6.json"),
    );
    const loaded = loadSettings();
    saveSettings({ ...loaded, quotaPerspective: "remaining" });

    const stored = JSON.parse(map.get("rate-limits.settings.v1")!);
    expect(stored.providerPreferences).toEqual({
      order: ["zai", "openai-codex"],
      hidden: [],
    });
    expect(stored.quotaPerspective).toBe("remaining");
    expect(loadSettings().quotaPerspective).toBe("remaining");
  });

  it("keeps quotaPerspective when a providerPreferences save lands", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      JSON.stringify({
        ...DEFAULT_SETTINGS,
        quotaPerspective: "remaining",
      }),
    );
    const loaded = loadSettings();
    saveSettings({
      ...loaded,
      providerPreferences: { order: ["grok"], hidden: [] },
    });

    const stored = JSON.parse(map.get("rate-limits.settings.v1")!);
    expect(stored.quotaPerspective).toBe("remaining");
    expect(stored.providerPreferences).toEqual({ order: ["grok"], hidden: [] });
  });

  it("keeps unknown future fields across a save", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      readFixture("future-unknown-field.json"),
    );
    saveSettings(loadSettings());

    const stored = JSON.parse(map.get("rate-limits.settings.v1")!);
    // Not implemented yet: transparency is a foreign field here and must
    // pass through untouched instead of being adopted or erased.
    expect(stored.surfaceTransparency).toBe(45);
    expect(stored.futurePluginSettings).toEqual({
      telemetryOptIn: false,
      experimentalCharts: true,
    });
    expect(loadSettings().theme).toBe("glass");
  });

  it("normalizes invalid known values instead of preserving them", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      readFixture("invalid-values.json"),
    );
    const loaded = loadSettings();
    expect(loaded).toEqual({
      launchAtStartup: false,
      refreshIntervalMinutes: 5,
      theme: "graphite",
      quotaNotifications: false,
      providerPreferences: { order: ["openai-codex"], hidden: [] },
      quotaPerspective: "used",
    });

    saveSettings(loaded);
    const stored = JSON.parse(map.get("rate-limits.settings.v1")!);
    expect(stored.theme).toBe("graphite");
    expect(stored.refreshIntervalMinutes).toBe(5);
    expect(stored.quotaPerspective).toBe("used");
  });

  it("replaces corrupt storage with a clean canonical object on save", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      readFixture("corrupt-json.json"),
    );
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS);

    saveSettings({ ...DEFAULT_SETTINGS, theme: "oled" });
    const stored = JSON.parse(map.get("rate-limits.settings.v1")!);
    expect(stored).toEqual({
      launchAtStartup: false,
      refreshIntervalMinutes: 5,
      theme: "oled",
      quotaNotifications: false,
      providerPreferences: { order: [], hidden: [] },
      quotaPerspective: "used",
    });
  });

  it("appends registry providers missing from the saved order", () => {
    const registry = [
      "openai-codex",
      "zai",
      "opencode-go",
      "antigravity",
      "grok",
      "future-provider",
    ];
    const resolved = resolveProviderPreferences(registry, {
      order: ["grok", "openai-codex"],
      hidden: [],
    });
    expect(resolved.visibleOrderedIds).toEqual([
      "grok",
      "openai-codex",
      "zai",
      "opencode-go",
      "antigravity",
      "future-provider",
    ]);
    expect(resolved.hiddenIds).toEqual([]);
  });

  it("never hides the last visible provider and self-heals an all-hidden store", () => {
    const registry = ["openai-codex", "zai"];

    const refused = toggleProviderHidden(
      registry,
      { order: ["openai-codex", "zai"], hidden: ["zai"] },
      "openai-codex",
      true,
    );
    expect(refused.applied).toBe(false);

    const healed = resolveProviderPreferences(registry, {
      order: ["openai-codex", "zai"],
      hidden: ["openai-codex", "zai"],
    });
    expect(healed.visibleOrderedIds).toEqual(["openai-codex"]);
  });

  it("round-trips the full v0.6 canonical settings", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      readFixture("v0.6-full.json"),
    );
    const loaded = loadSettings();
    expect(loaded).toEqual({
      launchAtStartup: true,
      refreshIntervalMinutes: 15,
      theme: "oled",
      quotaNotifications: true,
      providerPreferences: {
        order: ["openai-codex", "zai", "opencode-go", "antigravity", "grok"],
        hidden: ["grok"],
      },
      quotaPerspective: "remaining",
    });

    saveSettings(loaded);
    expect(JSON.parse(map.get("rate-limits.settings.v1")!)).toEqual(loaded);
    expect(loadSettings()).toEqual(loaded);
  });
});

describe("settings safe write contract", () => {
  it("passes foreign keys through while canonical keys win over stale raw values", () => {
    const raw = JSON.stringify({
      theme: "broken",
      futureFeature: 123,
      futurePluginSettings: { telemetryOptIn: false },
    });
    const stored = JSON.parse(mergeSettingsWithRaw(raw, DEFAULT_SETTINGS));
    expect(stored).toEqual({
      futureFeature: 123,
      futurePluginSettings: { telemetryOptIn: false },
      launchAtStartup: false,
      refreshIntervalMinutes: 5,
      theme: "graphite",
      quotaNotifications: false,
      providerPreferences: { order: [], hidden: [] },
      quotaPerspective: "used",
    });
  });

  it("preserves nothing when the stored value is corrupt or not an object", () => {
    const corrupt = JSON.parse(mergeSettingsWithRaw("{not json", DEFAULT_SETTINGS));
    expect(corrupt).toEqual(DEFAULT_SETTINGS);

    const primitive = JSON.parse(mergeSettingsWithRaw('"garbage"', DEFAULT_SETTINGS));
    expect(primitive).toEqual(DEFAULT_SETTINGS);

    const array = JSON.parse(mergeSettingsWithRaw("[1,2,3]", DEFAULT_SETTINGS));
    expect(array).toEqual(DEFAULT_SETTINGS);
  });

  it("sanitizes provider preferences on write", () => {
    const stored = JSON.parse(
      mergeSettingsWithRaw(
        null,
        withPrefs({
          order: ["zai", "zai", "", 5 as unknown as string],
          hidden: ["grok", "grok"],
        }),
      ),
    );
    expect(stored.providerPreferences).toEqual({
      order: ["zai"],
      hidden: ["grok"],
    });
  });
});

describe("settings reset (v0.7 local data)", () => {
  it("pins the six canonical fields and preserves an unknown field across save, perspective change, and reset", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      JSON.stringify({
        theme: "glass",
        futureLaneField: { nested: [1, 2, 3] },
      }),
    );

    saveSettings(loadSettings());
    saveSettings({ ...loadSettings(), quotaPerspective: "remaining" });
    resetSettings();

    const stored = JSON.parse(map.get("rate-limits.settings.v1")!);
    expect(Object.keys(stored).sort()).toEqual([
      "futureLaneField",
      "launchAtStartup",
      "providerPreferences",
      "quotaNotifications",
      "quotaPerspective",
      "refreshIntervalMinutes",
      "theme",
    ]);
    expect(stored.futureLaneField).toEqual({ nested: [1, 2, 3] });
    expect(stored.quotaPerspective).toBe("used");
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS);
  });

  it("restores every canonical field to its default", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      JSON.stringify({
        launchAtStartup: true,
        refreshIntervalMinutes: 30,
        theme: "oled",
        quotaNotifications: true,
        providerPreferences: { order: ["grok"], hidden: ["zai"] },
        quotaPerspective: "remaining",
      }),
    );

    expect(resetSettings()).toEqual(DEFAULT_SETTINGS);
    expect(JSON.parse(map.get("rate-limits.settings.v1")!)).toEqual(
      DEFAULT_SETTINGS,
    );
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS);
  });

  it("preserves unknown future fields instead of deleting them", () => {
    const map = stubStorage().set(
      "rate-limits.settings.v1",
      JSON.stringify({
        theme: "glass",
        futurePluginSettings: { telemetryOptIn: false },
        surfaceTransparency: 45,
      }),
    );

    resetSettings();

    const stored = JSON.parse(map.get("rate-limits.settings.v1")!);
    expect(stored.theme).toBe("graphite");
    expect(stored.futurePluginSettings).toEqual({ telemetryOptIn: false });
    expect(stored.surfaceTransparency).toBe(45);
  });

  it("replaces a corrupt store with the canonical defaults", () => {
    const map = stubStorage().set("rate-limits.settings.v1", "{not json");
    resetSettings();
    expect(JSON.parse(map.get("rate-limits.settings.v1")!)).toEqual(
      DEFAULT_SETTINGS,
    );
  });

  it("is idempotent and safe on empty storage", () => {
    stubStorage();
    resetSettings();
    expect(resetSettings()).toEqual(DEFAULT_SETTINGS);
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS);
  });
});
