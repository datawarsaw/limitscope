import { useCallback, useEffect, useRef, useState } from "react";
import { AttentionRail } from "./components/dashboard/AttentionRail";
import { ExecutionRunPanel } from "./components/dashboard/ExecutionRunPanel";
import { LocalDataSection } from "./components/LocalDataSection";
import { PrimaryQuotaPanel } from "./components/dashboard/PrimaryQuotaPanel";
import { ProviderRail } from "./components/dashboard/ProviderRail";
import { QuotaWindowList } from "./components/dashboard/QuotaWindowList";
import { UsageView } from "./components/usage/UsageView";
import { useNow } from "./hooks/useNow";
import { useDashboardLayout } from "./hooks/useDashboardLayout";
import { useExecutionRuns } from "./hooks/useExecutionRuns";
import { useQuotaPredictions } from "./hooks/useQuotaPredictions";
import { exportDiagnostics } from "./lib/diagnosticsExport";
import { attachMainWindowBounds } from "./lib/mainWindowBounds";
import {
  exportUsageHistory,
  pickUsageExportDirectory,
  type UsageExportFormat,
  type UsageExportResult,
} from "./lib/usageExport";
import {
  UsageExportSection,
  type UsageExportRequest,
} from "./components/UsageExportSection";
import {
  clearExecutionRuns,
  clearProviderCache,
  clearUsageIntelligenceStore,
} from "./lib/localData";
import { useProviderUsage } from "./hooks/useProviderUsage";
import { useSettings } from "./hooks/useSettings";
import { useUpdater } from "./hooks/useUpdater";
import type { ReleaseNote, SafeUpdateError, UpdatePhase } from "./lib/updater";
import { formatTime } from "./lib/format";
import {
  attentionItems,
  defaultSelectedProviderId,
  shortProviderName,
} from "./lib/dashboard";
import { formatRunElapsed } from "./lib/executionRuns";
import { readDevFixture } from "./dev/usageFixtures";
import {
  fullDisplayOrder,
  orderVisibleProviders,
} from "./lib/providerPreferences";
import {
  REFRESH_INTERVAL_MINUTES,
  QUOTA_PERSPECTIVES,
  THEMES,
  THEME_LABELS,
  type RefreshIntervalMinutes,
  type Theme,
} from "./lib/settings";

function RefreshIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path
        d="M13.5 8a5.5 5.5 0 1 1-1.62-3.9M13.5 1.8v3.2h-3.2"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/**
 * v0.7 main window: primary Overview | Usage navigation over the unchanged
 * three-zone dashboard. Overview keeps the accepted v0.5 composition
 * (provider rail · primary overview + quota windows · needs-attention rail);
 * Usage is one coherent analytics surface over the v0.7 backend projection.
 * App owns only orchestration — runtime data, the selected provider, the
 * active view, and the structural layout — while focused components render
 * each zone. The settings drawer and utility bar keep their accepted compact
 * pattern, and every state semantic (fresh/stale/error/unknown, retained
 * last-good, simulated exclusion, prediction gating) is consumed, not
 * redefined.
 */
/**
 * The Updates settings block. Rendered inline in Settings — a startup check
 * only lights this block up, it never interrupts with a modal. Exported for
 * state-machine tests: each updater phase has a pinned presentation.
 */
export function UpdatesSection({
  phase,
  currentVersion,
  update,
  error,
  onCheck,
  onInstall,
  onDismiss,
}: {
  phase: UpdatePhase;
  currentVersion: string | null;
  update: ReleaseNote | null;
  error: SafeUpdateError | null;
  onCheck: () => void;
  onInstall: () => void;
  onDismiss: () => void;
}) {
  const busy = phase === "checking" || phase === "downloading" || phase === "installing";
  return (
    <div className="updates-block" role="group" aria-label="Updates">
      <span className="updates-title">Updates</span>
      <div className="setting-row">
        <span className="setting-label">Current version</span>
        <span className="update-version">{currentVersion ?? "—"}</span>
      </div>
      <div className="setting-row">
        <span className="setting-copy">
          {phase === "upToDate" ? (
            <span className="setting-note" role="status">
              You're up to date.
            </span>
          ) : null}
          {phase === "available" && update ? (
            <>
              <span className="setting-label" role="status">
                Update available: LimitScope {update.version}
              </span>
              {update.notes ? (
                <span className="setting-note">{update.notes}</span>
              ) : null}
            </>
          ) : null}
          {phase === "downloading" ? (
            <span className="setting-note" role="status">
              Downloading update…
            </span>
          ) : null}
          {phase === "installing" ? (
            <span className="setting-note" role="status">
              Installing update — LimitScope will restart when done.
            </span>
          ) : null}
          {phase === "error" && error ? (
            <span className="setting-error" role="alert" title={error.detail}>
              {error.message}
            </span>
          ) : null}
        </span>
        {phase === "available" && update ? (
          <span className="update-actions">
            <button
              type="button"
              className="update-install-btn"
              onClick={onInstall}
            >
              Update now
            </button>
            <button
              type="button"
              className="update-dismiss-btn"
              onClick={onDismiss}
            >
              Later
            </button>
          </span>
        ) : phase === "downloading" || phase === "installing" ? null : (
          <button
            type="button"
            className="check-updates-btn"
            onClick={onCheck}
            disabled={busy}
          >
            {phase === "checking" ? "Checking…" : "Check for updates"}
          </button>
        )}
      </div>
    </div>
  );
}

export default function App() {
  const {
    settings,
    setLaunchAtStartup,
    setRefreshInterval,
    setTheme,
    setQuotaNotifications,
    setUsageIntelligence,
    setProviderHidden,
    moveProvider,
    setQuotaPerspective,
    resetPreferences,
    startupPending,
    startupError,
  } = useSettings();
  const {
    usages: runtimeUsages,
    loading,
    lastUpdatedAt,
    refresh,
    refreshOverdue,
    stale,
    staleMinutes,
    historyRevision,
    usageIntelligenceRevision,
  } =
    useProviderUsage(settings.refreshIntervalMinutes);
  // Dev-only visual fixtures (`?fixture=…` in `npm run dev`); null in any
  // production or test run. See src/dev/usageFixtures.ts.
  const devFixture =
    import.meta.env.DEV && typeof window !== "undefined"
      ? readDevFixture(window.location.search)
      : null;
  const usages = devFixture?.usages ?? runtimeUsages;
  // Local wall clock for the reset countdowns; no provider refetch involved.
  const nowMs = useNow();
  const now = new Date(nowMs);
  const { predictionFor, historyUnavailable, clearLocalHistory } =
    useQuotaPredictions(usages, nowMs, historyRevision);
  const layout = useDashboardLayout();
  const {
    phase: updatePhase,
    update: availableUpdate,
    error: updateError,
    currentVersion,
    checkForUpdates,
    installUpdate,
    dismissUpdate,
  } = useUpdater();

  // Bottom settings drawer: collapsed by default so the main window stays
  // short. The utility bar always shows the freshest update time plus the
  // current auto-refresh interval.
  const [settingsOpen, setSettingsOpen] = useState(false);
  // A01/K02: the drawer is a nonmodal disclosure — opening moves focus to the
  // first setting, Escape closes and restores the invoker, and closing never
  // strands focus on a detached node (same refocus pattern as the run panel).
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
  const firstSettingRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (settingsOpen) firstSettingRef.current?.focus();
  }, [settingsOpen]);
  const closeSettingsDrawer = useCallback(() => {
    setSettingsOpen(false);
    settingsButtonRef.current?.focus();
  }, []);
  const openSettingsDrawer = useCallback(() => {
    setSettingsOpen(true);
  }, []);
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

  // v0.7 execution runs: a compact toolbar action plus a small panel — no
  // new navigation section. The workflow itself (one active run, bounded
  // recovery, provenance-backed results) lives in useExecutionRuns.
  const {
    activeRun,
    recentRuns,
    startRun,
    finishRun,
    discardRun,
    syncRuns,
  } = useExecutionRuns(usages);
  // Local data "Clear execution runs": wipe the owned store, then drop the
  // in-memory run state so an open panel and the toolbar catch up at once.
  const clearExecutionRunStore = useCallback(async () => {
    const result = await clearExecutionRuns({
      deliberateActiveConfirmation: true,
    });
    if (result.ok) syncRuns();
    return result;
  }, [syncRuns]);
  // Local data "Clear Usage Intelligence": wipes the owned token-usage
  // events and cursors only. While the source stays enabled, collection
  // re-baselines at the current high-water mark — pre-clear usage does
  // not reappear. The ZCode database is never touched.
  const clearUsageIntelligenceData = useCallback(
    () => clearUsageIntelligenceStore(),
    [],
  );
  const [runPanelOpen, setRunPanelOpen] = useState(false);
  const runButtonRef = useRef<HTMLButtonElement>(null);
  const closeRunPanel = useCallback(() => {
    setRunPanelOpen(false);
    runButtonRef.current?.focus();
  }, []);

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
        // The sanitized drawer alert is the only failure surface; raw
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
  // Transient feedback when hiding the final visible provider is refused.
  const [providerPrefsNote, setProviderPrefsNote] = useState<string | null>(null);

  // Presentation order: canonical registry (snapshot) order shaped by the
  // visibility/order preferences. Everything that navigates, selects, or
  // summarizes providers reads this list - never the raw snapshot - while
  // runtime-owned concerns (history, predictions, notifications) keep the
  // full `usages` above.
  const registryIds = usages.map((usage) => usage.id);
  const visibleUsages = orderVisibleProviders(
    usages,
    (usage) => usage.id,
    registryIds,
    settings.providerPreferences,
  );

  // Provider selection: null until the user picks. Until then — and whenever
  // the picked provider is not in the snapshot — the deterministic default
  // anchors the view (highest usable primary percent, the quota strip's
  // concept). An explicit selection sticks across refreshes; it is view
  // state only and deliberately not persisted.
  const [selectedByUser, setSelectedByUser] = useState<string | null>(null);
  // Default selection considers only visible providers; a selection that
  // becomes hidden falls back deterministically instead of sticking on an
  // invisible provider.
  const fallbackId = defaultSelectedProviderId(visibleUsages);
  const selectedId =
    selectedByUser !== null && visibleUsages.some((usage) => usage.id === selectedByUser)
      ? selectedByUser
      : fallbackId;
  const selectedUsage = visibleUsages.find((usage) => usage.id === selectedId);
  const selectProvider = (providerId: string) => setSelectedByUser(providerId);

  // Primary product navigation: Overview | Usage. Usage keeps its own scope
  // (all providers by default; the rail narrows it); a rail pick in Usage
  // also becomes the Overview selection so the provider context is shared.
  const [view, setView] = useState<"overview" | "usage">(() =>
    import.meta.env.DEV &&
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("view") === "usage"
      ? "usage"
      : "overview",
  );
  const [usageScopeProviderId, setUsageScopeProviderId] = useState<string | null>(
    null,
  );
  const selectUsageScope = (providerId: string) => {
    setSelectedByUser(providerId);
    setUsageScopeProviderId(providerId);
  };

  // CSS defaults the document to graphite; this swaps in a stored non-default
  // theme right after mount and keeps the attribute in sync on every switch.
  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme;
  }, [settings.theme]);

  // W01: keep the window inside its monitor's work area at launch, on DPI
  // change, and after monitor loss. Clamp-only, no-op outside Tauri.
  useEffect(() => attachMainWindowBounds(), []);

  // The attention rail is a presentation surface: hidden providers stay out
  // of the rail, but their notification behavior is unchanged (Rust lane).
  // The perspective only reshapes how attention is phrased; canonical
  // usedPercent severity underneath is untouched.
  const attention = attentionItems(
    visibleUsages,
    predictionFor,
    settings.quotaPerspective,
  );

  const updatedLabel = refreshOverdue
    ? "Refresh overdue"
    : stale
      ? lastUpdatedAt
        ? `Last updated ${staleMinutes} min ago`
        : "Last updated —"
      : `Updated ${lastUpdatedAt ? formatTime(lastUpdatedAt.toISOString()) : "—"}`;

  return (
    <div className="app" data-layout={layout} data-view={view}>
      <header className="header">
        <div className="header-lead">
          <h1 className="title">LimitScope</h1>
          <div className="view-nav" role="group" aria-label="View">
            <button
              type="button"
              className="view-nav-btn"
              aria-pressed={view === "overview"}
              onClick={() => setView("overview")}
            >
              Overview
            </button>
            <button
              type="button"
              className="view-nav-btn"
              aria-pressed={view === "usage"}
              onClick={() => setView("usage")}
            >
              Usage
            </button>
          </div>
        </div>
        <div className="header-actions">
          <button
            type="button"
            className={`run-btn${activeRun !== null ? " active" : ""}`}
            onClick={() => setRunPanelOpen((open) => !open)}
            aria-expanded={runPanelOpen}
            aria-controls="execution-run-panel"
            ref={runButtonRef}
            title={
              activeRun !== null
                ? "Execution run in progress"
                : "Start or view an execution run"
            }
            aria-label={
              activeRun !== null
                ? `Execution run in progress, running for ${formatRunElapsed(
                    activeRun.startedAt,
                    nowMs,
                  )}`
                : "Execution run"
            }
          >
            {activeRun !== null ? (
              <>
                <span className="run-btn-dot" aria-hidden="true" />
                {formatRunElapsed(activeRun.startedAt, nowMs)}
              </>
            ) : (
              "Run"
            )}
          </button>
          <button
            type="button"
            className={`refresh-btn${loading ? " spinning" : ""}`}
            onClick={() => void refresh()}
            disabled={loading}
            title="Refresh"
            aria-label="Refresh"
          >
            <RefreshIcon />
          </button>
        </div>
      </header>

      {runPanelOpen ? (
        <ExecutionRunPanel
          activeRun={activeRun}
          recentRuns={recentRuns}
          selectedUsage={selectedUsage}
          usages={usages}
          nowMs={nowMs}
          onStart={startRun}
          onFinish={() => finishRun()}
          onDiscard={discardRun}
          onRefresh={() => void refresh()}
          onClose={closeRunPanel}
        />
      ) : null}

      <div className="dashboard">
        <ProviderRail
          usages={visibleUsages}
          selectedId={view === "usage" ? usageScopeProviderId : selectedId}
          onSelect={view === "usage" ? selectUsageScope : selectProvider}
          orientation={layout === "narrow" ? "horizontal" : "vertical"}
          controlsId={view === "usage" ? "usage-panel" : "provider-panel"}
          perspective={settings.quotaPerspective}
        />

        {view === "usage" ? (
          <main
            id="usage-panel"
            className="provider-main usage-main"
            aria-label="Usage"
          >
            <UsageView
              usages={usages}
              scopeProviderId={usageScopeProviderId}
              onScopeProvider={setUsageScopeProviderId}
              historyRevision={historyRevision}
              perspective={settings.quotaPerspective}
              loader={devFixture?.loader}
              usageIntelligenceEnabled={settings.usageIntelligence === true}
              usageIntelligenceRevision={usageIntelligenceRevision}
              onOpenSettings={openSettingsDrawer}
            />
          </main>
        ) : (
          <>
            <main
              id="provider-panel"
              className="provider-main"
              role="tabpanel"
              aria-labelledby={selectedId ? `provider-tab-${selectedId}` : undefined}
            >
              {selectedUsage ? (
                <>
                  <PrimaryQuotaPanel
                    usage={selectedUsage}
                    now={now}
                    predictionFor={predictionFor}
                    perspective={settings.quotaPerspective}
                  />
                  <QuotaWindowList
                    usage={selectedUsage}
                    now={now}
                    predictionFor={predictionFor}
                    perspective={settings.quotaPerspective}
                  />
                </>
              ) : (
                <div className="empty">Loading provider data…</div>
              )}
            </main>

        {visibleUsages.length > 0 ? (
          <AttentionRail items={attention} onSelect={selectProvider} />
        ) : null}
          </>
        )}
      </div>

      {settingsOpen ? (
        <section
          id="settings-drawer"
          className="settings-drawer"
          aria-label="Settings"
          onKeyDown={(event) => {
            if (event.key !== "Escape") return;
            event.stopPropagation();
            closeSettingsDrawer();
          }}
        >
          <label className="setting-row">
            <span className="setting-label">
              Launch LimitScope when Windows starts
            </span>
            <input
              ref={firstSettingRef}
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
          <div className="providers-prefs" role="group" aria-label="Providers">
            <span className="setting-label">Providers</span>
            <p className="providers-prefs-note">
              Visibility and order affect this dashboard and the floating
              bar. Hidden providers keep refreshing, keep their history,
              and keep notifications.
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
                            aria-label={"Show " + usageForRow.name + " in the dashboard"}
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
          <LocalDataSection
            onClearUsageHistory={clearLocalHistory}
            onClearProviderCache={clearProviderCache}
            onClearUsageIntelligence={clearUsageIntelligenceData}
            onClearExecutionRuns={clearExecutionRunStore}
            onResetPreferences={resetPreferences}
            activeExecutionRun={activeRun !== null}
          />
          {historyUnavailable ? (
            <p className="setting-note" role="status">
              Prediction history is unavailable on this device.
            </p>
          ) : null}
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
          <UpdatesSection
            phase={updatePhase}
            currentVersion={currentVersion}
            update={availableUpdate}
            error={updateError}
            onCheck={checkForUpdates}
            onInstall={() => void installUpdate()}
            onDismiss={dismissUpdate}
          />
          {startupError ? (
            <p className="setting-error" role="alert">
              Could not update the startup setting: {startupError}
            </p>
          ) : null}
        </section>
      ) : null}

      <footer className="utility-bar">
        <span
          className={
            refreshOverdue || stale
              ? "utility-updated stale"
              : "utility-updated"
          }
          role="status"
        >
          {updatedLabel}
        </span>
        <span className="utility-sep" aria-hidden="true">
          ·
        </span>
        <span className="utility-interval">
          Auto-refresh · {settings.refreshIntervalMinutes} min
        </span>
        <span className="utility-sep" aria-hidden="true">
          ·
        </span>
        <button
          type="button"
          ref={settingsButtonRef}
          className="utility-settings-btn"
          aria-expanded={settingsOpen}
          aria-controls="settings-drawer"
          onClick={() => setSettingsOpen((open) => !open)}
        >
          Settings
          <svg
            className={`utility-chevron${settingsOpen ? " open" : ""}`}
            viewBox="0 0 10 6"
            aria-hidden="true"
          >
            <path
              d="M1 1l4 4 4-4"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
              fill="none"
            />
          </svg>
        </button>
      </footer>
    </div>
  );
}


