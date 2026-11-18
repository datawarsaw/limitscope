import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** The Rust command's answer (see claude_cli.rs). */
export type ClaudeRefreshStatus =
  | "launched"
  | "notFound"
  | "spawnFailed"
  | "scratchUnavailable"
  | "alreadyRunning";

export interface ClaudeRefreshResult {
  status: ClaudeRefreshStatus;
}

/**
 * What one attempt did. Every message describes the ATTEMPT; none of them
 * claims a new quota measurement, and none of them says the usage is fresh.
 */
export function claudeRefreshMessage(status: ClaudeRefreshStatus): string {
  switch (status) {
    case "launched":
      return "Claude launched; quota pending. The 5-hour and 7-day values appear once Claude Code reports them.";
    case "notFound":
      return "Claude CLI not found. Install Claude Code (claude.exe) or add it to PATH.";
    case "spawnFailed":
      return "Claude CLI could not be started.";
    case "scratchUnavailable":
      return "The isolated working directory for Claude could not be created, so nothing was started.";
    case "alreadyRunning":
      return "A Claude session is already running, so no second request was sent. Try again once it is closed.";
  }
}

/**
 * The one explicit on-demand action for the passive Claude Code provider.
 *
 * The provider is passive: it only learns anything when Claude Code runs
 * its status line hook. This button starts the installed Claude Code in a
 * visible window with one fixed prompt, so the hook runs and Claude reports
 * the subscription windows. The click is the only thing that sends a
 * request; nothing here polls, schedules or retries.
 */
export function ClaudeRefreshControl() {
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<ClaudeRefreshStatus | null>(null);
  const [failed, setFailed] = useState(false);

  async function ping() {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setFailed(false);
    setStatus(null);
    try {
      const result = await invoke<ClaudeRefreshResult>("refresh_claude_usage");
      setStatus(result?.status ?? "spawnFailed");
    } catch {
      setFailed(true);
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }

  return (
    <div className="claude-setup claude-refresh">
      <p>
        On demand only. Ping Claude starts your installed Claude Code in a new
        window and sends one tiny request, so Claude Code runs its status
        line hook and reports the 5-hour and 7-day usage. Nothing runs in the
        background: the button is the only thing that sends a request. While
        that session is open, another ping is refused. If Claude Code asks
        whether it may trust the scratch folder, accept it once to let the
        request through.
      </p>
      <button
        type="button"
        className="claude-refresh-button"
        aria-label="Ping Claude for usage (uses 1 request)"
        aria-disabled={busy || undefined}
        aria-busy={busy || undefined}
        onClick={() => void ping()}
      >
        {busy ? "Starting Claude..." : "Ping Claude (uses 1 request)"}
      </button>
      {status ? (
        <p role="status" className="claude-refresh-result">
          {claudeRefreshMessage(status)}
        </p>
      ) : null}
      {failed ? (
        <p role="alert" className="claude-refresh-result">
          Couldn't reach the Claude CLI. If no Claude window opened, nothing
          was sent.
        </p>
      ) : null}
    </div>
  );
}
