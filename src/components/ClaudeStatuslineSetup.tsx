import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** The Rust command's answer (see `ClaudeStatuslineSetup` in claude_statusline.rs). */
export interface ClaudeStatuslineSetupInfo {
  command: string | null;
  powershellCommand: string | null;
  settingsSnippet: string | null;
}

/** Git Bash recipe for a user who already has a statusLine command. */
export function chainedCommand(command: string): string {
  return `input=$(cat); printf '%s' "$input" | ${command}; printf '%s' "$input" | YOUR_EXISTING_COMMAND`;
}

/** The chained command as a ready-to-paste, correctly escaped settings entry. */
export function chainedSnippet(command: string): string {
  return JSON.stringify(
    { statusLine: { type: "command", command: chainedCommand(command) } },
    null,
    2,
  );
}

/**
 * Opt-in setup for the passive Claude Code provider. This component only
 * DISPLAYS a command: LimitScope never edits Claude Code's settings, never
 * reads them, and never starts Claude Code. The operator copies the snippet
 * into their own `~/.claude/settings.json` and can remove it again at any
 * time by deleting the `statusLine` entry.
 */
export function ClaudeStatuslineSetup() {
  const [setup, setSetup] = useState<ClaudeStatuslineSetupInfo | null | "failed">(null);
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    invoke<ClaudeStatuslineSetupInfo>("get_claude_statusline_setup")
      .then((info) => {
        if (active) setSetup(info);
      })
      .catch(() => {
        if (active) setSetup("failed");
      });
    return () => {
      active = false;
    };
  }, []);

  async function copy(key: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
    } catch {
      setCopied(null);
    }
  }

  const info = setup && setup !== "failed" ? setup : null;
  const command = info?.command ?? null;
  return (
    <div className="claude-setup">
      <p>
        Passive source. LimitScope receives your Claude subscription 5-hour and
        7-day usage only when Claude Code runs its status line hook, so values
        can update after Claude Code responses and become last known when no new
        quota information arrives.
        It needs a claude.ai Pro, Max or Ultra plan and one Claude Code
        response. No network request, login or model call is made by LimitScope.
      </p>
      <details>
        <summary>Set up Claude Code monitoring (opt-in)</summary>
        {setup === null ? <p>Loading…</p> : null}
        {setup === "failed" ? (
          <p role="alert">Couldn't read the setup command. Nothing was changed.</p>
        ) : null}
        {info && !command ? (
          <p role="alert">
            This install path can't be quoted safely for a status line command.
            Install LimitScope in a path without special characters to use
            Claude Code monitoring.
          </p>
        ) : null}
        {info && command ? (
          <>
            <ol>
              <li>
                Open your Claude Code <code>~/.claude/settings.json</code> and add this{" "}
                <code>statusLine</code> entry. LimitScope never edits that file.
              </li>
              <li>
                Already have a <code>statusLine</code>? Don't replace it. In Git Bash,
                use the chained entry below instead; it keeps your command's output.
              </li>
              <li>
                Start a Claude Code session and send one message. The values appear
                after Claude Code's first response.
              </li>
              <li>To stop, delete the <code>statusLine</code> entry.</li>
            </ol>
            <pre aria-label="Claude Code statusLine settings">{info.settingsSnippet}</pre>
            <button type="button" className="opendesign-refresh" onClick={() => void copy("snippet", info.settingsSnippet ?? command)}>
              {copied === "snippet" ? "Copied" : "Copy settings"}
            </button>
            {info.powershellCommand && info.powershellCommand !== command ? (
              <p>
                If Claude Code uses PowerShell (Git Bash not installed), use this
                command instead: <code>{info.powershellCommand}</code>
              </p>
            ) : null}
            <p>
              Chained entry (Git Bash; replace <code>YOUR_EXISTING_COMMAND</code> with the
              command you already use):
            </p>
            <pre aria-label="Chained status line settings">{chainedSnippet(command)}</pre>
            <button type="button" className="opendesign-refresh" onClick={() => void copy("chain", chainedSnippet(command))}>
              {copied === "chain" ? "Copied" : "Copy chained settings"}
            </button>
            <p>
              The command points at this LimitScope install; update it if you move or
              reinstall the app. It prints nothing and stores only the two usage
              percentages and their reset times.
            </p>
          </>
        ) : null}
      </details>
    </div>
  );
}
