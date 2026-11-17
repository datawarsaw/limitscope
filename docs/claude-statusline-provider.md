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
