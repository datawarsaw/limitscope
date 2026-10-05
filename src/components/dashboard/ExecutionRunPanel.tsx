import { useEffect, useRef, useState } from "react";
import { formatTime } from "../../lib/format";
import {
  confidencePresentation,
  formatDeltaPoints,
  formatRunDuration,
  formatRunElapsed,
  DEFAULT_EXECUTION_HARNESS,
  EXECUTION_HARNESSES,
  resolveStartSnapshot,
  type ExecutionHarness,
  type FinishRunOutcome,
  type StartRunOutcome,
  type StartRunRequest,
} from "../../lib/executionRuns";
import type { ExecutionProvenanceRun, ProvenanceWindowDelta } from "../../lib/provenance";
import type { ProviderUsage } from "../../types";

/**
 * The v0.7 execution-run panel: one deliberately small surface for the
 * manual Start → work elsewhere → Finish bracket. Every attribution verdict
 * (confidence, comparability, reset/account states) is rendered from the
 * provenance core's run object — this component never recomputes deltas.
 * All primary actions are native buttons/inputs, reachable by keyboard, and
 * focus lands somewhere sensible after each transition.
 */
export function ExecutionRunPanel({
  activeRun,
  recentRuns,
  selectedUsage,
  usages,
  nowMs,
  onStart,
  onFinish,
  onDiscard,
  onRefresh,
  onClose,
}: {
  activeRun: ExecutionProvenanceRun | null;
  recentRuns: ExecutionProvenanceRun[];
  selectedUsage: ProviderUsage | undefined;
  usages: ProviderUsage[];
  /** Wall clock for the live "Running · 12m" elapsed readout. */
  nowMs: number;
  onStart: (request: StartRunRequest) => StartRunOutcome;
  onFinish: () => FinishRunOutcome;
  onDiscard: () => void;
  onRefresh: () => void;
  onClose: () => void;
}) {
  const [harness, setHarness] = useState<ExecutionHarness>(DEFAULT_EXECUTION_HARNESS);
  const [model, setModel] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState("");
  const [startError, setStartError] = useState<string | null>(null);
  const [finishError, setFinishError] = useState<string | null>(null);
  // A start attempt refused because a run is already active. The active run
  // is never overwritten; the operator gets the bounded resolution.
  const [conflictRun, setConflictRun] = useState<ExecutionProvenanceRun | null>(null);
  const [announcement, setAnnouncement] = useState<string | null>(null);

  const headingRef = useRef<HTMLHeadingElement>(null);
  const finishButtonRef = useRef<HTMLButtonElement>(null);
  const summaryHeadingRef = useRef<HTMLHeadingElement>(null);
  // Where keyboard focus should land after the next lifecycle transition.
  const focusIntentRef = useRef<"active" | "summary" | null>(null);

  useEffect(() => {
    headingRef.current?.focus();
  }, []);

  useEffect(() => {
    if (focusIntentRef.current === "active" && activeRun !== null) {
      finishButtonRef.current?.focus();
    } else if (focusIntentRef.current === "summary" && activeRun === null) {
      summaryHeadingRef.current?.focus();
    }
    focusIntentRef.current = null;
  });

  const providerName = (providerId: string) =>
    usages.find((usage) => usage.id === providerId)?.name ?? providerId;

  const accountLabel = (run: ExecutionProvenanceRun) => (run.account ? ` · ${run.account}` : "");

  const handleStart = () => {
    setStartError(null);
    const outcome = onStart({
      usage: selectedUsage,
      harness,
      model: model.trim() !== "" ? model : undefined,
      reasoningEffort: reasoningEffort.trim() !== "" ? reasoningEffort : undefined,
    });
    if (outcome.status === "no-usable-snapshot") {
      setStartError(outcome.reason);
      return;
    }
    if (outcome.status === "conflict") {
      setConflictRun(outcome.activeRun);
      return;
    }
    setModel("");
    setReasoningEffort("");
    setAnnouncement(
      `Run started on ${providerName(outcome.run.providerId)}. Finish it when the work is done.`,
    );
    focusIntentRef.current = "active";
  };

  const handleFinish = () => {
    setFinishError(null);
    const outcome = onFinish();
    if (outcome.status === "after-unavailable") {
      setFinishError(`${outcome.reason} Refresh the provider and finish again, or discard the run.`);
      return;
    }
    if (outcome.status === "finished") {
      setConflictRun(null);
      setAnnouncement(
        `Run finished. Attribution confidence: ${confidencePresentation(outcome.run.confidence).label}.`,
      );
      focusIntentRef.current = "summary";
    }
  };

  const handleDiscard = () => {
    onDiscard();
    setConflictRun(null);
    setFinishError(null);
    setAnnouncement("Run discarded. Nothing was recorded.");
  };

  const lastRun = recentRuns[0];
  const baseline = resolveStartSnapshot(selectedUsage, Date.now());

  return (
    <section
      id="execution-run-panel"
      className="execution-panel"
      aria-label="Execution run"
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented) {
          onClose();
        }
      }}
    >
      <p className="visually-hidden" role="status" aria-live="polite">
        {announcement}
      </p>
      <header className="run-panel-head">
        <h2 className="run-panel-title" ref={headingRef} tabIndex={-1}>
          Execution run
        </h2>
        <button
          type="button"
          className="run-close-btn"
          aria-label="Close execution run panel"
          onClick={onClose}
        >
          ×
        </button>
      </header>

      {conflictRun !== null ? (
        <div className="run-conflict" role="alert">
          <p className="run-conflict-copy">
            A run is already active (started {formatTime(conflictRun.startedAt)}). Only one run
            can be tracked at a time.
          </p>
          <div className="run-actions">
            <button type="button" className="run-btn-primary" onClick={handleFinish}>
              Finish current run
            </button>
            <button type="button" className="run-btn-secondary" onClick={handleDiscard}>
              Discard current run
            </button>
            <button
              type="button"
              className="run-btn-secondary"
              onClick={() => setConflictRun(null)}
            >
              Cancel
            </button>
          </div>
        </div>
      ) : activeRun !== null ? (
        <section className="run-active" aria-label="Run in progress">
          <p className="run-active-status" role="status">
            <span className="run-active-dot" aria-hidden="true" />
            Running · {formatRunElapsed(activeRun.startedAt, nowMs)}
          </p>
          <p className="run-active-meta">
            {activeRun.harness}
            {activeRun.model ? ` · ${activeRun.model}` : ""}
            {activeRun.reasoningEffort ? ` · ${activeRun.reasoningEffort}` : ""} · Started{" "}
            {formatTime(activeRun.startedAt)} · {providerName(activeRun.providerId)}
            {accountLabel(activeRun)}
          </p>
          <p className="run-note">
            The harness label is the one you selected. LimitScope does not detect or verify it.
          </p>
          {finishError !== null ? (
            <p className="run-error" role="alert">
              {finishError}{" "}
              <button type="button" className="run-inline-btn" onClick={onRefresh}>
                Refresh
              </button>
            </p>
          ) : null}
          <div className="run-actions">
            <button
              type="button"
              className="run-btn-primary"
              ref={finishButtonRef}
              onClick={handleFinish}
            >
              Finish run
            </button>
            <button type="button" className="run-btn-secondary" onClick={handleDiscard}>
              Discard
            </button>
          </div>
        </section>
      ) : (
        <>
          <form
            className="run-start"
            aria-label="Start execution run"
            onSubmit={(event) => {
              event.preventDefault();
              handleStart();
            }}
          >
            <div className="run-field-row">
              <label className="run-field-label" htmlFor="run-harness-select">
                Harness
              </label>
              <select
                id="run-harness-select"
                className="run-select"
                value={harness}
                onChange={(event) => setHarness(event.target.value as ExecutionHarness)}
              >
                {EXECUTION_HARNESSES.map((choice) => (
                  <option key={choice} value={choice}>
                    {choice}
                  </option>
                ))}
              </select>
            </div>
            <details className="run-optional">
              <summary>Optional details</summary>
              <div className="run-field-row">
                <label className="run-field-label" htmlFor="run-model-input">
                  Model
                </label>
                <input
                  id="run-model-input"
                  className="run-input"
                  type="text"
                  value={model}
                  maxLength={64}
                  onChange={(event) => setModel(event.target.value)}
                />
              </div>
              <div className="run-field-row">
                <label className="run-field-label" htmlFor="run-effort-input">
                  Reasoning effort
                </label>
                <input
                  id="run-effort-input"
                  className="run-input"
                  type="text"
                  value={reasoningEffort}
                  maxLength={64}
                  onChange={(event) => setReasoningEffort(event.target.value)}
                />
              </div>
            </details>
            <p className="run-baseline">
              {baseline.ok
                ? `Baseline: ${selectedUsage?.name ?? ""}${
                    baseline.snapshot.accountIdentity !== undefined
                      ? ` · ${baseline.snapshot.accountIdentity}`
                      : ""
                  }`
                : "Baseline: no usable current provider snapshot"}
            </p>
            <p className="run-note">
              The harness label is declared by you; LimitScope does not detect or verify it.
              Optional fields are recorded only when you type them.
            </p>
            {startError !== null ? (
              <p className="run-error" role="alert">
                Cannot start: {startError}{" "}
                <button type="button" className="run-inline-btn" onClick={onRefresh}>
                  Refresh
                </button>
              </p>
            ) : null}
            <div className="run-actions">
              <button type="submit" className="run-btn-primary">
                Start run
              </button>
            </div>
          </form>

          {lastRun !== undefined ? (
            <ExecutionSummaryCard
              run={lastRun}
              providerName={providerName(lastRun.providerId)}
              headingRef={summaryHeadingRef}
            />
          ) : null}
        </>
      )}

      {recentRuns.length > 0 && activeRun === null ? (
        <details className="run-recent">
          <summary>Recent runs ({recentRuns.length})</summary>
          <ul className="run-recent-list">
            {recentRuns.map((run) => (
              <li key={run.runId} className="run-recent-item">
                <span className="run-recent-meta">
                  {formatTime(run.endedAt ?? run.startedAt)} · {run.harness} ·{" "}
                  {run.endedAt ? formatRunDuration(run.startedAt, run.endedAt) : "—"} ·{" "}
                  {confidencePresentation(run.confidence).label}
                </span>
                <span className="run-recent-delta">
                  {run.comparable
                    ? (run.windows.find((delta) => delta.comparable) !== undefined
                        ? formatDeltaPoints(
                            run.windows.find((delta) => delta.comparable)!.deltaPoints,
                          )
                        : "no comparable windows")
                    : "not comparable"}
                </span>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </section>
  );
}

/** Presentation-only reason line for a window the core marked incomparable. */
function windowBlockedReason(
  run: ExecutionProvenanceRun,
  delta: ProvenanceWindowDelta,
): string {
  if (run.resetCrossed) {
    return "Reset occurred during this run — delta not comparable";
  }
  if (delta.reason?.includes("appeared after")) {
    return "Window appeared after this run started — not comparable";
  }
  if (delta.reason?.includes("disappeared before")) {
    return "Window disappeared before this run ended — not comparable";
  }
  return delta.reason ?? run.incomparabilityReason ?? "Delta not comparable";
}

/**
 * The bounded execution summary: per-window observed deltas, the explicit
 * confidence with an on-demand explanation, and the mandatory disclaimer —
 * present whenever deltas are displayed, comparable or not.
 */
export function ExecutionSummaryCard({
  run,
  providerName,
  headingRef,
}: {
  run: ExecutionProvenanceRun;
  providerName: string;
  headingRef?: React.RefObject<HTMLHeadingElement>;
}) {
  const confidence = confidencePresentation(run.confidence);
  return (
    <section className="run-summary" aria-label="Execution summary">
      <h3 className="run-summary-title" ref={headingRef} tabIndex={-1}>
        Execution summary
      </h3>
      <p className="run-summary-meta">
        {run.harness}
        {run.model ? ` · ${run.model}` : ""}
        {run.reasoningEffort ? ` · ${run.reasoningEffort}` : ""} ·{" "}
        {formatTime(run.startedAt)} → {formatTime(run.endedAt ?? run.startedAt)} ·{" "}
        {run.endedAt ? formatRunDuration(run.startedAt, run.endedAt) : "—"} · {providerName}
        {run.account ? ` · ${run.account}` : ""}
      </p>
      <ul className="run-summary-windows">
        {run.windows.map((delta) => (
          <li key={delta.label} className="run-summary-window">
            <span className="run-window-label">{delta.label}</span>
            {delta.comparable ? (
              <span className="run-window-delta">
                {Math.round(delta.beforeUsedPercent)}% → {Math.round(delta.afterUsedPercent)}%{" "}
                <strong>{formatDeltaPoints(delta.deltaPoints)}</strong>
              </span>
            ) : (
              <span className="run-window-blocked">{windowBlockedReason(run, delta)}</span>
            )}
          </li>
        ))}
        {run.windows.length === 0 ? (
          <li className="run-window-blocked">No quota windows were available to compare.</li>
        ) : null}
      </ul>
      {!run.comparable && run.incomparabilityReason ? (
        <p className="run-summary-blocked" role="status">
          {run.incomparabilityReason}
        </p>
      ) : null}
      <div className="run-confidence-row">
        Confidence:{" "}
        <span className={`run-confidence run-confidence-${run.confidence.toLowerCase()}`}>
          {confidence.label}
        </span>
        <details className="run-confidence-why">
          <summary>Why?</summary>
          <p className="run-confidence-explanation">{confidence.description}</p>
        </details>
      </div>
      <p className="run-disclaimer">Observed quota delta during execution (not exact task cost).</p>
    </section>
  );
}
