import {
  sanitizeProviderPreferences,
  type ProviderPreferences,
} from "./providerPreferences";
import {
  isQuotaPerspective,
  type QuotaPerspective,
} from "./quotaPresentation";

export const REFRESH_INTERVAL_MINUTES = [1, 5, 15, 30] as const;
export type RefreshIntervalMinutes = (typeof REFRESH_INTERVAL_MINUTES)[number];

export const THEMES = ["graphite", "glass", "oled"] as const;
export type Theme = (typeof THEMES)[number];

export const THEME_LABELS: Record<Theme, string> = {
  graphite: "Graphite",
  glass: "Glass",
  oled: "OLED",
};

export { QUOTA_PERSPECTIVES } from "./quotaPresentation";
export type { QuotaPerspective } from "./quotaPresentation";

export type Settings = {
  launchAtStartup: boolean;
  refreshIntervalMinutes: RefreshIntervalMinutes;
  theme: Theme;
  quotaNotifications: boolean;
  /**
   * Provider presentation preferences (v0.6): which providers are shown
   * prominently and in what order. Presentation only - the runtime keeps
   * refreshing every registered provider, history keeps accumulating,
   * and notification evaluation is unchanged.
   */
  providerPreferences: ProviderPreferences;
  /**
   * Quota presentation perspective (v0.6): whether meters read "used" or
   * "remaining". Presentation only; canonical usedPercent values and
   * history are never rewritten to match it.
   */
  quotaPerspective: QuotaPerspective;
};

export const DEFAULT_SETTINGS: Settings = {
  launchAtStartup: false,
  refreshIntervalMinutes: 5,
  theme: "graphite",
  // Interruptive behavior is opt-in, matching launchAtStartup: the runtime
  // lane starts disabled until the user turns notifications on.
  quotaNotifications: false,
  providerPreferences: { order: [], hidden: [] },
  quotaPerspective: "used",
};

// Plain localStorage is the smallest mechanism that survives restarts without
// an extra plugin or Tauri command. Stored under the WebView2 user-data dir.
export const SETTINGS_STORAGE_KEY = "rate-limits.settings.v1";

// Keys this schema owns. Every other key found in stored JSON belongs to
// another lane or a future version, and must survive writes untouched.
const KNOWN_SETTINGS_KEYS: ReadonlySet<string> = new Set([
  "launchAtStartup",
  "refreshIntervalMinutes",
  "theme",
  "quotaNotifications",
  "providerPreferences",
  "quotaPerspective",
]);

function isRefreshInterval(value: unknown): value is RefreshIntervalMinutes {
  return (
    typeof value === "number" &&
    (REFRESH_INTERVAL_MINUTES as readonly number[]).includes(value)
  );
}

function isTheme(value: unknown): value is Theme {
  return typeof value === "string" && (THEMES as readonly string[]).includes(value);
}

/**
 * Validates arbitrary parsed storage content into Settings. Invalid or
 * absent fields fall back to their defaults while valid siblings survive,
 * so one corrupted value never discards the user's other preferences.
 */
export function parseSettings(raw: unknown): Settings {
  const settings: Settings = {
    ...DEFAULT_SETTINGS,
    providerPreferences: { ...DEFAULT_SETTINGS.providerPreferences },
  };
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return settings;
  const record = raw as Record<string, unknown>;
  if (typeof record.launchAtStartup === "boolean") {
    settings.launchAtStartup = record.launchAtStartup;
  }
  if (isRefreshInterval(record.refreshIntervalMinutes)) {
    settings.refreshIntervalMinutes = record.refreshIntervalMinutes;
  }
  if (isTheme(record.theme)) {
    settings.theme = record.theme;
  }
  if (typeof record.quotaNotifications === "boolean") {
    settings.quotaNotifications = record.quotaNotifications;
  }
  settings.providerPreferences = sanitizeProviderPreferences(
    record.providerPreferences,
  );
  if (isQuotaPerspective(record.quotaPerspective)) {
    settings.quotaPerspective = record.quotaPerspective;
  }
  return settings;
}

export function loadSettings(): Settings {
  try {
    const raw = localStorage.getItem(SETTINGS_STORAGE_KEY);
    if (!raw) return parseSettings(null);
    return parseSettings(JSON.parse(raw));
  } catch {
    // Corrupted or unavailable storage falls back to defaults.
    return parseSettings(null);
  }
}

/**
 * Builds the stored value for a save: foreign keys from the raw stored
 * object pass through untouched, then the validated in-memory settings
 * overwrite their canonical keys. Canonical fields always win - a stale
 * invalid raw value never resurrects over the validated state - and
 * storage that is corrupt or not an object preserves nothing.
 */
export function mergeSettingsWithRaw(
  raw: string | null,
  settings: Settings,
): string {
  let base: Record<string, unknown> = {};
  if (raw) {
    try {
      const parsed: unknown = JSON.parse(raw);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        base = parsed as Record<string, unknown>;
      }
    } catch {
      base = {};
    }
  }
  const merged: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(base)) {
    if (!KNOWN_SETTINGS_KEYS.has(key)) merged[key] = value;
  }
  merged.launchAtStartup = settings.launchAtStartup;
  merged.refreshIntervalMinutes = settings.refreshIntervalMinutes;
  merged.theme = settings.theme;
  merged.quotaNotifications = settings.quotaNotifications;
  merged.providerPreferences = sanitizeProviderPreferences(
    settings.providerPreferences,
  );
  merged.quotaPerspective = settings.quotaPerspective;
  return JSON.stringify(merged);
}

export function saveSettings(settings: Settings): void {
  try {
    localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      mergeSettingsWithRaw(localStorage.getItem(SETTINGS_STORAGE_KEY), settings),
    );
  } catch {
    // Persistence is best-effort; the UI keeps working without it.
  }
}

/**
 * The canonical reset for an explicit user action (v0.7 "Reset preferences"):
 * every field this schema owns goes back to its default and is persisted.
 *
 * The write goes through the same read-modify-write merge as an ordinary
 * save, so the v0.6 unknown-field policy still holds: fields another lane or
 * a future version wrote survive, because LimitScope only clears what it
 * knows it owns. Resetting therefore never destroys future/plugin state.
 */
export function resetSettings(): Settings {
  const defaults: Settings = {
    ...DEFAULT_SETTINGS,
    providerPreferences: { ...DEFAULT_SETTINGS.providerPreferences },
  };
  saveSettings(defaults);
  return defaults;
}
