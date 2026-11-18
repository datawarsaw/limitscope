# Claude Code provider (passive statusLine) — candidate

Supersedes the "Claude Code monitoring — BLOCKED" section of
`provider-defaults-candidate-20261117.md`. The blocker there was the private
OAuth usage endpoint; this provider does not use it.

* Source: the documented Claude Code `statusLine` hook. Claude Code pipes a
  JSON document to a user-configured command; `rate_limits.five_hour` and
  `rate_limits.seven_day` (`used_percentage`, `resets_at`) are the only fields
  kept. Everything else is discarded unread-further and never stored.
* Opt-in: the dashboard card only DISPLAYS the command
  (`LimitScope.exe --claude-statusline-ingest`) and a chained variant for an
  existing `statusLine`. LimitScope never reads or writes `~/.claude`.
* No credential, network request, model call or private endpoint. The
  historical private OAuth adapter stays forbidden (release verifier S8).
* Honest states: no data → Unavailable; corrupt store → Error; windows past
  their reset are dropped (never shown as 0%); readings older than 15 minutes
  are Stale and never become history/notification observations.
* Store: `claude-statusline-v1.json` in the app-data directory (bounded,
  atomic replace, re-validated on every read). Not part of the last-good cache.

## Codex second-account corrections shipped with it

* The initial CoWork name/home apply only to an ABSENT second-account entry.
  A saved entry — including a cleared home or a paused account — is never
  repopulated, in both the WebView sanitizer and the Rust store; a partially
  saved entry takes neutral values (`""`, `""`, monitored) in both.
* At registration the runtime refuses to read the primary account's login
  under the second account's name (explicit or ambient `CODEX_HOME`): the
  second account reports `codex_home_collision` instead.

## On-demand refresh: one explicit request

The provider stays passive. One explicit dashboard action, "Ping Claude (uses
1 request)", starts the already installed Claude CLI in a visible console
window with one fixed prompt ("Reply exactly OK."). The session keeps the
user's settings (--setting-sources user), so the statusLine hook runs, and
it keeps no tool (--tools ""), no command (--disable-slash-commands) and no
MCP server (--strict-mcp-config). Claude Code then runs its own statusLine
hook, and the existing ingest path stores whatever windows Claude reports.

* The click is the only thing that sends a request. There is no timer, no
  polling, no retry and no background service.
* The button reports the ATTEMPT, never a measurement: "Claude launched;
  quota pending". The provider runtime keeps owning the derived documents
  and refreshes them on its own schedule.
* The executable is resolved from a bounded list of known install locations
  and the PATH, and it is started with a direct CreateProcessW call: no
  shell, no command-string interpolation, no caller-supplied argument. Only
  claude.exe is accepted.
* The child gets a console of its own and a fresh empty scratch working
  directory below the app's per-user data directory, so the one-shot
  session cannot reach the user's projects. The scratch directory is
  removed without recursion once the session ends; a directory Claude Code
  wrote into is left in place.
* While the session runs, the backend holds the attempt lock: a second
  click is refused ("A Claude session is already running") instead of
  starting a second paid request.
* Known limitation: a headless claude -p run does NOT trigger the
  statusLine hook in Claude Code 2.1.318. Verified locally on 2026-11-18:
  one authorized prompt returned OK, and the receipt and the stored windows
  did not move. That is why the action starts an interactive session.
* Known limitation: the interactive session stays open and may ask once
  whether the scratch folder can be trusted, because the session runs in a
  fresh directory rather than in a project.
