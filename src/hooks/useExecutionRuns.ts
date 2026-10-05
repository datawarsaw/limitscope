import { useCallback, useEffect, useRef, useState } from "react";
import {
  beginExecutionRun,
  clearExecutionRunsState,
  completeExecutionRun,
  discardExecutionRun,
  loadExecutionRunsState,
  saveExecutionRunsState,
  type ClearExecutionRunsOptions,
  type ClearExecutionRunsResult,
  type ExecutionRunsState,
  type FinishRunOptions,
  type FinishRunOutcome,
  type StartRunOutcome,
  type StartRunRequest,
} from "../lib/executionRuns";
import type { ProviderUsage } from "../types";

/**
 * React binding for the manual execution run workflow. Owns no comparison
 * logic: every transition delegates to the pure store in
 * src/lib/executionRuns.ts, which delegates attribution to the provenance
 * core. State is persisted on every change so an active run survives an app
 * restart (bounded local recovery — no daemon, no timers).
 *
 * Each transition re-reads the persisted state first and works from that as
 * the base, so a second app window (which shares localStorage) can never
 * silently overwrite an active run it did not see — it gets the conflict
 * resolution instead.
 */
export function useExecutionRuns(usages: ProviderUsage[]) {
  const [state, setState] = useState<ExecutionRunsState>(() => loadExecutionRunsState());
  const usagesRef = useRef(usages);
  usagesRef.current = usages;

  useEffect(() => {
    saveExecutionRunsState(state);
  }, [state]);

  const startRun = useCallback((request: StartRunRequest): StartRunOutcome => {
    const outcome = beginExecutionRun(loadExecutionRunsState(), request);
    setState(outcome.state);
    return outcome;
  }, []);

  const finishRun = useCallback((options: FinishRunOptions = {}): FinishRunOutcome => {
    const outcome = completeExecutionRun(loadExecutionRunsState(), usagesRef.current, options);
    setState(outcome.state);
    return outcome;
  }, []);

  const discardRun = useCallback(() => {
    setState(discardExecutionRun(loadExecutionRunsState()));
  }, []);

  /** Applies the Local Data clear result to the open panel and toolbar. */
  const clearRuns = useCallback(
    (options: ClearExecutionRunsOptions = {}): ClearExecutionRunsResult => {
      const result = clearExecutionRunsState(options);
      setState(result.state);
      return result;
    },
    [],
  );

  const syncRuns = useCallback(() => {
    setState(loadExecutionRunsState());
  }, []);

  return {
    activeRun: state.activeRun,
    recentRuns: state.recentRuns,
    startRun,
    finishRun,
    discardRun,
    clearRuns,
    syncRuns,
  };
}
