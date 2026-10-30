import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
import {
  withWalletFloatingBar,
  withWalletMonitored,
  type WalletPreferences,
} from "../lib/walletPreferences";
import {
  withCodexAccount,
  type CodexAccountSettings,
} from "../lib/codexAccounts";


type OperationalSettings = { revision: number; quotaNotifications: boolean; monitored: Record<string, boolean> };

const USAGE_SAVE_ERROR =
  "Usage Intelligence preference could not be saved. Please retry.";
const ACCOUNT_SAVE_ERROR = "Account settings could not be saved. Please retry.";
const LOCAL_CACHE_NOTE = " The local preference cache could not be updated.";
const ACCOUNT_LOAD_ERROR_GENERIC =
  "Account settings could not be loaded. Existing account settings were preserved.";

// Rust rejects an unsupported account store with this exact, already-sanitized
// sentence. Anything else may contain paths or internal detail, so the UI gets
// a fixed message instead of the raw IPC error.
const UNSUPPORTED_ACCOUNT_SCHEMA =
  /^Unsupported account settings schema version \d+; existing file preserved$/;

function accountLoadErrorText(error: unknown): string {
  const raw =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : "";
  const message = raw.trim();
  return UNSUPPORTED_ACCOUNT_SCHEMA.test(message)
    ? message
    : ACCOUNT_LOAD_ERROR_GENERIC;
}

export function useSettings() {
  const [settings, setSettings] = useState<Settings>(loadSettings);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const [startupPending, setStartupPending] = useState(false);
  const [startupError, setStartupError] = useState<string | null>(null);
  const accountsEdit = useRef(0);
  const usageEdit = useRef(0);
  const accountWrites = useRef<Promise<void>>(Promise.resolve());
  const usageWrites = useRef<Promise<void>>(Promise.resolve());
  // Last value confirmed by Rust. A failed durable write returns here, so the
  // React state and this WebView's cache cannot keep an optimistic value even
  // when the recovery read fails too.
  const accountsCommitted = useRef<CodexAccountSettings[] | null>(null);
  const usageCommitted = useRef<boolean | null>(null);
  // Before the first confirmed Rust reply there is no committed value. The
  // chain base is the pre-edit value captured when an unresolved edit chain
  // starts, so a later queued failure can never roll back to another edit's
  // optimistic value.
  const accountsBase = useRef<CodexAccountSettings[] | null>(null);
  const usageBase = useRef<boolean | null>(null);
  const accountsPending = useRef(0);
  const usagePending = useRef(0);

  const operationalRevision = useRef(-1);
  const operationalWrites = useRef<Promise<void>>(Promise.resolve());
  const applyOperational = useCallback((value: OperationalSettings) => {
    if (!value || !Number.isFinite(value.revision) || value.revision < operationalRevision.current) return;
    operationalRevision.current = value.revision;
    setSettings(prev => ({
      ...prev, quotaNotifications: value.quotaNotifications,
      walletPreferences: { ...prev.walletPreferences, monitored: value.monitored },
    }));
  }, []);
  const writeOperational = useCallback((command: string, args: Record<string, unknown>) => {
    operationalWrites.current = operationalWrites.current.then(async () => {
      const value = await invoke<OperationalSettings>(command, args);
      applyOperational(value);
      setStartupError(null);
    }).catch(() => setStartupError("The preference could not be saved. Please retry."));
  }, [applyOperational]);
  useEffect(() => {
    if (!isRunningInTauri()) return;
    let cancelled = false;
    const subscription = listen<OperationalSettings>("operational-settings-changed", event => {
      if (!cancelled) applyOperational(event.payload);
    });
    void subscription.then(() => invoke<OperationalSettings>("get_operational_settings"))
      .then(value => { if (!cancelled) applyOperational(value); })
      .catch(() => { if (!cancelled) setStartupError(prev => prev ?? "Could not load saved monitoring preferences."); });
    return () => { cancelled = true; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [applyOperational]);

  // Rust loads these stores before the scheduler starts. The WebView copy
  // is only a cache: attaching a window must never push it back into Rust.
  useEffect(() => {
    if (!isRunningInTauri()) return;
    let cancelled = false;
    const accountsVersion = accountsEdit.current;
    const usageVersion = usageEdit.current;
    void invoke<CodexAccountSettings[]>("get_codex_accounts")
      .then((accounts) => {
        if (cancelled || accountsEdit.current !== accountsVersion) return;
        accountsCommitted.current = accounts;
        setSettings((prev) => ({ ...prev, codexAccounts: { accounts } }));
      })
      .catch((error) => {
        console.error("Failed to load the OpenAI/Codex account settings", error);
        if (cancelled) return;
        setStartupError((prev) => prev ?? accountLoadErrorText(error));
      });
    // Existing read-only projection: never probes sources or starts collection.
    void invoke<{ enabled: boolean }>("get_usage_intelligence", {
      query: { range: "today" },
    })
      .then(({ enabled }) => {
        if (cancelled || usageEdit.current !== usageVersion) return;
        usageCommitted.current = enabled;
        setSettings((prev) => ({ ...prev, usageIntelligence: enabled }));
      })
      .catch((error) => {
        console.error("Failed to load the Usage Intelligence setting", error);
      });
    return () => {
      cancelled = true;
    };
  }, []);

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
      const cached = loadSettings();
      setSettings((prev) =>
        isRunningInTauri()
          ? {
              ...cached,
              codexAccounts: prev.codexAccounts,
              usageIntelligence: prev.usageIntelligence,
              quotaNotifications: prev.quotaNotifications,
              walletPreferences: { ...cached.walletPreferences, monitored: prev.walletPreferences.monitored },
            }
          : cached,
      );
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
    if (isRunningInTauri()) { writeOperational("set_quota_notifications_enabled", { enabled: quotaNotifications }); return; }
    setSettings((prev) => {
      if (prev.quotaNotifications === quotaNotifications) return prev;
      const next = { ...prev, quotaNotifications };
      saveSettings(next);
      return next;
    });
  }, [writeOperational]);

  const setUsageIntelligence = useCallback((usageIntelligence: boolean) => {
    const edit = ++usageEdit.current;
    setSettings((prev) => {
      if (prev.usageIntelligence === usageIntelligence) return prev;
      const next = { ...prev, usageIntelligence };
      saveSettings(next);
      return next;
    });
    if (isRunningInTauri()) {
      if (usagePending.current === 0) {
        usageBase.current = settingsRef.current.usageIntelligence;
      }
      usagePending.current += 1;
      const rollbackValue = () =>
        usageCommitted.current ?? usageBase.current ?? settingsRef.current.usageIntelligence;
      const settled = () => {
        usagePending.current -= 1;
        if (usagePending.current === 0) usageBase.current = null;
      };
      // An explicit choice is valid even if the stale cache already had it.
      usageWrites.current = usageWrites.current
        .then(() =>
          invoke<void>("set_usage_intelligence_enabled", {
            enabled: usageIntelligence,
          }).then(() => {
            usageCommitted.current = usageIntelligence;
            setStartupError((prev) =>
              prev === USAGE_SAVE_ERROR || prev === USAGE_SAVE_ERROR + LOCAL_CACHE_NOTE
                ? null
                : prev,
            );
          }),
        )
        .catch((error) => {
          console.error("Failed to sync the Usage Intelligence setting", error);
          // A newer explicit choice owns the UI; its own write settles it.
          if (edit !== usageEdit.current) return;
          // Roll back React and the WebView cache to the last value Rust
          // confirmed before asking Rust again. If that recovery read also
          // fails, the committed value is still the honest state.
          const committed = rollbackValue();
          const persisted = saveSettings({
            ...settingsRef.current,
            usageIntelligence: committed,
          });
          // Re-save inside the updater so a concurrent edit's value cannot be
          // dropped from the merged cache write.
          setSettings((prev) => {
            if (prev.usageIntelligence === committed) return prev;
            const next = { ...prev, usageIntelligence: committed };
            saveSettings(next);
            return next;
          });
          setStartupError(persisted ? USAGE_SAVE_ERROR : USAGE_SAVE_ERROR + LOCAL_CACHE_NOTE);
          void invoke<{ enabled: boolean }>("get_usage_intelligence", { query: { range: "today" } })
            .then((value) => {
              if (edit !== usageEdit.current) return;
              usageCommitted.current = value.enabled;
              const cached = saveSettings({
                ...settingsRef.current,
                usageIntelligence: value.enabled,
              });
              setSettings((prev) => {
                if (prev.usageIntelligence === value.enabled) return prev;
                const next = { ...prev, usageIntelligence: value.enabled };
                saveSettings(next);
                return next;
              });
              if (!cached) {
                setStartupError((prev) =>
                  prev === USAGE_SAVE_ERROR || prev === null
                    ? USAGE_SAVE_ERROR + LOCAL_CACHE_NOTE
                    : prev,
                );
              }
            })
            .catch(() => {});
        })
        .finally(settled);
    }
  }, []);

  const setQuotaPerspective = useCallback((quotaPerspective: QuotaPerspective) => {
    setSettings((prev) => {
      if (prev.quotaPerspective === quotaPerspective) return prev;
      const next = { ...prev, quotaPerspective };
      saveSettings(next);
      return next;
    });
  }, []);

  const setWalletPreferences = useCallback(
    (walletPreferences: WalletPreferences) => {
      if (isRunningInTauri()) writeOperational("set_wallet_monitoring", { monitored: walletPreferences.monitored });
      setSettings((prev) => {
        const next = { ...prev, walletPreferences: isRunningInTauri() ? { ...walletPreferences, monitored: prev.walletPreferences.monitored } : walletPreferences };
        saveSettings(next);
        return next;
      });
    },
    [],
  );

  /**
   * Turns one wallet's automatic balance collection on or off. The runtime
   * enforces the flag (its own persisted mirror is loaded before the first
   * collection cycle); a disabled wallet keeps its last known balance and
   * never loses historical data.
   */
  const setWalletMonitored = useCallback((walletId: string, monitored: boolean) => {
    if (isRunningInTauri()) { writeOperational("set_wallet_monitoring", { monitored: { [walletId]: monitored } }); return; }
    setSettings((prev) => {
      const next = {
        ...prev,
        walletPreferences: withWalletMonitored(
          prev.walletPreferences,
          walletId,
          monitored,
        ),
      };
      saveSettings(next);
      return next;
    });
  }, [writeOperational]);

  /**
   * Shows or hides one wallet in the floating API Wallets card.
   * Presentation only: a hidden wallet keeps being monitored, and a
   * monitored wallet stays displayable with no other configuration change.
   */
  const setWalletInFloatingBar = useCallback((walletId: string, visible: boolean) => {
    setSettings((prev) => {
      const next = {
        ...prev,
        walletPreferences: withWalletFloatingBar(
          prev.walletPreferences,
          walletId,
          visible,
        ),
      };
      saveSettings(next);
      return next;
    });
  }, []);

  /**
   * Updates one OpenAI/Codex account: its alias, its own credential
   * reference (the Codex home that holds that account's `auth.json`) or
   * whether it is still monitored.
   *
   * Only an explicit edit writes to Rust. Read the effective configuration
   * before applying the patch so a stale window cannot replace the other
   * account's alias, credential reference or monitoring flag. Serialize
   * edits to preserve rapid input and changes to different accounts.
   */
  const setCodexAccount = useCallback(
    (
      accountId: string,
      patch: Partial<
        Pick<CodexAccountSettings, "alias" | "codexHome" | "monitored">
      >,
    ) => {
      const edit = ++accountsEdit.current;
      setSettings((prev) => {
        const next = {
          ...prev,
          codexAccounts: {
            accounts: withCodexAccount(
              prev.codexAccounts.accounts,
              accountId,
              patch,
            ),
          },
        };
        saveSettings(next);
        return next;
      });
      if (!isRunningInTauri()) return;
      if (accountsPending.current === 0) {
        accountsBase.current = settingsRef.current.codexAccounts.accounts;
      }
      accountsPending.current += 1;
      const rollbackAccounts = () =>
        accountsCommitted.current ??
        accountsBase.current ??
        settingsRef.current.codexAccounts.accounts;
      const settled = () => {
        accountsPending.current -= 1;
        if (accountsPending.current === 0) accountsBase.current = null;
      };
      accountWrites.current = accountWrites.current
        .then(async () => {
          const current = await invoke<CodexAccountSettings[]>("get_codex_accounts");
          const accounts = withCodexAccount(current, accountId, patch);
          if (accounts === current) {
            accountsCommitted.current = current;
            return;
          }
          await invoke("set_codex_accounts", { accounts });
          accountsCommitted.current = accounts;
          if (edit !== accountsEdit.current) return;
          setSettings((prev) => {
            const next = { ...prev, codexAccounts: { accounts } };
            saveSettings(next);
            return next;
          });
          setStartupError((prev) =>
            prev === ACCOUNT_SAVE_ERROR ||
            prev === ACCOUNT_SAVE_ERROR + LOCAL_CACHE_NOTE ||
            prev === ACCOUNT_LOAD_ERROR_GENERIC
              ? null
              : prev,
          );
        })
        .catch((error) => {
          console.error("Failed to sync the OpenAI/Codex account settings", error);
          if (edit !== accountsEdit.current) return;
          // Restore the last configuration Rust confirmed, in React and in
          // this WebView's cache, before the recovery read. A failed recovery
          // leaves that confirmed configuration in place.
          const committed = rollbackAccounts();
          const persistRollback = () => {
            const persisted = saveSettings({
              ...settingsRef.current,
              codexAccounts: { accounts: committed },
            });
            setSettings((prev) => {
              if (prev.codexAccounts.accounts === committed) return prev;
              const next = { ...prev, codexAccounts: { accounts: committed } };
              saveSettings(next);
              return next;
            });
            setStartupError((prev) => {
              // The safe schema diagnostic explains why saving is refused;
              // a later generic message would hide that cause.
              if (prev && UNSUPPORTED_ACCOUNT_SCHEMA.test(prev)) return prev;
              return persisted ? ACCOUNT_SAVE_ERROR : ACCOUNT_SAVE_ERROR + LOCAL_CACHE_NOTE;
            });
            return persisted;
          };
          persistRollback();
          void invoke<CodexAccountSettings[]>("get_codex_accounts").then(accounts => {
            if (edit !== accountsEdit.current) return;
            accountsCommitted.current = accounts;
            const cached = saveSettings({ ...settingsRef.current, codexAccounts: { accounts } });
            setSettings(prev => {
              if (prev.codexAccounts.accounts === accounts) return prev;
              const next = { ...prev, codexAccounts: { accounts } };
              saveSettings(next);
              return next;
            });
            if (!cached) {
              setStartupError(prev =>
                prev === ACCOUNT_SAVE_ERROR || prev === null
                  ? ACCOUNT_SAVE_ERROR + LOCAL_CACHE_NOTE
                  : prev,
              );
            }
          }).catch(() => {});
        })
        .finally(settled);
    },
    [],
  );

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
    // Reset is also an explicit action; late initialization reads cannot
    // restore the pre-reset values, and pending edits must finish first.
    accountsEdit.current += 1;
    usageEdit.current += 1;
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
      await Promise.all([accountWrites.current, usageWrites.current, operationalWrites.current]);
      // The runtime persists two of these preferences itself (the
      // notification lane's enabled flag and the refresh interval), so the
      // reset is mirrored into it. Both setters are idempotent, which makes
      // the ordinary change effects firing on the same state harmless. The
      // wallet monitor map is mirrored the same way: its Rust store is
      // loaded before the first collection cycle.
      await invoke("set_quota_notifications_enabled", {
        enabled: defaults.quotaNotifications,
      });
      await invoke("set_refresh_interval", {
        minutes: defaults.refreshIntervalMinutes,
      });
      await invoke("set_wallet_monitoring", {
        monitored: defaults.walletPreferences.monitored,
      });
      await invoke("set_codex_accounts", { accounts: defaults.codexAccounts.accounts });
      await invoke("set_usage_intelligence_enabled", { enabled: defaults.usageIntelligence });
      // The reset is the new confirmed Rust state for both protected stores.
      accountsCommitted.current = defaults.codexAccounts.accounts;
      usageCommitted.current = defaults.usageIntelligence;
    } catch (error) {
      console.error("Failed to push the reset preferences to the runtime", error);
      return { ok: false };
    }
    return { ok: true, removed: true };
  }, []);

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
    setWalletPreferences,
    setWalletMonitored,
    setWalletInFloatingBar,
    setCodexAccount,
    resetPreferences,
    startupPending,
    startupError,
  };
}


