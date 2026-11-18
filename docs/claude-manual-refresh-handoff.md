## BOOTSTRAP
TASK: Correct the independently confirmed security and reliability blockers of the LimitScope Claude ping candidate 390c0ffdfb0b6c7ff5edb02db158a884091074a8 in one consolidated pass: P0 (--restricted ignores the user settings and therefore the statusLine hook), P1 (a second paid launch is possible while a session runs), P2 (the working directory is the shared %TEMP% root), P3 (relative PATH candidates are accepted).
BASE: 390c0ffdfb0b6c7ff5edb02db158a884091074a8 (the reviewed candidate), whose parent is dd60f764671e6cb4a1f5218ced97bfe66b55fc73 (published stable v0.8.33); the candidate is the commit containing this file.
STATE: IMPLEMENTED in a fresh isolated worktree; focused Rust and frontend checks pass; nothing installed, released, pushed or merged.
RESULT: The same one button, "Ping Claude (uses 1 request)", now starts the installed Claude CLI with the USER settings in place (--setting-sources user, --tools "", --disable-slash-commands, --strict-mcp-config) in a fresh empty per-user scratch directory, and the backend keeps its attempt lock for the whole life of the child process, so a second click is refused instead of starting a second paid request.
GUARDRAILS: no install, release, push, PR, tag or merge; no polling, schedule or service; no credential, transcript, private-API or Claude settings read; no additional paid Claude prompt; no version bump; the original feature worktree was not touched.
NEXT ACTION: Independent reviewer re-checks this commit against P0-P3 and the security boundaries it touches.

## STATUS
READY FOR RE-REVIEW. One local commit on a fresh branch; independent re-review, Human Acceptance and release remain separate.

## RESULT / VERDICT
ROLE: worker. TASK: one consolidated correction pass over a reviewed candidate.
OBJECTIVE (verbatim): "Implement ONE consolidated correction of independently confirmed security/reliability blockers in LimitScope Claude ping candidate 390c0ffdfb0b6c7ff5edb02db158a884091074a8. Work ONLY in this fresh isolated worktree/branch; do not touch original feature worktree because it has unknown uncommitted frontend changes. Commit corrected source locally for later cherry-pick into release/v0.8.34."

P0 (--restricted ignores the user settings and thus prevents the existing statusLine hook) - RESOLVED. The installed CLI's own help (Claude Code 2.1.318, read locally with `claude --help`, no request sent) documents --restricted as: "ignores user, project and local settings files (managed settings and --settings still apply; add --strict-mcp-config to skip MCP servers too)". The statusLine hook this feature exists for lives in the user's settings, so with --restricted the hook could never run. --restricted is gone. The replacement set is documented in the same help output: --setting-sources user (loads the user settings, the hook included, and no project or local setting), --tools "" (disables every built-in tool), --disable-slash-commands (no skills, no commands) and --strict-mcp-config (no MCP server). --safe-mode and --bare are deliberately not used either: both disable hooks. The prompt stays the FIRST argument, because --tools takes a variable number of values and would otherwise swallow it as a tool name.

P1 (duplicate paid launches, because ATTEMPT_RUNNING was released right after CreateProcessW returned) - RESOLVED. The one-shot AtomicBool is replaced by a session lock (a Mutex holding the ONE live session) that is taken in the same critical section as the launch and is released only when the child process exits. PROCESS_INFORMATION.hProcess is no longer closed immediately: it is retained in a ProcessHandle (closed exactly once by Drop), the session keeps it, and a bounded waiter thread (60 s slices, at most 720 of them) waits on it until the child exits. A second click while the child lives is refused with the honest status alreadyRunning ("A Claude session is already running, so no second request was sent. Try again once it is closed."). The frontend may re-enable the button after the IPC call returns; the backend refuses the second launch, and no event, timer, poll or new IPC command was added.

P2 (the process cwd was the populated shared %TEMP% root) - RESOLVED. Each launch creates a fresh, empty, isolated directory below the app's own per-user data directory (\<app data>\claude-ping\ping-<n>), never the %TEMP% root. The leaf is created with create_dir, so a directory that already exists is never reused, and it is therefore empty: no CLAUDE.md, no .claude directory and no other instruction file of the user's projects can be in scope. The cleanup after the session is NOT recursive: the directory is removed only while it is empty, and a directory Claude Code wrote into is left in place. Project and local instruction loading is closed by --setting-sources user; the trust prompt is not bypassed (that would mean writing Claude Code's private state): the operator accepts it once in Claude Code's own window, and the card says so.

P3 (relative PATH candidates were accepted) - RESOLVED. A candidate is accepted only when it is an absolute path to an existing file named claude.exe; relative PATH entries are skipped, and a relative candidate is never launched. The resolved path is passed to CreateProcessW as lpApplicationName, so the program that runs is the one that was identified.

Required action 1 (preserve the user-settings hook; use documented safe flags; no headless -p; no polling) - MET. The flag set comes from the installed CLI's own --help output. The prompt is still one fixed sentence, still one per explicit click, still not headless: -p/--print is refused in the test as well. No timer, no retry, no schedule was added.
Required action 2 (track the child until exit; hold the guard for the complete session; close handles reliably; no daemon) - MET. See P1. The waiter is a plain thread of the app process: no daemon, no scheduler, no service.
Required action 3 (fresh isolated per-user scratch directory; no recursive cleanup; safe instruction loading; absolute PATH) - MET. See P2 and P3.
Required action 4 (focused Rust/TypeScript tests and the existing handoff document; preserve updater trust, Tauri ACL, quota timestamps and the original feature; no version bump) - MET. The command name, the permission identifier, the capability entry, the statusLine store, the runtime and the quota timestamps are untouched; the permission description was corrected because the command now creates one empty scratch directory. Cargo.toml is unchanged, so the version stays 0.8.33.
Required action 5 (focused validation only; no additional paid prompt; one correction pass) - MET. See EVIDENCE. No live Claude request was made in this pass; the interactive session's hook behaviour stays UNVERIFIED and is reported as such.
Required action 6 (one clean local commit; exact SHA, base, changed files, resolutions, risks; no push/merge/tag/PR/release/install; other worktrees untouched) - MET. See TARGET and the run summary.

## EXECUTION METADATA
MODEL: minimax/MiniMax-M3.1-Flash (runtime reported); upstream serving provider UNVERIFIED (no separate provider evidence).
REASONING / THINKING EFFORT: max (runtime reported).
HARNESS: T3 Code through the Codex harness.
SESSION TYPE: implementation.
SUBAGENTS: 0. SUBAGENTS USED: 0.
The advisor tool was called twice and returned HTTP 404 both times, so no advisor findings exist.

## TARGET / CANDIDATE
Repository/worktree: C:/Users/micha/.t3/worktrees/limitscope/fix-claude-ping-release-e3cf967004e1ba98
Branch: fix/claude-ping-release-safety. Base and pre-edit HEAD: 390c0ffdfb0b6c7ff5edb02db158a884091074a8, whose parent is dd60f764671e6cb4a1f5218ced97bfe66b55fc73 (the accepted published stable v0.8.33 SHA). Date: 2026-11-19.
Candidate: the commit containing this handoff, resolvable with git log -1 --format=%H -- docs/claude-manual-refresh-handoff.md.
Changed: src-tauri/src/claude_cli.rs (flags, absolute resolution, isolated scratch directory, session lock with a retained process handle and a bounded waiter), src/components/ClaudeRefreshControl.tsx (one new status, honest refusal copy, trust-prompt note), src/components/ClaudeRefreshControl.test.tsx (the new status and the refusal copy), src-tauri/permissions/ipc-windows.toml (permission description only), docs/claude-statusline-provider.md (the launch description), this handoff.
Not changed: the statusLine ingest module, the stored schema, the freshness and merge rules, the provider runtime, the updater configuration, the capability grants, any other provider, the installed app, Claude Code's settings, auth or cookies, the root checkout at C:/AI/limitscope, and the original feature worktree C:/Users/micha/.t3/worktrees/limitscope/feature-claude-manual-r-36273ff7c7ace734 (4 unknown dirty UI files; never read, written or cleaned).
Dirty state at authoring: the changes above were uncommitted; recheck HEAD and cleanliness before the cherry-pick.

## EVIDENCE
- cargo test --locked --offline claude_cli: the focused module suite, including the new tests for the absolute-only resolution, the isolated scratch directory and the attempt lock. See the run summary for the exact counts.
- The installed CLI's own help output (claude --help, Claude Code 2.1.318, no request sent) is the source for the flag set and for the --restricted description quoted above.
- Windows console mechanics probe, no Claude involved (from the earlier candidate): Rust's std::process::Command hands the PARENT's console handles to a CREATE_NEW_CONSOLE child, while a direct CreateProcessW with STARTF_USESTDHANDLES unset gives the child its own console. The shipped code therefore uses CreateProcessW.
- Live Claude request count in this pass: zero. The one request of the earlier candidate is the only request ever spent.
- Not run: the full Rust suite, the native app, the installer, a live interactive Claude session, and the repository secret scan.

## RISKS / BLOCKERS
- UNVERIFIED: that an interactive session started with --setting-sources user actually runs the statusLine hook. The evidence is the CLI's documented flag semantics, not a measurement; no further Claude request was permitted. If a session does not run the hook, the button will keep reporting "Claude launched; quota pending"; the remedy is a different launch form, not more Claude calls.
- UNVERIFIED: whether --tools "" is accepted exactly as the help describes ("Use "" to disable all tools") by this CLI build. The argument is written as an empty quoted argument, which is the documented spelling.
- UNVERIFIED: whether Claude Code asks to trust the fresh scratch directory in this configuration. It is expected to ask once; the operator accepts it in Claude Code's own window. Trust is never pre-accepted, because that would mean writing Claude Code's private state.
- The button costs one subscription request per click, and the request is only sent once the operator accepts the trust prompt. The label states the cost.
- The waiter thread gives up after 12 hours; a session that outlives it keeps the lock, and the next attempt asks the retained handle whether the child is gone, so a second paid launch stays impossible.
- A scratch directory that Claude Code wrote into is left on disk on purpose (no recursive deletion).
- Only claude.exe is accepted, at an absolute path. A claude.cmd shim from an npm install is not launched, because it is not a console program and running it would require a shell.
- The Claude console window stays open after the reply; the operator closes it.
- No independent re-review and no Human Acceptance have occurred.

## HUMAN ACCEPTANCE INSTRUCTIONS
1. Run the candidate from this worktree (no install): the Claude Code card of the dashboard shows one button, "Ping Claude (uses 1 request)".
2. Click it once. Expect a Claude Code console window that replies OK, and the card to report "Claude launched; quota pending". If Claude asks to trust the scratch folder, accept it once.
3. Expect the card's values and its existing provenance lines (last statusLine event, 5h usage changed, 7d usage changed) to follow once Claude has answered. Unchanged percentages are expected when Claude was not used since, and are not a failure.
4. Click the button again while that window is still open. Expect the honest refusal: "A Claude session is already running, so no second request was sent." No second Claude window appears.
5. Close the Claude window, wait a moment, click again. Expect a new session: the lock is released when the child exits.

## NEXT ACTION
Independent reviewer re-checks the commit containing this handoff against P0-P3 and the security boundaries it touches.

Handoff self-check: PASS. BOOTSTRAP present and within the line limit; candidate and dirty state disclosed; every common envelope field present; SESSION TYPE from the controlled vocabulary; evidence separated into observed, inferred and not-run; exactly one NEXT ACTION; worker role extension satisfied with per-criterion met and unmet evidence.
