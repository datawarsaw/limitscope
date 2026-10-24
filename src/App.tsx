import { useCallback, useEffect, useRef, useState } from "react";
import { ExecutionRunPanel } from "./components/dashboard/ExecutionRunPanel";
import { SettingsView } from "./components/settings/SettingsView";
import { UsageOverview } from "./components/usage/UsageOverview";
import { useNow } from "./hooks/useNow";
import { useDashboardLayout } from "./hooks/useDashboardLayout";
import { useExecutionRuns } from "./hooks/useExecutionRuns";
import { useQuotaPredictions } from "./hooks/useQuotaPredictions";
import { attachMainWindowBounds } from "./lib/mainWindowBounds";
import { clearExecutionRuns } from "./lib/localData";
import { useProviderUsage } from "./hooks/useProviderUsage";
import { useSettings } from "./hooks/useSettings";
import { useUpdater } from "./hooks/useUpdater";
import { formatTime } from "./lib/format";
import { defaultSelectedProviderId } from "./lib/dashboard";
import { formatRunElapsed } from "./lib/executionRuns";
import { readDevFixture } from "./dev/usageFixtures";
import { orderVisibleProviders } from "./lib/providerPreferences";

// Re-exported from its own home for the existing updater state-machine tests.
export { UpdatesSection } from "./components/settings/UpdatesSection";

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
 * v0.9 main window: the primary navigation is Usage | Settings.
 *
 * The floating bar stays the product's primary interface for live quotas,
 * resets, and wallets; the main window is a calm overview of locally
 * observed AI usage. Usage (the default) is one coherent surface over the
 * Usage Intelligence projection — reported tokens, events, model and source
 * rankings, an honest collection context, and the Daily activity heatmap.
 * Settings is a dedicated page (formerly the bottom drawer)
 * organized into General, Providers & Wallets, Data & Privacy, and Support.
 *
 * App owns only orchestration — runtime data, the active view, and the
 * structural layout — while focused components render each destination.
 * Every state semantic (fresh/stale/error/unknown, retained last-good,
 * prediction gating) is consumed, not redefined.
 */
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
  const { historyUnavailable, clearLocalHistory } =
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
  const [runPanelOpen, setRunPanelOpen] = useState(false);
  const runButtonRef = useRef<HTMLButtonElement>(null);
  const closeRunPanel = useCallback(() => {
    setRunPanelOpen(false);
    runButtonRef.current?.focus();
  }, []);

  // Presentation order: canonical registry (snapshot) order shaped by the
  // visibility/order preferences. The floating bar and the run panel read
  // this list - never the raw snapshot - while runtime-owned concerns
  // (history, predictions, notifications) keep the full `usages` above.
  const registryIds = usages.map((usage) => usage.id);
  const visibleUsages = orderVisibleProviders(
    usages,
    (usage) => usage.id,
    registryIds,
    settings.providerPreferences,
  );

  // Without the dashboard rail, the run panel anchors on the deterministic
  // default (highest usable primary percent, the quota strip's concept) and
  // offers its own provider select for explicit choices.
  const fallbackId = defaultSelectedProviderId(visibleUsages);
  const selectedUsage = visibleUsages.find((usage) => usage.id === fallbackId);

  // Primary product navigation: Usage | Settings. Usage-first: the usage
  // overview is the landing view; legacy dev-URL values ("overview",
  // "insights") map to it so existing fixture links keep working.
  const [view, setView] = useState<"usage" | "settings">(() => {
    if (
      import.meta.env.DEV &&
      typeof window !== "undefined"
    ) {
      const param = new URLSearchParams(window.location.search).get("view");
      if (param === "settings") return "settings";
      if (param === "usage" || param === "overview" || param === "insights") {
        return "usage";
      }
    }
    return "usage";
  });

  // CSS defaults the document to graphite; this swaps in a stored non-default
  // theme right after mount and keeps the attribute in sync on every switch.
  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme;
  }, [settings.theme]);

  // W01: keep the window inside its monitor's work area at launch, on DPI
  // change, and after monitor loss. Clamp-only, no-op outside Tauri.
  useEffect(() => attachMainWindowBounds(), []);

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
              aria-pressed={view === "usage"}
              onClick={() => setView("usage")}
            >
              Usage
            </button>
            <button
              type="button"
              className="view-nav-btn"
              aria-pressed={view === "settings"}
              onClick={() => setView("settings")}
            >
              Settings
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
        {view === "usage" ? (
          <main
            id="usage-panel"
            className="provider-main usage-main"
            aria-label="Usage"
          >
            <UsageOverview
              enabled={
                devFixture?.usageIntelligenceEnabled ??
                settings.usageIntelligence === true
              }
              revision={usageIntelligenceRevision}
              loader={devFixture?.intelligenceLoader}
              onOpenSettings={() => setView("settings")}
            />
          </main>
        ) : (
          <main
            id="settings-panel"
            className="provider-main settings-main"
            aria-label="Settings"
          >
            <SettingsView
              settings={settings}
              setLaunchAtStartup={setLaunchAtStartup}
              setRefreshInterval={setRefreshInterval}
              setTheme={setTheme}
              setQuotaNotifications={setQuotaNotifications}
              setUsageIntelligence={setUsageIntelligence}
              setQuotaPerspective={setQuotaPerspective}
              setProviderHidden={setProviderHidden}
              moveProvider={moveProvider}
              resetPreferences={resetPreferences}
              startupPending={startupPending}
              startupError={startupError}
              usages={usages}
              visibleUsages={visibleUsages}
              registryIds={registryIds}
              onClearUsageHistory={clearLocalHistory}
              onClearExecutionRuns={clearExecutionRunStore}
              activeExecutionRun={activeRun !== null}
              historyUnavailable={historyUnavailable}
              updatePhase={updatePhase}
              currentVersion={currentVersion}
              availableUpdate={availableUpdate}
              updateError={updateError}
              onCheckForUpdates={checkForUpdates}
              onInstallUpdate={() => void installUpdate()}
              onDismissUpdate={dismissUpdate}
            />
          </main>
        )}
      </div>

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
      </footer>
    </div>
  );
}
