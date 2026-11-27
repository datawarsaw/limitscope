import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { AttentionRail } from "./components/dashboard/AttentionRail";
import { ExecutionRunPanel } from "./components/dashboard/ExecutionRunPanel";
import { PrimaryQuotaPanel } from "./components/dashboard/PrimaryQuotaPanel";
import { ProviderRail } from "./components/dashboard/ProviderRail";
import { QuotaWindowList } from "./components/dashboard/QuotaWindowList";
import { StatusHistoryChart } from "./components/status/StatusHistoryChart";
import { SettingsView } from "./components/settings/SettingsView";
import { UsageView } from "./components/usage/UsageView";
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
import {
  attentionItems,
  defaultSelectedProviderId,
} from "./lib/dashboard";
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
 * v0.9 main window: the primary navigation is Status | Insights | Settings.
 *
 * Status is the calm, chart-led landing view: the provider rail selects a
 * provider, a compact status header carries the essential quota context, a
 * prominent history chart shows how that provider's quota windows moved,
 * and the needs-attention rail keeps the glanceable warnings. The floating
 * bar stays the product's primary interface for live quotas and wallets.
 *
 * Insights is the analytics surface over the backend quota-history
 * projection plus the Usage Intelligence token plane (KPIs, model and
 * source rankings, coverage honesty, daily activity).
 *
 * Settings is a dedicated page organized into General, Providers & Wallets,
 * Data & Privacy, and Support. Its persisted values stay owned by the Rust
 * settings lane (useSettings): no view here writes a preference directly.
 *
 * App owns only orchestration — runtime data, the selected provider, the
 * active view, and the structural layout — while focused components render
 * each zone. Every state semantic (fresh/stale/error/unknown, retained
 * last-good, prediction gating, Codex identity fencing) is consumed, not
 * redefined.
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
    setWalletMonitored,
    setWalletInFloatingBar,
    setCodexAccount,
    setQuotaPerspective,
    resetPreferences,
    syncRefreshInterval,
    startupPending,
    startupError,
    unverifiedPreferences,
  } = useSettings();
  const {
    usages: runtimeUsages,
    wallets: runtimeWallets,
    loading,
    lastUpdatedAt,
    refresh,
    refreshOverdue,
    stale,
    staleMinutes,
    historyRevision,
    usageIntelligenceRevision,
    codexIdentityStates,
  } =
    // The runtime owns the refresh cadence. The persisted interval reaches
    // it through the settings lane, which owns the write queue: the hook
    // itself never writes a preference, so no window can race a reset or
    // overwrite a newer edit with a stale copy of the cadence.
    useProviderUsage({ onAttach: syncRefreshInterval });
  // Dev-only visual fixtures (`?fixture=…` in `npm run dev`); null in any
  // production or test run. See src/dev/usageFixtures.ts.
  const devFixture =
    import.meta.env.DEV && typeof window !== "undefined"
      ? readDevFixture(window.location.search)
      : null;
  const usages = devFixture?.usages ?? runtimeUsages;
  // An older runtime (or a test surface without the field) yields no wallets.
  const wallets = runtimeWallets ?? [];
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

  // Provider selection: null until the user picks. Until then - and whenever
  // the picked provider is not visible - the deterministic default anchors
  // the view (highest usable primary percent). An explicit selection sticks
  // across refreshes; it is view state only and deliberately not persisted.
  const [selectedByUser, setSelectedByUser] = useState<string | null>(null);
  const fallbackId = defaultSelectedProviderId(visibleUsages);
  const selectedId =
    selectedByUser !== null &&
    visibleUsages.some((usage) => usage.id === selectedByUser)
      ? selectedByUser
      : fallbackId;
  const selectedUsage = visibleUsages.find((usage) => usage.id === selectedId);
  const selectProvider = (providerId: string) => setSelectedByUser(providerId);

  // Primary product navigation: Status | Insights | Settings. Insights keeps
  // its own provider scope (all providers by default; the rail narrows it);
  // a rail pick in Insights also becomes the Status selection so the provider
  // context is shared. Legacy dev-URL values ("overview", "usage") still map
  // so existing fixture links keep working.
  const [view, setView] = useState<"status" | "insights" | "settings">(() => {
    if (import.meta.env.DEV && typeof window !== "undefined") {
      const param = new URLSearchParams(window.location.search).get("view");
      if (param === "insights" || param === "usage") return "insights";
      if (param === "settings") return "settings";
    }
    return "status";
  });
  // The dashboard grid is one persistent scroll container; only its children
  // swap with the view. Reset the scroll position when (and only when) the
  // view changes so a page scrolled down in Status never opens Insights
  // mid-page. Focus stays on the header button that switched the view.
  const dashboardRef = useRef<HTMLDivElement>(null);
  const viewAtLastRender = useRef(view);
  useLayoutEffect(() => {
    if (viewAtLastRender.current === view) return;
    viewAtLastRender.current = view;
    const dashboard = dashboardRef.current;
    if (dashboard) dashboard.scrollTop = 0;
  }, [view]);
  const [usageScopeProviderId, setUsageScopeProviderId] = useState<
    string | null
  >(null);
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
  const attention = attentionItems(
    visibleUsages,
    predictionFor,
    "remaining",
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
              aria-pressed={view === "status"}
              onClick={() => setView("status")}
            >
              Status
            </button>
            <button
              type="button"
              className="view-nav-btn"
              aria-pressed={view === "insights"}
              onClick={() => setView("insights")}
            >
              Insights
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
          providerChoices={visibleUsages}
          nowMs={nowMs}
          onStart={startRun}
          onFinish={() => finishRun()}
          onDiscard={discardRun}
          onRefresh={() => void refresh()}
          onClose={closeRunPanel}
        />
      ) : null}

      <div className="dashboard" ref={dashboardRef}>
        {view === "settings" ? (
          <main
            id="settings-panel"
            className="provider-main settings-main"
            aria-label="Settings"
          >
            <SettingsView
              settings={settings}
              setLaunchAtStartup={setLaunchAtStartup}
              setRefreshInterval={setRefreshInterval}
              syncRefreshInterval={syncRefreshInterval}
              setTheme={setTheme}
              setQuotaNotifications={setQuotaNotifications}
              setUsageIntelligence={setUsageIntelligence}
              setQuotaPerspective={setQuotaPerspective}
              setProviderHidden={setProviderHidden}
              moveProvider={moveProvider}
              setWalletMonitored={setWalletMonitored}
              setWalletInFloatingBar={setWalletInFloatingBar}
              setCodexAccount={setCodexAccount}
              resetPreferences={resetPreferences}
              startupPending={startupPending}
              startupError={startupError}
              unverifiedPreferences={unverifiedPreferences}
              usages={usages}
              wallets={wallets}
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
        ) : (
          <>
            <ProviderRail
              usages={visibleUsages}
              selectedId={view === "insights" ? usageScopeProviderId : selectedId}
              onSelect={view === "insights" ? selectUsageScope : selectProvider}
              orientation={layout === "narrow" ? "horizontal" : "vertical"}
              controlsId={view === "insights" ? "usage-panel" : "provider-panel"}
              perspective="remaining"
              now={now}
            />
            {view === "insights" ? (
              <main
                id="usage-panel"
                className="provider-main usage-main"
                aria-label="Insights"
              >
                <UsageView
                  usages={usages}
                  scopeProviderId={usageScopeProviderId}
                  onScopeProvider={setUsageScopeProviderId}
                  historyRevision={historyRevision}
                  perspective={settings.quotaPerspective}
                  loader={devFixture?.loader}
                  usageIntelligenceEnabled={
                    devFixture?.usageIntelligenceEnabled ??
                    settings.usageIntelligence === true
                  }
                  usageIntelligenceRevision={usageIntelligenceRevision}
                  usageIntelligenceLoader={devFixture?.intelligenceLoader}
                  codexIdentityStates={codexIdentityStates}
                  onOpenSettings={() => setView("settings")}
                />
              </main>
            ) : (
              <>
                <main
                  id="provider-panel"
                  className="provider-main"
                  role="tabpanel"
                  aria-labelledby={
                    selectedId ? `provider-tab-${selectedId}` : undefined
                  }
                >
                  {selectedUsage ? (
                    <>
                      <PrimaryQuotaPanel
                        usage={selectedUsage}
                        now={now}
                        predictionFor={predictionFor}
                        perspective="remaining"
                      />
                      <StatusHistoryChart
                        usage={selectedUsage}
                        historyRevision={historyRevision}
                        perspective="remaining"
                        loader={devFixture?.loader}
                        codexIdentityStates={codexIdentityStates}
                      />
                      <QuotaWindowList
                        usage={selectedUsage}
                        now={now}
                        predictionFor={predictionFor}
                        perspective="remaining"
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
          </>
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
