import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { isRunningInTauri } from "../lib/runtime";
import {
  resetPreferenceStores,
  type LocalDataClearResult,
} from "../lib/localData";
import type { ProviderPreferences } from "../lib/providerPreferences";
import {
  loadSettings,
  saveSettings,
  SETTINGS_STORAGE_KEY,
  type RefreshIntervalMinutes,
  type Settings,
  type Theme,
  type QuotaPerspective,
} from "../lib/settings";
import {
  fullDisplayOrder,
  moveProviderOrder,
  toggleProviderHidden,
} from "../lib/providerPreferences";


export function useSettings() {
  const [settings, setSettings] = useState<Settings>(loadSettings);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const [startupPending, setStartupPending] = useState(false);
  const [startupError, setStartupError] = useState<string | null>(null);

  // The OS autostart state is authoritative over the stored preference:
  // reconcile once at startup in case the entry was changed or removed
  // outside the app (e.g. Task Manager's Startup tab).
  useEffect(() => {
    if (!isRunningInTauri()) return;
    let cancelled = false;
    isEnabled()
      .then((enabled) => {
        if (cancelled) return;
        setSettings((prev) => {
          if (prev.launchAtStartup === enabled) return prev;
          const next = { ...prev, launchAtStartup: enabled };
          saveSettings(next);
          return next;
        });
      })
      .catch(() => {
        // The plugin is only consulted to correct drift; a failed probe
        // leaves the stored preference untouched.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Other windows of this app share the WebView profile. A theme change in
  // the dashboard has to reach the floating bar without a second theme system.
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== SETTINGS_STORAGE_KEY) return;
      setSettings(loadSettings());
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  const setLaunchAtStartup = useCallback(async (launchAtStartup: boolean) => {
    setStartupError(null);
    if (!isRunningInTauri()) {
      // Plain-browser dev: remember the preference without OS interaction.
      setSettings((prev) => {
        const next = { ...prev, launchAtStartup };
        saveSettings(next);
        return next;
      });
      return;
    }
    setStartupPending(true);
    const previous = settingsRef.current.launchAtStartup;
    setSettings((prev) => {
      const next = { ...prev, launchAtStartup };
      saveSettings(next);
      return next;
    });
    try {
      if (launchAtStartup) {
        await enable();
      } else {
        await disable();
      }
    } catch (error) {
      // Roll the toggle back to the state the OS actually has.
      setSettings((prev) => {
        const next = { ...prev, launchAtStartup: previous };
        saveSettings(next);
        return next;
      });
      setStartupError(
        error instanceof Error ? error.message : String(error),
      );
    } finally {
      setStartupPending(false);
    }
  }, []);

  const setRefreshInterval = useCallback(
    (refreshIntervalMinutes: RefreshIntervalMinutes) => {
      setSettings((prev) => {
        if (prev.refreshIntervalMinutes === refreshIntervalMinutes) return prev;
        const next = { ...prev, refreshIntervalMinutes };
        saveSettings(next);
        return next;
      });
    },
    [],
  );

  const setTheme = useCallback((theme: Theme) => {
    setSettings((prev) => {
      if (prev.theme === theme) return prev;
      const next = { ...prev, theme };
      saveSettings(next);
      return next;
    });
  }, []);

  const setQuotaNotifications = useCallback((quotaNotifications: boolean) => {
    setSettings((prev) => {
      if (prev.quotaNotifications === quotaNotifications) return prev;
      const next = { ...prev, quotaNotifications };
      saveSettings(next);
      return next;
    });
  }, []);

  const setUsageIntelligence = useCallback((usageIntelligence: boolean) => {
    setSettings((prev) => {
      if (prev.usageIntelligence === usageIntelligence) return prev;
      const next = { ...prev, usageIntelligence };
      saveSettings(next);
      return next;
    });
  }, []);

  const setQuotaPerspective = useCallback((quotaPerspective: QuotaPerspective) => {
    setSettings((prev) => {
      if (prev.quotaPerspective === quotaPerspective) return prev;
      const next = { ...prev, quotaPerspective };
      saveSettings(next);
      return next;
    });
  }, []);

  const setProviderPreferences = useCallback(
    (providerPreferences: ProviderPreferences) => {
      setSettings((prev) => {
        const next = { ...prev, providerPreferences };
        saveSettings(next);
        return next;
      });
    },
    [],
  );

  /**
   * Hides or re-shows a provider in prominent UI surfaces. Presentation
   * only: the runtime keeps refreshing, recording history, and evaluating
   * notifications for hidden providers. Returns false when the change was
   * refused - hiding the final visible provider, or an unknown id - so the
   * caller can explain instead of leaving an empty UI.
   */
  const setProviderHidden = useCallback(
    (providerId: string, hidden: boolean, registryIds: readonly string[]): boolean => {
      const { prefs, applied } = toggleProviderHidden(
        registryIds,
        settingsRef.current.providerPreferences,
        providerId,
        hidden,
      );
      if (!applied) return false;
      setSettings((prev) => {
        const next = { ...prev, providerPreferences: prefs };
        saveSettings(next);
        return next;
      });
      return true;
    },
    [],
  );

  /**
   * Moves a provider up (-1) or down (+1) in the presentation order.
   * Boundary moves and unknown ids leave the order unchanged.
   */
  const moveProvider = useCallback(
    (providerId: string, direction: 1 | -1, registryIds: readonly string[]): boolean => {
      const current = settingsRef.current.providerPreferences;
      const next = moveProviderOrder(registryIds, current, providerId, direction);
      const before = fullDisplayOrder(registryIds, current);
      if (before.join("\0") === next.join("\0")) return false;
      setSettings((prev) => {
        const prefs = { ...prev.providerPreferences, order: next };
        const nextSettings = { ...prev, providerPreferences: prefs };
        saveSettings(nextSettings);
        return nextSettings;
      });
      return true;
    },
    [],
  );

  /**
   * The v0.7 "Reset preferences" action: every canonical LimitScope
   * preference returns to its default.
   *
   * Both WebView-owned stores (core settings, floating bar) are reset through
   * their own APIs, so unknown/future fields written by another lane survive:
   * an explicit reset clears what LimitScope knows it owns and nothing else.
   *
   * Launch-at-startup is the only OS integration in this class. Its canonical
   * default is off, so LimitScope's own autostart entry is disabled — no
   * other Run entry is read, modified, or removed. Quota history, the provider
   * cache, and every credential store are untouched by design.
   */
  const resetPreferences = useCallback(async (): Promise<LocalDataClearResult> => {
    setStartupError(null);
    const { settings: defaults } = resetPreferenceStores();
    setSettings(defaults);
    if (!isRunningInTauri()) return { ok: true, removed: true };
    setStartupPending(true);
    try {
      await disable();
    } catch (error) {
      // The preference stores are already at their defaults; report the
      // partial result instead of claiming a clean reset. The OS entry keeps
      // whatever state it actually has, and the settings note explains it.
      setStartupError(error instanceof Error ? error.message : String(error));
      return { ok: false };
    } finally {
      setStartupPending(false);
    }
    try {
      // The runtime persists two of these preferences itself (the
      // notification lane's enabled flag and the refresh interval), so the
      // reset is mirrored into it. Both setters are idempotent, which makes
      // the ordinary change effects firing on the same state harmless.
      await invoke("set_quota_notifications_enabled", {
        enabled: defaults.quotaNotifications,
      });
      await invoke("set_refresh_interval", {
        minutes: defaults.refreshIntervalMinutes,
      });
    } catch (error) {
      console.error("Failed to push the reset preferences to the runtime", error);
      return { ok: false };
    }
    return { ok: true, removed: true };
  }, []);

  // The runtime's notification lane is the single evaluation point, so the
  // toggle is only forwarded to Rust (on attach and on every change, like
  // the refresh interval). The lane persists the flag itself, which covers
  // the scheduler's immediate startup cycle before any window attaches.
  useEffect(() => {
    if (!isRunningInTauri()) return;
    void invoke("set_quota_notifications_enabled", {
      enabled: settings.quotaNotifications,
    }).catch((error) => {
      console.error("Failed to sync the quota notification setting", error);
    });
  }, [settings.quotaNotifications]);

  // The runtime's Usage Intelligence store is the single collection owner
  // (v0.8.9), so the opt-in toggle is forwarded to Rust on attach and on
  // every change, like the notification lane. The store persists the flag
  // itself, which covers the scheduler's startup cycle before any window
  // attaches; enabling there triggers the first baseline immediately.
  useEffect(() => {
    if (!isRunningInTauri()) return;
    void invoke("set_usage_intelligence_enabled", {
      enabled: settings.usageIntelligence,
    }).catch((error) => {
      console.error("Failed to sync the Usage Intelligence setting", error);
    });
  }, [settings.usageIntelligence]);

  return {
    settings,
    setLaunchAtStartup,
    setRefreshInterval,
    setTheme,
    setQuotaNotifications,
    setUsageIntelligence,
    setQuotaPerspective,
    setProviderPreferences,
    setProviderHidden,
    moveProvider,
    resetPreferences,
    startupPending,
    startupError,
  };
}


