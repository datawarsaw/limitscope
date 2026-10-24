import type { ReleaseNote, SafeUpdateError, UpdatePhase } from "../../lib/updater";

/**
 * The Updates settings block. Rendered inline in the Settings view's Support
 * group — a startup check only lights this block up, it never interrupts
 * with a modal. Exported for state-machine tests: each updater phase has a
 * pinned presentation.
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
