import { useCallback, useState } from "react";
import { LocalDataSection } from "../LocalDataSection";
import {
  UsageExportSection,
  type UsageExportRequest,
} from "../UsageExportSection";
import { UpdatesSection } from "./UpdatesSection";
import { exportDiagnostics } from "../../lib/diagnosticsExport";
import {
  exportUsageHistory,
  pickUsageExportDirectory,
  type UsageExportFormat,
  type UsageExportResult,
} from "../../lib/usageExport";
import {
  clearProviderCache,
  clearUsageIntelligenceStore,
} from "../../lib/localData";
import { shortProviderName } from "../../lib/dashboard";
import { fullDisplayOrder } from "../../lib/providerPreferences";
import {
  REFRESH_INTERVAL_MINUTES,
  QUOTA_PERSPECTIVES,
  THEMES,
  THEME_LABELS,
  type RefreshIntervalMinutes,
  type Settings,
  type Theme,
} from "../../lib/settings";
import type { ProviderUsage } from "../../types";
import type { ReleaseNote, SafeUpdateError, UpdatePhase } from "../../lib/updater";
import type { LocalDataClearResult } from "../../lib/localData";

/**
 * The Settings destination of the Usage-first main window: the long bottom
 * drawer became a proper page under the primary navigation, organized into
 * four scannable sections — General, Providers & Wallets, Data & Privacy,
 * Support. Every functional setting and action keeps its exact behavior
 * (same controls, same confirmations, same callbacks); only the structure
 * changed.
 *
 * Section-owned transient state (diagnostics and export outcomes) lives here,
 * on the page it belongs to, instead of the app shell.
 */
export type SettingsViewProps = {
  settings: Settings;
  setLaunchAtStartup: (launchAtStartup: boolean) => void;
  setRefreshInterval: (minutes: RefreshIntervalMinutes) => void;
  setTheme: (theme: Theme) => void;
  setQuotaNotifications: (enabled: boolean) => void;
  setUsageIntelligence: (enabled: boolean) => void;
  setQuotaPerspective: (perspective: Settings["quotaPerspective"]) => void;
  setProviderHidden: (
    providerId: string,
    hidden: boolean,
    registryIds: readonly string[],
  ) => boolean;
  moveProvider: (
    providerId: string,
    direction: 1 | -1,
    registryIds: readonly string[],
  ) => boolean;
  resetPreferences: () => Promise<LocalDataClearResult> | LocalDataClearResult;
  startupPending: boolean;
  startupError: string | null;
  /** Full provider registry (hidden providers keep their preferences rows). */
  usages: readonly ProviderUsage[];
  /** Visible providers, for the hidden-state switch. */
  visibleUsages: readonly ProviderUsage[];
  registryIds: readonly string[];
  onClearUsageHistory: () => Promise<LocalDataClearResult> | LocalDataClearResult;
  /** Clears the owned store and syncs the app's in-memory run state. */
  onClearExecutionRuns: () => Promise<LocalDataClearResult> | LocalDataClearResult;
  activeExecutionRun: boolean;
  historyUnavailable: boolean;
  updatePhase: UpdatePhase;
  currentVersion: string | null;
  availableUpdate: ReleaseNote | null;
  updateError: SafeUpdateError | null;
  onCheckForUpdates: () => void;
  onInstallUpdate: () => void;
  onDismissUpdate: () => void;
};

export function SettingsView({
  settings,
  setLaunchAtStartup,
  setRefreshInterval,
  setTheme,
  setQuotaNotifications,
  setUsageIntelligence,
  setQuotaPerspective,
  setProviderHidden,
  moveProvider,
  resetPreferences,
  startupPending,
  startupError,
  usages,
  visibleUsages,
  registryIds,
  onClearUsageHistory,
  onClearExecutionRuns,
  activeExecutionRun,
  historyUnavailable,
  updatePhase,
  currentVersion,
  availableUpdate,
  updateError,
  onCheckForUpdates,
  onInstallUpdate,
  onDismissUpdate,
}: SettingsViewProps) {
  // Diagnostics export: transient page-local feedback only.
  const [diagnosticsSaving, setDiagnosticsSaving] = useState(false);
  const [diagnosticsMessage, setDiagnosticsMessage] = useState<string | null>(
    null,
  );
  const [diagnosticsError, setDiagnosticsError] = useState<string | null>(null);

  const [usageExporting, setUsageExporting] = useState<UsageExportFormat | null>(
    null,
  );
  const [usageExportMessage, setUsageExportMessage] = useState<string | null>(
    null,
  );
  const [usageExportError, setUsageExportError] = useState<string | null>(null);

  // Transient feedback when hiding the final visible provider is refused.
  const [providerPrefsNote, setProviderPrefsNote] = useState<string | null>(null);

  const handleDiagnosticsExport = useCallback(async () => {
    setDiagnosticsSaving(true);
    setDiagnosticsMessage(null);
    setDiagnosticsError(null);
    try {
      const result = await exportDiagnostics(settings);
      if (result.status === "saved") {
        setDiagnosticsMessage(
          result.fileName
            ? `Saved ${result.fileName}`
            : "Diagnostics saved",
        );
      }
    } catch {
      setDiagnosticsError("Could not export diagnostics.");
    } finally {
      setDiagnosticsSaving(false);
    }
  }, [settings]);

  const handleUsageExportPick = useCallback(async (): Promise<string | null> => {
    try {
      return await pickUsageExportDirectory();
    } catch {
      // A picker failure settles like a cancel; the chooser stays usable.
      return null;
    }
  }, []);

  const handleUsageExport = useCallback(
    async (request: UsageExportRequest): Promise<UsageExportResult> => {
      setUsageExporting(request.format);
      setUsageExportMessage(null);
      setUsageExportError(null);
      try {
        return await exportUsageHistory(request.format, {
          directory: request.directory,
          fileName: request.fileName,
          confirmOverwrite: request.confirmOverwrite,
        });
      } catch (cause) {
        // The sanitized page alert is the only failure surface; raw
        // OS/provider errors never reach the UI.
        setUsageExportError("Could not export usage history.");
        throw cause;
      } finally {
        setUsageExporting(null);
      }
    },
    [],
  );

  const handleUsageExportSaved = useCallback((fileName?: string) => {
    setUsageExportMessage(
      fileName ? `Saved ${fileName}` : "Usage history saved",
    );
  }, []);

  const dismissUsageExportOutcome = useCallback(() => {
    setUsageExportMessage(null);
    setUsageExportError(null);
  }, []);

  const clearProviderCacheAction = useCallback(
    () => clearProviderCache(),
    [],
  );
  const clearUsageIntelligenceData = useCallback(
    () => clearUsageIntelligenceStore(),
    [],
  );

  return (
    <div className="settings-view">
      <section className="settings-section" aria-labelledby="settings-general">
        <h3 className="settings-section-heading" id="settings-general">
          General
        </h3>
        <label className="setting-row">
          <span className="setting-label">
            Launch LimitScope when Windows starts
          </span>
          <input
            type="checkbox"
            role="switch"
            checked={settings.launchAtStartup}
            disabled={startupPending}
            onChange={(event) => void setLaunchAtStartup(event.target.checked)}
          />
        </label>
        <label className="setting-row">
          <span className="setting-label">Quota notifications</span>
          <input
            type="checkbox"
            role="switch"
            checked={settings.quotaNotifications}
            onChange={(event) => setQuotaNotifications(event.target.checked)}
          />
        </label>
        <fieldset className="setting-row setting-fieldset">
          <legend className="setting-label">Quota display</legend>
          <div className="quota-perspective-control" role="radiogroup" aria-label="Quota display">
            {QUOTA_PERSPECTIVES.map((perspective) => (
              <label key={perspective} className="quota-perspective-option">
                <input
                  type="radio"
                  name="quota-perspective"
                  value={perspective}
                  checked={settings.quotaPerspective === perspective}
                  onChange={() => setQuotaPerspective(perspective)}
                />
                <span>{perspective === "used" ? "Used" : "Remaining"}</span>
              </label>
            ))}
          </div>
        </fieldset>
        <label className="setting-row">
          <span className="setting-label">Auto-refresh</span>
          <select
            aria-label="Auto-refresh"
            value={settings.refreshIntervalMinutes}
            onChange={(event) =>
              setRefreshInterval(Number(event.target.value) as RefreshIntervalMinutes)
            }
          >
            {REFRESH_INTERVAL_MINUTES.map((minutes) => (
              <option key={minutes} value={minutes}>
                {minutes} min
              </option>
            ))}
          </select>
        </label>
        <label className="setting-row">
          <span className="setting-label">Theme</span>
          <select
            aria-label="Theme"
            value={settings.theme}
            onChange={(event) => setTheme(event.target.value as Theme)}
          >
            {THEMES.map((theme) => (
              <option key={theme} value={theme}>
                {THEME_LABELS[theme]}
              </option>
            ))}
          </select>
        </label>
      </section>

      <section
        className="settings-section"
        aria-labelledby="settings-providers-wallets"
      >
        <h3 className="settings-section-heading" id="settings-providers-wallets">
          Providers &amp; Wallets
        </h3>
        <div className="providers-prefs" role="group" aria-label="Providers">
          <span className="setting-label">Providers</span>
          <p className="providers-prefs-note">
            Visibility and order affect the floating bar. Hidden providers
            keep refreshing, keep their history, and keep notifications.
          </p>
          {usages.length === 0 ? (
            <p className="providers-prefs-note">No providers yet.</p>
          ) : (
            <ul className="providers-prefs-list">
              {fullDisplayOrder(registryIds, settings.providerPreferences).map(
                (providerId, index, all) => {
                  const usageForRow = usages.find((usage) => usage.id === providerId);
                  if (!usageForRow) return null;
                  const hidden = !visibleUsages.some((usage) => usage.id === providerId);
                  const label = shortProviderName(usageForRow);
                  return (
                    <li key={providerId} className="provider-prefs-row">
                      <span className="provider-prefs-name">{label}</span>
                      <label className="provider-prefs-visibility">
                        <input
                          type="checkbox"
                          role="switch"
                          aria-label={"Show " + usageForRow.name + " in the floating bar"}
                          checked={!hidden}
                          onChange={(event) => {
                            const ok = setProviderHidden(
                              providerId,
                              !event.target.checked,
                              registryIds,
                            );
                            setProviderPrefsNote(
                              ok ? null : "At least one provider stays visible.",
                            );
                          }}
                        />
                      </label>
                      <span className="provider-prefs-moves" role="group" aria-label={"Reorder " + label}>
                        <button
                          type="button"
                          aria-label={"Move " + label + " up"}
                          disabled={index === 0}
                          onClick={() => {
                            moveProvider(providerId, -1, registryIds);
                          }}
                        >
                          ↑
                        </button>
                        <button
                          type="button"
                          aria-label={"Move " + label + " down"}
                          disabled={index === all.length - 1}
                          onClick={() => {
                            moveProvider(providerId, 1, registryIds);
                          }}
                        >
                          ↓
                        </button>
                      </span>
                    </li>
                  );
                },
              )}
            </ul>
          )}
          {providerPrefsNote ? (
            <p className="setting-error" role="status">
              {providerPrefsNote}
            </p>
          ) : null}
        </div>
        <div className="wallet-prefs" role="group" aria-label="API wallets">
          <span className="setting-label">API wallets</span>
          <p className="providers-prefs-note">
            Wallet balances live in the floating bar. Per-wallet monitor and
            floating-bar controls arrive with the API wallet feature.
          </p>
        </div>
      </section>

      <section
        className="settings-section"
        aria-labelledby="settings-data-privacy"
      >
        <h3 className="settings-section-heading" id="settings-data-privacy">
          Data &amp; Privacy
        </h3>
        <label className="setting-row">
          <span className="setting-copy">
            <span className="setting-label">Usage Intelligence</span>
            <span className="setting-note">
              Reads local AI tool usage metadata (models and token counts)
              from tools like ZCode, on this device only. Never reads
              prompt or response content.
            </span>
          </span>
          <input
            type="checkbox"
            role="switch"
            aria-label="Usage Intelligence"
            checked={settings.usageIntelligence === true}
            onChange={(event) => setUsageIntelligence(event.target.checked)}
          />
        </label>
        <LocalDataSection
          onClearUsageHistory={onClearUsageHistory}
          onClearProviderCache={clearProviderCacheAction}
          onClearUsageIntelligence={clearUsageIntelligenceData}
          onClearExecutionRuns={onClearExecutionRuns}
          onResetPreferences={resetPreferences}
          activeExecutionRun={activeExecutionRun}
        />
        <UsageExportSection
          onPickDirectory={handleUsageExportPick}
          onExport={handleUsageExport}
          exporting={usageExporting}
          onOutcomeDismiss={dismissUsageExportOutcome}
          onSaved={handleUsageExportSaved}
        />
        {usageExportMessage ? (
          <p className="setting-success" role="status">
            {usageExportMessage}
          </p>
        ) : null}
        {usageExportError ? (
          <p className="setting-error" role="alert">
            {usageExportError}
          </p>
        ) : null}
      </section>

      <section className="settings-section" aria-labelledby="settings-support">
        <h3 className="settings-section-heading" id="settings-support">
          Support
        </h3>
        <div className="setting-row utility-action">
          <span className="setting-copy">
            <span className="setting-label">Support diagnostics</span>
            <span className="setting-note">Redacted runtime summary</span>
          </span>
          <button
            type="button"
            className="clear-history-btn"
            onClick={() => void handleDiagnosticsExport()}
            disabled={diagnosticsSaving}
          >
            {diagnosticsSaving ? "Exporting..." : "Export diagnostics"}
          </button>
        </div>
        {diagnosticsMessage ? (
          <p className="setting-success" role="status">
            {diagnosticsMessage}
          </p>
        ) : null}
        {diagnosticsError ? (
          <p className="setting-error" role="alert">
            {diagnosticsError}
          </p>
        ) : null}
        {historyUnavailable ? (
          <p className="setting-note" role="status">
            Prediction history is unavailable on this device.
          </p>
        ) : null}
        <UpdatesSection
          phase={updatePhase}
          currentVersion={currentVersion}
          update={availableUpdate}
          error={updateError}
          onCheck={onCheckForUpdates}
          onInstall={onInstallUpdate}
          onDismiss={onDismissUpdate}
        />
        {startupError ? (
          <p className="setting-error" role="alert">
            Could not update the startup setting: {startupError}
          </p>
        ) : null}
      </section>
    </div>
  );
}
