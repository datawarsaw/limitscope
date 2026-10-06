import { useEffect, useRef, useState } from "react";
import {
  localDataControls,
  type LocalDataCategoryId,
  type LocalDataClearResult,
  type LocalDataControl,
} from "../lib/localData";

/**
 * The v0.7 "Local data" settings section: one compact row per
 * LimitScope-owned data class, each with its own deliberate confirmation and
 * its own restrained outcome line.
 *
 * The component knows no filesystem path, storage key, or wire argument — it
 * calls the owned application actions it is handed (see `src/lib/localData.ts`
 * for the ownership split). Confirmations replace the row action in place
 * rather than opening a modal, so nothing here blocks the drawer: every
 * destructive choice is named in words, focus moves into the confirmation
 * and returns to the control that opened it, and a failure is reported as a
 * scoped error instead of a claim that the data was removed.
 *
 * While an execution run is in flight, its row confirms the active-run
 * discard explicitly rather than hiding it behind an ordinary clear.
 */
export type LocalDataSectionProps = {
  onClearUsageHistory: () => Promise<LocalDataClearResult> | LocalDataClearResult;
  onClearProviderCache: () => Promise<LocalDataClearResult> | LocalDataClearResult;
  onClearUsageIntelligence: () => Promise<LocalDataClearResult> | LocalDataClearResult;
  onClearExecutionRuns: () => Promise<LocalDataClearResult> | LocalDataClearResult;
  onResetPreferences: () => Promise<LocalDataClearResult> | LocalDataClearResult;
  activeExecutionRun: boolean;
};

type Outcome = { kind: "success" | "failure"; text: string };

export function LocalDataSection({
  onClearUsageHistory,
  onClearProviderCache,
  onClearUsageIntelligence,
  onClearExecutionRuns,
  onResetPreferences,
  activeExecutionRun,
}: LocalDataSectionProps) {
  const controls = localDataControls(activeExecutionRun);
  const [confirming, setConfirming] = useState<LocalDataCategoryId | null>(null);
  const [pending, setPending] = useState<LocalDataCategoryId | null>(null);
  const [outcomes, setOutcomes] = useState<
    Partial<Record<LocalDataCategoryId, Outcome>>
  >({});
  const triggerRefs = useRef<
    Partial<Record<LocalDataCategoryId, HTMLButtonElement | null>>
  >({});
  const confirmRefs = useRef<
    Partial<Record<LocalDataCategoryId, HTMLButtonElement | null>>
  >({});
  // The control whose confirmation is closing, so focus can go back to it.
  const refocusRef = useRef<LocalDataCategoryId | null>(null);

  useEffect(() => {
    if (confirming === null) return;
    confirmRefs.current[confirming]?.focus();
  }, [confirming]);

  useEffect(() => {
    if (confirming !== null) return;
    const id = refocusRef.current;
    if (id === null) return;
    refocusRef.current = null;
    triggerRefs.current[id]?.focus();
  }, [confirming]);

  const actionFor = (id: LocalDataCategoryId) =>
    id === "usageHistory"
      ? onClearUsageHistory
      : id === "usageIntelligence"
        ? onClearUsageIntelligence
        : id === "providerCache"
          ? onClearProviderCache
          : id === "executionRuns"
            ? onClearExecutionRuns
            : onResetPreferences;

  const dropOutcome = (id: LocalDataCategoryId) => {
    setOutcomes((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
  };

  const openConfirm = (id: LocalDataCategoryId) => {
    // A stale result line must not sit next to a fresh question.
    dropOutcome(id);
    setConfirming(id);
  };

  const cancelConfirm = (id: LocalDataCategoryId) => {
    refocusRef.current = id;
    setConfirming(null);
  };

  const runControl = async (control: LocalDataControl) => {
    setPending(control.id);
    let outcome: Outcome;
    try {
      const result = await actionFor(control.id)();
      outcome = result.ok
        ? { kind: "success", text: control.success }
        : { kind: "failure", text: control.failure };
    } catch {
      outcome = { kind: "failure", text: control.failure };
    }
    setOutcomes((prev) => ({ ...prev, [control.id]: outcome }));
    setPending(null);
    refocusRef.current = control.id;
    setConfirming(null);
  };

  return (
    <div className="local-data" role="group" aria-label="Local data">
      <span className="local-data-title">Local data</span>
      {controls.map((control) => {
        const outcome = outcomes[control.id];
        const isConfirming = confirming === control.id;
        const isPending = pending === control.id;
        return (
          <div className="local-data-row" key={control.id}>
            <span className="setting-copy">
              <span className="setting-label">{control.label}</span>
              <span className="setting-note">{control.note}</span>
            </span>
            {isConfirming ? (
              <div
                className="local-data-confirm"
                role="group"
                aria-labelledby={"local-data-confirm-title-" + control.id}
              >
                <span
                  className="setting-label"
                  id={"local-data-confirm-title-" + control.id}
                >
                  {control.confirmTitle}
                </span>
                <p className="setting-note">{control.confirmBody}</p>
                <span className="local-data-confirm-actions">
                  <button
                    type="button"
                    className="local-data-cancel-btn"
                    onClick={() => cancelConfirm(control.id)}
                    disabled={isPending}
                  >
                    Cancel
                  </button>
                  <button
                    type="button"
                    className="local-data-confirm-btn"
                    ref={(node) => {
                      confirmRefs.current[control.id] = node;
                    }}
                    onClick={() => void runControl(control)}
                    disabled={isPending}
                  >
                    {isPending ? "Working\u2026" : control.confirmAction}
                  </button>
                </span>
              </div>
            ) : (
              <button
                type="button"
                className="clear-history-btn"
                ref={(node) => {
                  triggerRefs.current[control.id] = node;
                }}
                onClick={() => openConfirm(control.id)}
                disabled={pending !== null}
                // The trigger and the confirming button share one accessible
                // name: the action in words ("Clear usage history"), so the
                // row's full intent survives without a visible sentence.
                aria-label={control.confirmAction}
              >
                {control.action}
              </button>
            )}
            {outcome ? (
              <p
                className={
                  outcome.kind === "success" ? "setting-success" : "setting-error"
                }
                role={outcome.kind === "success" ? "status" : "alert"}
              >
                {outcome.text}
              </p>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
