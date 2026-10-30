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
  REFRESH_INTERVAL_MINUTES,
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
import {
  ALL_PREFERENCE_SCOPES,
  PREFERENCE_SCOPE_LABELS,
  type PreferenceScope,
} from "../lib/preferenceScopes";


type OperationalSettings = { revision: number; quotaNotifications: boolean; monitored: Record<string, boolean> };

export type { PreferenceScope } from "../lib/preferenceScopes";

const USAGE_SAVE_ERROR =
  "Usage Intelligence preference could not be saved. Please retry.";
const ACCOUNT_SAVE_ERROR = "Account settings could not be saved. Please retry.";
const OPERATIONAL_SAVE_ERROR = "The preference could not be saved. Please retry.";
const LOCAL_CACHE_NOTE = " The local preference cache could not be updated.";
const ACCOUNT_LOAD_ERROR_GENERIC =
  "Account settings could not be loaded. Existing account settings were preserved.";
const PREFERENCE_ERROR_OWNERS: ReadonlySet<string> = new Set([
  "launchAtStartup",
  "refreshInterval",
  "operational",
  "accounts",
  "usageIntelligence",
  "reset",
]);

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

/** The raw reason of a failed write, as one sentence. */
function reasonSentence(reason: string): string {
  return reason.endsWith(".") ? reason : `${reason}.`;
}

/**
 * The message of a failed reset. It names the scopes whose saved state
 * could not be verified, so the alert and the per-row notes agree.
 */
function resetFailureMessage(
  autostartDetail: string | null,
  scopes: readonly PreferenceScope[],
): string {
  const labels = scopes.map((scope) => PREFERENCE_SCOPE_LABELS[scope]);
  const detail = labels.length > 0
    ? ` The saved state of ${labels.join(", ")} could not be verified.`
    : "";
  const autostart = autostartDetail ? `${autostartDetail} ` : "";
  return `${autostart}Preferences could not be reset.${detail} Please retry.`;
}

export function useSettings() {
  const [settings, setSettings] = useState<Settings>(loadSettings);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const [startupPending, setStartupPending] = useState(false);
  const [startupError, setStartupError] = useState<string | null>(null);
  // Scopes whose persisted value the last reset could not verify. The UI
  // shows the scoped error; exposing the groups keeps the uncertainty
  // explicit instead of presenting a stale value as the saved one.
  const [unverifiedPreferences, setUnverifiedPreferences] = useState<
    PreferenceScope[]
  >([]);
  // Every verification attempt of a preference scope owns a token. The
  // newest attempt decides the scope's verdict, so a slow read can never
  // overwrite a newer one, and a concurrent action can never clear the
  // unknown state another action just established.
  const scopeTokens = useRef<Record<PreferenceScope, number>>({
    launchAtStartup: 0,
    quotaNotifications: 0,
    walletMonitoring: 0,
    refreshIntervalMinutes: 0,
    codexAccounts: 0,
    usageIntelligence: 0,
  });
  const beginScopeCheck = useCallback((scope: PreferenceScope) => {
    scopeTokens.current[scope] += 1;
    return scopeTokens.current[scope];
  }, []);
  const settleScopeCheck = useCallback(
    (scope: PreferenceScope, token: number, verified: boolean) => {
      // A newer attempt owns this scope now; its verdict wins.
      if (scopeTokens.current[scope] !== token) return;
      setUnverifiedPreferences((prev) => {
        const listed = prev.includes(scope);
        if (verified === !listed) return prev;
        return verified
          ? prev.filter((item) => item !== scope)
          : [...prev, scope];
      });
    },
    [],
  );
  // Errors carry an owner and the generation of the preference action that
  // started them. A concurrent action may not drop another action's message,
  // and a successful reset may only supersede actions that started before it.
  // The unknown-scope list remains the durable record of what could not be
  // verified.
  const startupErrorOwner = useRef<string | null>(null);
  const startupErrorMessage = useRef<string | null>(null);
  const startupErrorAction = useRef<number | null>(null);
  const preferenceActionSequence = useRef(0);
  const completedPreferenceReset = useRef(0);
  const beginPreferenceAction = useCallback(() => {
    preferenceActionSequence.current += 1;
    return preferenceActionSequence.current;
  }, []);
  const publishStartupError = useCallback(
    (owner: string, message: string, action: number | null = null) => {
      // A late failure from an action that a successful reset superseded is
      // obsolete. It must not resurrect either an error or an unknown scope.
      if (
        action !== null &&
        action <= completedPreferenceReset.current &&
        PREFERENCE_ERROR_OWNERS.has(owner)
      ) {
        return;
      }
      startupErrorOwner.current = owner;
      startupErrorMessage.current = message;
      startupErrorAction.current = action;
      setStartupError(message);
    },
    [],
  );
  const publishStartupErrorIfEmpty = useCallback(
    (owner: string, message: string) => {
      if (startupErrorOwner.current !== null) return;
      startupErrorOwner.current = owner;
      startupErrorMessage.current = message;
      startupErrorAction.current = null;
      setStartupError(message);
    },
    [],
  );
  const clearStartupError = useCallback((owner: string, action?: number) => {
    if (startupErrorOwner.current !== owner) return;
    if (
      action !== undefined &&
      startupErrorAction.current !== null &&
      startupErrorAction.current > action
    ) {
      return;
    }
    startupErrorOwner.current = null;
    startupErrorMessage.current = null;
    startupErrorAction.current = null;
    setStartupError(null);
  }, []);
  const clearSupersededPreferenceErrors = useCallback((resetAction: number) => {
    completedPreferenceReset.current = Math.max(
      completedPreferenceReset.current,
      resetAction,
    );
    const owner = startupErrorOwner.current;
    const action = startupErrorAction.current;
    if (
      owner !== null &&
      PREFERENCE_ERROR_OWNERS.has(owner) &&
      action !== null &&
      action <= resetAction
    ) {
      startupErrorOwner.current = null;
      startupErrorMessage.current = null;
      startupErrorAction.current = null;
      setStartupError(null);
    }
  }, []);
  // The OS autostart entry is authoritative over the stored preference. A
  // setter can fail after the OS applied the change, so only a fresh read
  // tells which state the OS really has. A failed read yields null: the
  // caller keeps the scope explicitly unknown instead of guessing.
  const readAutostartState = useCallback(async (): Promise<boolean | null> => {
    try {
      const enabled = await isEnabled();
      return typeof enabled === "boolean" ? enabled : null;
    } catch {
      return null;
    }
  }, []);
  // The runtime owns the timer but not the interval: the value is persisted in
  // this WebView's cache and handed to Rust. A failed read yields null, so the
  // caller keeps the scope explicitly unknown instead of reusing the value the
  // rejected write merely intended to store.
  const readRefreshInterval = useCallback(
    async (): Promise<RefreshIntervalMinutes | null> => {
      try {
        const snapshot = await invoke<{ refreshIntervalMinutes?: unknown }>(
          "get_runtime_snapshot",
        );
        const minutes = snapshot?.refreshIntervalMinutes;
        return REFRESH_INTERVAL_MINUTES.find((option) => option === minutes) ?? null;
      } catch {
        return null;
      }
    },
    [],
  );
  // What the last failed reset left unresolved. The message follows the
  // unknown-scope list, so a scope that is verified again disappears from
  // the alert instead of staying named there.
  const resetFailureRef = useRef<{
    runtimeFailed: boolean;
    autostartDetail: string | null;
  }>({ runtimeFailed: false, autostartDetail: null });
  useEffect(() => {
    if (startupErrorOwner.current !== "reset") return;
    const failure = resetFailureRef.current;
    // Only the runtime failure names the scopes; the autostart-only failure
    // carries its own explanation and stays as it is.
    if (!failure.runtimeFailed) return;
    const message = resetFailureMessage(
      failure.autostartDetail,
      unverifiedPreferences,
    );
    startupErrorMessage.current = message;
    setStartupError(message);
  }, [unverifiedPreferences]);
  const accountsEdit = useRef(0);
  const usageEdit = useRef(0);
  const refreshEdit = useRef(0);
  const resetsPending = useRef(0);
  // Every operation that can change Rust-owned preferences shares one tail.
  // In particular, a reset can no longer overtake a reordered account write,
  // and an edit invoked after reset is appended after the reset defaults.
  const runtimePreferenceWrites = useRef<Promise<void>>(Promise.resolve());
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

  /**
   * Replaces a rejected interval write with the value the runtime really has.
   * A newer edit owns the value and its own queued write, so an older failure
   * never rolls it back. Without a readable runtime state the scope stays
   * unknown; `failure` is null for the startup hand-off, which has no user
   * action to explain.
   */
  const recoverRefreshInterval = useCallback(
    async (
      edit: number,
      token: number,
      action: number,
      failure: string | null,
      subject = "saved",
    ) => {
      const recovered = await readRefreshInterval();
      if (edit !== refreshEdit.current) return;
      if (recovered === null) {
        settleScopeCheck("refreshIntervalMinutes", token, false);
        if (failure) {
          publishStartupError(
            "refreshInterval",
            `${reasonSentence(failure)} The refresh interval could not be ${subject} and its saved state could not be verified.`,
            action,
          );
        }
        return;
      }
      settleScopeCheck("refreshIntervalMinutes", token, true);
      setSettings((prev) => {
        if (prev.refreshIntervalMinutes === recovered) return prev;
        const next = { ...prev, refreshIntervalMinutes: recovered };
        saveSettings(next);
        return next;
      });
      if (failure) {
        publishStartupError(
          "refreshInterval",
          `${reasonSentence(failure)} The refresh interval could not be ${subject}; it is currently ${recovered} minutes.`,
          action,
        );
      }
    },
    [publishStartupError, readRefreshInterval, settleScopeCheck],
  );

  const operationalRevision = useRef(-1);
  // Every explicit operational edit bumps this counter. A reset that is
  // already recovering must not push an older runtime snapshot over a newer
  // user choice; the counter is checked again inside the state updater.
  const operationalEdit = useRef(0);
  const applyOperational = useCallback(
    (
      value: OperationalSettings,
      options?: { persistCache?: boolean; fence?: number },
    ) => {
      if (!value || !Number.isFinite(value.revision)) return;
      const fence = options?.fence;
      if (fence !== undefined && operationalEdit.current !== fence) return;
      if (value.revision < operationalRevision.current) return;
      operationalRevision.current = value.revision;
      setSettings(prev => {
        // A user edit may start while this reply is in flight. The edit owns
        // React and the local cache from that point on.
        if (fence !== undefined && operationalEdit.current !== fence) return prev;
        const next = {
          ...prev,
          quotaNotifications: value.quotaNotifications,
          walletPreferences: { ...prev.walletPreferences, monitored: value.monitored },
        };
        // Startup/event projections are read-only copies; explicit writes and
        // reset recovery must keep the persisted WebView cache in step.
        if (options?.persistCache) saveSettings(next);
        return next;
      });
    },
    [],
  );
  const queueRuntimePreferenceWrite = useCallback(
    <T>(write: () => Promise<T>): Promise<T> => {
      const queued = runtimePreferenceWrites.current.then(write);
      // A failed operation must not poison every later preference edit.
      runtimePreferenceWrites.current = queued.then(
        (): void => undefined,
        (): void => undefined,
      );
      return queued;
    },
    [],
  );
  /** The canonical operational snapshot, or null when Rust cannot be read. */
  const readOperationalSettings = useCallback(
    async (): Promise<OperationalSettings | null> => {
      try {
        const value = await invoke<OperationalSettings>("get_operational_settings");
        return value && Number.isFinite(value.revision) ? value : null;
      } catch {
        return null;
      }
    },
    [],
  );
  /**
   * Writes the operational preferences Rust owns (notifications, wallet
   * monitoring). The reply is the canonical state, so it confirms the scopes
   * this write owns - including a scope an earlier failure left unknown. A
   * rejected write may still have persisted, so only a fresh read settles the
   * scopes; without one they stay explicitly unknown.
   */
  const writeOperational = useCallback((
    command: string,
    args: Record<string, unknown>,
    scopes: readonly PreferenceScope[],
  ) => {
    const action = beginPreferenceAction();
    clearStartupError("operational", action);
    operationalEdit.current += 1;
    const fence = operationalEdit.current;
    // The newest attempt owns the scopes; an older verdict is stale. The
    // scopes leave the confirmed set only if this attempt cannot be verified.
    const tokens = scopes.map((scope) => [scope, beginScopeCheck(scope)] as const);
    void queueRuntimePreferenceWrite(async () => {
      try {
        const value = await invoke<OperationalSettings>(command, args);
        applyOperational(value, { persistCache: true });
        for (const [scope, token] of tokens) settleScopeCheck(scope, token, true);
        clearStartupError("operational", action);
      } catch (error) {
        console.error("Failed to save an operational preference", error);
        const recovered = await readOperationalSettings();
        if (fence === operationalEdit.current && recovered) {
          // A newer explicit edit owns the live state; this read would be
          // older evidence and applyOperational's fence drops it.
          applyOperational(recovered, { persistCache: true, fence });
          for (const [scope, token] of tokens) settleScopeCheck(scope, token, true);
        } else {
          for (const [scope, token] of tokens) settleScopeCheck(scope, token, false);
        }
        // Only the newest operational edit owns the error state: an older
        // failure that lands after a newer edit took over must not erase the
        // newer result or its message.
        if (fence === operationalEdit.current) {
          publishStartupError("operational", OPERATIONAL_SAVE_ERROR, action);
        }
      }
    });
  }, [
    applyOperational,
    beginPreferenceAction,
    beginScopeCheck,
    clearStartupError,
    publishStartupError,
    queueRuntimePreferenceWrite,
    readOperationalSettings,
    settleScopeCheck,
  ]);
  useEffect(() => {
    if (!isRunningInTauri()) return;
    let cancelled = false;
    const subscription = listen<OperationalSettings>("operational-settings-changed", event => {
      if (!cancelled) applyOperational(event.payload);
    });
    void subscription.then(() => invoke<OperationalSettings>("get_operational_settings"))
      .then(value => { if (!cancelled) applyOperational(value); })
      .catch(() => {
        if (!cancelled) {
          publishStartupErrorIfEmpty(
            "load",
            "Could not load saved monitoring preferences.",
          );
        }
      });
    return () => { cancelled = true; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [applyOperational, publishStartupErrorIfEmpty]);

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
        publishStartupErrorIfEmpty("load", accountLoadErrorText(error));
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
  }, [publishStartupErrorIfEmpty]);

  // The OS autostart state is authoritative over the stored preference:
  // reconcile once at startup in case the entry was changed or removed
  // outside the app (e.g. Task Manager's Startup tab).
  useEffect(() => {
    if (!isRunningInTauri()) return;
    let cancelled = false;
    const token = beginScopeCheck("launchAtStartup");
    isEnabled()
      .then((enabled) => {
        if (cancelled) return;
        // The OS answered, so the stored preference can be compared with it.
        settleScopeCheck("launchAtStartup", token, true);
        setSettings((prev) => {
          if (prev.launchAtStartup === enabled) return prev;
          const next = { ...prev, launchAtStartup: enabled };
          saveSettings(next);
          return next;
        });
      })
      .catch(() => {
        if (cancelled) return;
        // The plugin is only consulted to correct drift; a failed probe
        // leaves the stored preference untouched, but it also means the OS
        // state is unconfirmed. The scope stays explicitly unknown instead
        // of presenting the stored value as the saved one.
        settleScopeCheck("launchAtStartup", token, false);
      });
    return () => {
      cancelled = true;
    };
  }, [beginScopeCheck, settleScopeCheck]);

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
    if (!isRunningInTauri()) {
      // Plain-browser dev: remember the preference without OS interaction.
      setSettings((prev) => {
        const next = { ...prev, launchAtStartup };
        saveSettings(next);
        return next;
      });
      return;
    }
    const action = beginPreferenceAction();
    // Only this action's own message is dropped. A concurrent action may
    // not clear another action's error or its unresolved state.
    clearStartupError("launchAtStartup", action);
    setStartupPending(true);
    const token = beginScopeCheck("launchAtStartup");
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
      // The OS accepted the change, so the scope is confirmed again.
      settleScopeCheck("launchAtStartup", token, true);
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      // The setter may have failed before or after the OS applied the
      // change, so the last value this WebView saw proves nothing. Only a
      // fresh read of the OS state tells what is actually saved.
      const recovered = await readAutostartState();
      if (recovered === null) {
        // Neither the write nor a read produced evidence: the scope stays
        // explicitly unknown and the UI says so.
        settleScopeCheck("launchAtStartup", token, false);
        setSettings((prev) => {
          const next = { ...prev, launchAtStartup: previous };
          saveSettings(next);
          return next;
        });
        publishStartupError(
          "launchAtStartup",
          `${reasonSentence(reason)} The launch-at-startup setting could not be applied and its saved state could not be verified.`,
          action,
        );
        return;
      }
      settleScopeCheck("launchAtStartup", token, true);
      // The canonical state is known, so it replaces the failed intent.
      setSettings((prev) => {
        if (prev.launchAtStartup === recovered) return prev;
        const next = { ...prev, launchAtStartup: recovered };
        saveSettings(next);
        return next;
      });
      publishStartupError(
        "launchAtStartup",
        `${reasonSentence(reason)} The launch-at-startup setting could not be applied; it is currently ${recovered ? "on" : "off"}.`,
        action,
      );
    } finally {
      setStartupPending(false);
    }
  }, [
    beginPreferenceAction,
    beginScopeCheck,
    clearStartupError,
    publishStartupError,
    readAutostartState,
    settleScopeCheck,
  ]);

  const setRefreshInterval = useCallback(
    (refreshIntervalMinutes: RefreshIntervalMinutes) => {
      const action = beginPreferenceAction();
      const edit = ++refreshEdit.current;
      // Only this action's own message is dropped.
      clearStartupError("refreshInterval", action);
      setSettings((prev) => {
        if (prev.refreshIntervalMinutes === refreshIntervalMinutes) return prev;
        const next = { ...prev, refreshIntervalMinutes };
        saveSettings(next);
        return next;
      });
      if (!isRunningInTauri()) return;
      const token = beginScopeCheck("refreshIntervalMinutes");
      // The timer belongs to the shared runtime, so this write joins the queue
      // that orders reset and the other Rust-owned preferences. Otherwise an
      // effect or the floating window could land the interval after a reset
      // default and leave Rust on a different cadence than the UI and cache.
      void queueRuntimePreferenceWrite(() =>
        invoke<void>("set_refresh_interval", { minutes: refreshIntervalMinutes }),
      ).then(
        () => {
          settleScopeCheck("refreshIntervalMinutes", token, true);
          clearStartupError("refreshInterval", action);
        },
        (error) => {
          console.error("Failed to save the refresh interval", error);
          const reason = error instanceof Error ? error.message : String(error);
          void recoverRefreshInterval(edit, token, action, reason);
        },
      );
    },
    [
      beginPreferenceAction,
      beginScopeCheck,
      clearStartupError,
      queueRuntimePreferenceWrite,
      recoverRefreshInterval,
      settleScopeCheck,
    ],
  );

  const setTheme = useCallback((theme: Theme) => {
    setSettings((prev) => {
      if (prev.theme === theme) return prev;
      const next = { ...prev, theme };
      saveSettings(next);
      return next;
    });
  }, []);

  const refreshIntervalSynced = useRef(false);
  /**
   * A failed startup hand-off leaves the persisted choice intact. The runtime
   * may still be on a transient default, so its snapshot is not proof of the
   * saved preference and must not be copied back into React or the cache.
   */
  const recoverStartupRefreshInterval = useCallback(
    (edit: number, token: number, action: number, reason: string) => {
      if (edit !== refreshEdit.current) return;
      refreshIntervalSynced.current = false;
      settleScopeCheck("refreshIntervalMinutes", token, false);
      publishStartupError(
        "refreshInterval",
        `${reasonSentence(reason)} The refresh interval could not be handed over to the runtime and its saved runtime state could not be verified. Retry the runtime sync.`,
        action,
      );
    },
    [publishStartupError, settleScopeCheck],
  );
  /**
   * The one startup hand-off of the persisted interval to the shared runtime.
   * Only the window that owns preference editing calls this: the floating
   * window is a read-only view, and two windows forwarding their own copies
   * would let the slower - possibly stale - write decide the cadence.
   */
  const syncRefreshInterval = useCallback(() => {
    if (!isRunningInTauri() || refreshIntervalSynced.current) return;
    refreshIntervalSynced.current = true;
    const action = beginPreferenceAction();
    clearStartupError("refreshInterval", action);
    const token = beginScopeCheck("refreshIntervalMinutes");
    // The hand-off is an edit like any other: a later explicit change owns
    // the value, so the recovery of a failed hand-off must not roll it back.
    const edit = refreshEdit.current;
    const minutes = settingsRef.current.refreshIntervalMinutes;
    void queueRuntimePreferenceWrite(() =>
      invoke<void>("set_refresh_interval", {
        minutes,
      }),
    ).then(
      () => {
        settleScopeCheck("refreshIntervalMinutes", token, true);
        clearStartupError("refreshInterval", action);
      },
      (error) => {
        console.error("Failed to hand the refresh interval to the runtime", error);
        const reason = error instanceof Error ? error.message : String(error);
        recoverStartupRefreshInterval(edit, token, action, reason);
      },
    );
  }, [
    beginPreferenceAction,
    beginScopeCheck,
    clearStartupError,
    queueRuntimePreferenceWrite,
    recoverStartupRefreshInterval,
    settleScopeCheck,
  ]);

  const setQuotaNotifications = useCallback((quotaNotifications: boolean) => {
    if (isRunningInTauri()) {
      writeOperational(
        "set_quota_notifications_enabled",
        { enabled: quotaNotifications },
        ["quotaNotifications"],
      );
      return;
    }
    setSettings((prev) => {
      if (prev.quotaNotifications === quotaNotifications) return prev;
      const next = { ...prev, quotaNotifications };
      saveSettings(next);
      return next;
    });
  }, [writeOperational]);

  const setUsageIntelligence = useCallback((usageIntelligence: boolean) => {
    const action = beginPreferenceAction();
    clearStartupError("usageIntelligence", action);
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
      void queueRuntimePreferenceWrite(async () => {
        await invoke<void>("set_usage_intelligence_enabled", {
          enabled: usageIntelligence,
        });
        usageCommitted.current = usageIntelligence;
        clearStartupError("usageIntelligence", action);
      })
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
          publishStartupError(
            "usageIntelligence",
            persisted ? USAGE_SAVE_ERROR : USAGE_SAVE_ERROR + LOCAL_CACHE_NOTE,
            action,
          );
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
                publishStartupError(
                  "usageIntelligence",
                  USAGE_SAVE_ERROR + LOCAL_CACHE_NOTE,
                  action,
                );
              }
            })
            .catch(() => {});
        })
        .finally(settled);
    }
  }, [
    beginPreferenceAction,
    clearStartupError,
    publishStartupError,
    queueRuntimePreferenceWrite,
  ]);

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
      if (isRunningInTauri()) {
        writeOperational(
          "set_wallet_monitoring",
          { monitored: walletPreferences.monitored },
          ["walletMonitoring"],
        );
      }
      setSettings((prev) => {
        const next = { ...prev, walletPreferences: isRunningInTauri() ? { ...walletPreferences, monitored: prev.walletPreferences.monitored } : walletPreferences };
        saveSettings(next);
        return next;
      });
    },
    [writeOperational],
  );

  /**
   * Turns one wallet's automatic balance collection on or off. The runtime
   * enforces the flag (its own persisted mirror is loaded before the first
   * collection cycle); a disabled wallet keeps its last known balance and
   * never loses historical data.
   */
  const setWalletMonitored = useCallback((walletId: string, monitored: boolean) => {
    if (isRunningInTauri()) {
      writeOperational(
        "set_wallet_monitoring",
        { monitored: { [walletId]: monitored } },
        ["walletMonitoring"],
      );
      return;
    }
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
      const action = beginPreferenceAction();
      clearStartupError("accounts", action);
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
      void queueRuntimePreferenceWrite(
        async () => {
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
          clearStartupError("accounts", action);
        },
      )
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
            // The safe schema diagnostic explains why saving is refused; a
            // later generic message would hide that cause. Load diagnostics
            // remain outside preference-action generations.
            const currentOwner = startupErrorOwner.current;
            const currentMessage = startupErrorMessage.current;
            const schemaLoadError =
              currentOwner === "load" &&
              currentMessage !== null &&
              UNSUPPORTED_ACCOUNT_SCHEMA.test(currentMessage);
            if (!schemaLoadError) {
              publishStartupError(
                "accounts",
                persisted
                  ? ACCOUNT_SAVE_ERROR
                  : ACCOUNT_SAVE_ERROR + LOCAL_CACHE_NOTE,
                action,
              );
            }
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
              publishStartupError(
                "accounts",
                ACCOUNT_SAVE_ERROR + LOCAL_CACHE_NOTE,
                action,
              );
            }
          }).catch(() => {});
        })
        .finally(settled);
    },
    [
      beginPreferenceAction,
      clearStartupError,
      publishStartupError,
      queueRuntimePreferenceWrite,
    ],
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
    const resetAction = beginPreferenceAction();
    // Only the previous reset message is dropped: another action's message
    // and its unresolved state belong to that action.
    clearStartupError("reset", resetAction);
    // Reset is also an explicit action. Its versions fence late initialization
    // replies and successful old edits; the shared write tail orders the Rust
    // changes against both earlier writes and newer explicit edits.
    const resetAccountsEdit = ++accountsEdit.current;
    const resetUsageEdit = ++usageEdit.current;
    const resetRefreshEdit = ++refreshEdit.current;
    // The reset verifies every scope, so it owns their verdicts from here on.
    // An action that starts later takes the scope over and this reset's
    // verdict for it is ignored.
    const resetScopeTokens = {
      launchAtStartup: beginScopeCheck("launchAtStartup"),
      quotaNotifications: beginScopeCheck("quotaNotifications"),
      walletMonitoring: beginScopeCheck("walletMonitoring"),
      refreshIntervalMinutes: beginScopeCheck("refreshIntervalMinutes"),
      codexAccounts: beginScopeCheck("codexAccounts"),
      usageIntelligence: beginScopeCheck("usageIntelligence"),
    };
    // An operational edit that starts after this point owns the operational
    // preferences. Recovery must not push an older snapshot over that edit.
    const resetOperationalFence = operationalEdit.current;
    const { settings: defaults } = resetPreferenceStores();
    setSettings(defaults);
    if (!isRunningInTauri()) return { ok: true, removed: true };
    if (resetsPending.current++ === 0) setStartupPending(true);
    const operation = queueRuntimePreferenceWrite(async () => {
      const unverified: PreferenceScope[] = [];
      let autostartError: string | null = null;
      let autostartDetail: string | null = null;
      let runtimeFailed = false;
      try {
        try {
          await disable();
          // The OS entry is off now, so the scope is confirmed again.
          settleScopeCheck(
            "launchAtStartup",
            resetScopeTokens.launchAtStartup,
            true,
          );
        } catch (error) {
          // The WebView stores are already at their defaults. The OS entry
          // keeps whatever state it actually has, but a failed disable must
          // not stop the runtime preferences from being reset and reconciled.
          // The default must not be presented as confirmed either: only a
          // fresh read of the OS state says what is really saved.
          autostartError = error instanceof Error ? error.message : String(error);
          const recovered = await readAutostartState();
          if (recovered === null) {
            unverified.push("launchAtStartup");
            settleScopeCheck(
              "launchAtStartup",
              resetScopeTokens.launchAtStartup,
              false,
            );
            autostartDetail =
              `${reasonSentence(autostartError)} The launch-at-startup entry could not be turned off and its saved state could not be verified.`;
          } else {
            settleScopeCheck(
              "launchAtStartup",
              resetScopeTokens.launchAtStartup,
              true,
            );
            // The canonical OS state replaces the default the reset wrote.
            setSettings((prev) => {
              if (prev.launchAtStartup === recovered) return prev;
              const next = { ...prev, launchAtStartup: recovered };
              saveSettings(next);
              return next;
            });
            autostartDetail =
              `${reasonSentence(autostartError)} The launch-at-startup entry could not be turned off; it is currently ${recovered ? "on" : "off"}.`;
          }
        }
        try {
          // The runtime persists these preferences itself. Each Rust reply is
          // applied as it arrives, so a later failure cannot erase which part
          // of the reset was actually committed.
          const quota = await invoke<OperationalSettings>(
            "set_quota_notifications_enabled",
            { enabled: defaults.quotaNotifications },
          );
          applyOperational(quota, {
            persistCache: true,
            fence: resetOperationalFence,
          });
          settleScopeCheck(
            "quotaNotifications",
            resetScopeTokens.quotaNotifications,
            true,
          );
          await invoke("set_refresh_interval", {
            minutes: defaults.refreshIntervalMinutes,
          });
          settleScopeCheck(
            "refreshIntervalMinutes",
            resetScopeTokens.refreshIntervalMinutes,
            true,
          );
          const wallets = await invoke<OperationalSettings>(
            "set_wallet_monitoring",
            { monitored: defaults.walletPreferences.monitored },
          );
          applyOperational(wallets, {
            persistCache: true,
            fence: resetOperationalFence,
          });
          settleScopeCheck(
            "walletMonitoring",
            resetScopeTokens.walletMonitoring,
            true,
          );
          await invoke("set_codex_accounts", {
            accounts: defaults.codexAccounts.accounts,
          });
          accountsCommitted.current = defaults.codexAccounts.accounts;
          settleScopeCheck(
            "codexAccounts",
            resetScopeTokens.codexAccounts,
            true,
          );
          await invoke("set_usage_intelligence_enabled", {
            enabled: defaults.usageIntelligence,
          });
          usageCommitted.current = defaults.usageIntelligence;
          settleScopeCheck(
            "usageIntelligence",
            resetScopeTokens.usageIntelligence,
            true,
          );
        } catch (error) {
          runtimeFailed = true;
          console.error("Failed to push the reset preferences to the runtime", error);

          // A reset can fail after one or more setters committed. Re-read every
          // Rust-owned preference in parallel, then restore React and the local
          // cache from the values that were actually read. A setter may have
          // persisted and then rejected, so any last-seen "committed" value is
          // stale evidence; a failed read leaves that scope explicitly unknown.
          // The edit versions ensure a newer user action remains authoritative
          // while this recovery is pending.
          const [operational, accounts, usage, snapshot] = await Promise.allSettled([
            invoke<OperationalSettings>("get_operational_settings"),
            invoke<CodexAccountSettings[]>("get_codex_accounts"),
            invoke<{ enabled: boolean }>("get_usage_intelligence", {
              query: { range: "today" },
            }),
            invoke<{ refreshIntervalMinutes: number }>("get_runtime_snapshot"),
          ]);

          if (operational.status === "fulfilled") {
            applyOperational(operational.value, {
              persistCache: true,
              fence: resetOperationalFence,
            });
            settleScopeCheck(
              "quotaNotifications",
              resetScopeTokens.quotaNotifications,
              true,
            );
            settleScopeCheck(
              "walletMonitoring",
              resetScopeTokens.walletMonitoring,
              true,
            );
          } else {
            unverified.push("quotaNotifications", "walletMonitoring");
            settleScopeCheck(
              "quotaNotifications",
              resetScopeTokens.quotaNotifications,
              false,
            );
            settleScopeCheck(
              "walletMonitoring",
              resetScopeTokens.walletMonitoring,
              false,
            );
          }

          if (accounts.status === "fulfilled") {
            accountsCommitted.current = accounts.value;
            settleScopeCheck(
              "codexAccounts",
              resetScopeTokens.codexAccounts,
              true,
            );
          } else {
            unverified.push("codexAccounts");
            settleScopeCheck(
              "codexAccounts",
              resetScopeTokens.codexAccounts,
              false,
            );
          }
          if (usage.status === "fulfilled") {
            usageCommitted.current = usage.value.enabled;
            settleScopeCheck(
              "usageIntelligence",
              resetScopeTokens.usageIntelligence,
              true,
            );
          } else {
            unverified.push("usageIntelligence");
            settleScopeCheck(
              "usageIntelligence",
              resetScopeTokens.usageIntelligence,
              false,
            );
          }

          const recoveredRefresh = snapshot.status === "fulfilled" &&
            REFRESH_INTERVAL_MINUTES.some(
              (minutes) => minutes === snapshot.value.refreshIntervalMinutes,
            )
            ? snapshot.value.refreshIntervalMinutes as RefreshIntervalMinutes
            : null;
          if (recoveredRefresh === null) {
            unverified.push("refreshIntervalMinutes");
            settleScopeCheck(
              "refreshIntervalMinutes",
              resetScopeTokens.refreshIntervalMinutes,
              false,
            );
          } else {
            settleScopeCheck(
              "refreshIntervalMinutes",
              resetScopeTokens.refreshIntervalMinutes,
              true,
            );
          }

          setSettings((prev) => {
            let next = prev;
            if (
              accountsEdit.current === resetAccountsEdit &&
              accounts.status === "fulfilled" &&
              next.codexAccounts.accounts !== accounts.value
            ) {
              next = { ...next, codexAccounts: { accounts: accounts.value } };
            }
            if (
              usageEdit.current === resetUsageEdit &&
              usage.status === "fulfilled" &&
              next.usageIntelligence !== usage.value.enabled
            ) {
              next = { ...next, usageIntelligence: usage.value.enabled };
            }
            if (
              refreshEdit.current === resetRefreshEdit &&
              recoveredRefresh !== null &&
              next.refreshIntervalMinutes !== recoveredRefresh
            ) {
              next = { ...next, refreshIntervalMinutes: recoveredRefresh };
            }
            if (next === prev) return prev;
            saveSettings(next);
            return next;
          });
        }

        const unknownScopes = [...new Set(unverified)];
        if (runtimeFailed) {
          resetFailureRef.current = { runtimeFailed: true, autostartDetail };
          publishStartupError(
            "reset",
            resetFailureMessage(autostartDetail, unknownScopes),
            resetAction,
          );
          return { ok: false } as const;
        }
        if (autostartError) {
          // The runtime reset completed; only the OS entry is unresolved. Keep
          // the original scoped explanation the user can act on.
          resetFailureRef.current = { runtimeFailed: false, autostartDetail };
          publishStartupError(
            "reset",
            autostartDetail ?? autostartError,
            resetAction,
          );
          return { ok: false } as const;
        }
        resetFailureRef.current = { runtimeFailed: false, autostartDetail: null };
        clearSupersededPreferenceErrors(resetAction);
        clearStartupError("reset", resetAction);
        return { ok: true, removed: true } as const;
      } finally {
        if (--resetsPending.current === 0) setStartupPending(false);
      }
    });
    try {
      return await operation;
    } catch (error) {
      console.error("Failed to coordinate the preference reset", error);
      for (const scope of ALL_PREFERENCE_SCOPES) {
        settleScopeCheck(scope, resetScopeTokens[scope], false);
      }
      resetFailureRef.current = { runtimeFailed: true, autostartDetail: null };
      publishStartupError(
        "reset",
        "Preferences could not be reset. The saved state of every preference could not be verified. Please retry.",
        resetAction,
      );
      return { ok: false };
    }
  }, [
    applyOperational,
    beginPreferenceAction,
    beginScopeCheck,
    clearStartupError,
    clearSupersededPreferenceErrors,
    publishStartupError,
    queueRuntimePreferenceWrite,
    readAutostartState,
    settleScopeCheck,
  ]);

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
    syncRefreshInterval,
    startupPending,
    startupError,
    unverifiedPreferences,
  };
}


