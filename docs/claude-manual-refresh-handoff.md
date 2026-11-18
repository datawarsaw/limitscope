## BOOTSTRAP
TASK: Implement the smallest reliable on-demand Claude quota refresh in LimitScope as a local, reviewable candidate: one explicit user action, no polling, at most one authorized live Claude request, no release or install.
BASE: dd60f764671e6cb4a1f5218ced97bfe66b55fc73 (published stable v0.8.33); candidate is the commit containing this file.
STATE: IMPLEMENTED in an isolated worktree; focused Rust and frontend checks pass; nothing installed, released or pushed.
RESULT: The Claude Code card now has one button, "Ping Claude (uses 1 request)", that starts the installed Claude CLI in a console of its own with one fixed prompt; the outcome reads "Claude launched; quota pending" and never claims freshness.
GUARDRAILS: no install, release, push, PR, tag or merge; no polling, schedule or service; no credential, transcript or private-API read; exactly one live Claude request, already spent.
NEXT ACTION: Independent reviewer reviews the commit containing this handoff and its launch, billing, privacy and ACL boundaries.

## STATUS
READY FOR REVIEW. Local implementation and a local commit only; independent review, Human Acceptance and release remain separate.

## RESULT / VERDICT
ROLE: worker. TASK: bounded on-demand Claude quota refresh.
OBJECTIVE (verbatim): "Implement the smallest reliable on-demand Claude quota refresh feature in LimitScope as a LOCAL, reviewable candidate: an explicit "Refresh Claude" user action (or existing equivalent interaction) that invokes the already installed Claude CLI and existing documented statusLine ingest path, with truthful success/freshness labels and no polling. Perform at most ONE tiny live Claude model request for validation, explicitly authorized by the operator. No release or install."
DELIVERABLE / ACCEPTANCE: the prompt stated no separately labelled DELIVERABLE or DONE WHEN section. Its "Required actions" 1-5 and its "Validation" are used below as the acceptance criteria, each with met or unmet evidence.

Required action 1 (inspect, reuse the present architecture, one explicit action, no polling) - MET. Only the Claude statusLine ingest module, the runtime projection, the IPC ACL surfaces and the existing manual-refresh precedents (Grok Bot, OpenDesign) were inspected. The implementation reuses them: one Tauri command granted to the main window, one button in the existing Claude Code card of the dashboard, and the existing ingest path untouched. The ingest flag, the receipt throttle and the window merge rules are unchanged. The action is explicit; no timer exists in the change.

Required action 2 (one real live Claude ping before fixing the mechanism; allow-listed comparison only) - MET. Exactly ONE live request was made, before the mechanism was chosen: claude -p "Reply exactly OK." --restricted --strict-mcp-config --output-format text, run in an empty scratch directory under the system temp dir that was removed afterwards and stayed empty. Result: exit 0 after about 5.4 s, with a four-character reply. Only the two allow-listed derived files were compared, before and after. Before: receipt {"timestamp":1795040813}; store fiveHour 0.0 percent with observedAt 1795039310 and sevenDay 37.0 percent with observedAt 1795039310. After the run, plus three seconds: receipt still 1795040813, store unchanged. The receipt is written at most once per 60 s and the gap was about 577 s, so the throttle cannot explain it: headless print mode did NOT run the statusLine hook in Claude Code 2.1.318. No further ping was made. No auth, settings, cookies or account were touched, and no session file, transcript or credential was read. The request may have consumed subscription quota.

Required action 3 (smallest truthful on-demand action; safe backend; no polling) - MET, with two recorded deviations. The backend command refresh_claude_usage resolves the installed claude.exe from a bounded list (two known install locations, then PATH) and starts it with a direct CreateProcessW call: no shell, no command-string interpolation, no caller-supplied argument. It runs in a console of its own, with the fixed prompt "Reply exactly OK." and the fixed arguments --restricted and --strict-mcp-config, and the system temp directory as working directory. Executable resolution accepts only an existing file named claude.exe; a second click while an attempt runs is refused; the failure states are distinct statuses; the scratch working directory and the restricted arguments keep the one-shot session away from the user's projects, from tools and from MCP servers. Deviations: (a) the prompt allowed a one-shot button running a minimal prompt only if the single ping had proved it safe, and it did not, so the button starts an interactive session instead of a headless one; (b) by later operator instruction the backend result was reduced to the attempt itself (launched, already running, not found, spawn failed), with no receipt polling, no wait loop and no synthetic freshness markers, because the provider runtime keeps owning the derived documents and refreshes them on its own schedule. The button therefore reports the attempt, never a measurement.

Required action 4 (focused deterministic validation; local commit only; technical handoff) - MET. Five new Rust tests and five new frontend tests, plus the existing ACL and guardrail suites that the new command touches. No repeated paid prompt and no benchmark.

Required action 5 (close out explicitly, stop at Human Acceptance) - MET. This handoff plus the run summary. No release, install, push, PR, tag, merge, or change to the deployed app.

Validation section.
- "Prove local button or command initiates exactly one CLI refresh attempt and shows outcome based on available evidence" - MET for the attempt, with deviation (b) above: the command performs exactly one spawn attempt and reports that attempt.
- "Distinguish attempted / hook received / valid quota windows accepted / fresh server change" - NOT implemented inside the command, by operator instruction. The existing provider surfaces keep that distinction (last statusLine event, 5h usage changed, 7d usage changed) and the runtime refreshes them; the command adds no second freshness vocabulary.
- "Do not conflate unchanged percentages with failure" - MET. The command reports the attempt and never a percentage.
- "Use the smallest relevant tests only" - MET.
- "Report UNVERIFIED for missing external evidence" - MET, see below.

UNVERIFIED.
- That an interactive Claude session started this way runs the statusLine hook, and that the windows arrive after the one reply: not verified live, because no further Claude call was permitted. The design rests on the prior observation, reported in the task briefing, that the receipt moves on Claude CLI startup; that is second-hand evidence, not a measurement of this candidate.
- Whether Claude Code asks once to trust the scratch directory in this configuration: not observed live.

## EXECUTION METADATA
MODEL: minimax/MiniMax-M3.1-Flash (runtime reported); upstream serving provider UNVERIFIED (no separate provider evidence).
REASONING / THINKING EFFORT: max (runtime reported).
HARNESS: T3 Code through the Codex harness.
SESSION TYPE: implementation.
SUBAGENTS: 0. SUBAGENTS USED: 0.
The advisor tool was called twice and returned HTTP 404 both times, so no advisor findings exist.

## TARGET / CANDIDATE
Repository/worktree: C:/Users/micha/.t3/worktrees/limitscope/feature-claude-manual-r-36273ff7c7ace734
Branch: feature/claude-manual-refresh. Base and pre-edit HEAD: dd60f764671e6cb4a1f5218ced97bfe66b55fc73, which is the accepted published stable v0.8.33 SHA, so the baseline shows no drift. Date: 2026-11-18.
Candidate: the commit containing this handoff, resolvable with git log -1 --format=%H -- docs/claude-manual-refresh-handoff.md.
Changed: src-tauri/src/claude_cli.rs (new), src-tauri/src/main.rs (module and command registration), src-tauri/permissions/ipc-windows.toml and src-tauri/capabilities/default.json (one new permission, main window only), src-tauri/src/ipc_window_authorization.rs (command table and counts), src/components/ClaudeRefreshControl.tsx (new), src/components/ClaudeRefreshControl.test.tsx (new), src/components/dashboard/PrimaryQuotaPanel.tsx (one control on the Claude Code card), src/components/ClaudeStatuslineSetup.tsx (one sentence corrected: LimitScope itself makes no model call, the ping does), src/styles.css (button styles), src/lib/ipcWindowAuthorization.test.ts, src/lib/localDataBoundary.test.ts, src/lib/runtimeTriggerGuardrail.test.ts (command surface), docs/claude-statusline-provider.md, this handoff.
Not changed: the ingest module, the stored schema, the freshness and merge rules, the provider runtime, any other provider, the installed app, Claude Code's settings, auth or cookies, and the root checkout at C:/AI/limitscope.
Dirty state at authoring: the changes above were uncommitted; recheck HEAD and cleanliness before review.

## EVIDENCE
- cargo test --locked --offline claude_cli: 5 passed, 0 failed (resolution, one fixed prompt with no shell metacharacter, Windows quoting, one attempt at a time, and that no status or field ever claims freshness).
- cargo test --locked --offline ipc_window_authorization: 3 passed. The matrix now holds 37 registered commands, 26 dashboard grants, 15 floating grants, 41 allowed dispatches, 23 window-scope denials and 10 unresolved dispatches.
- npm test with the five focused files (ClaudeRefreshControl, ClaudeStatuslineProvider, ipcWindowAuthorization, localDataBoundary, runtimeTriggerGuardrail): 33 passed.
- npm test over the whole frontend suite: 94 files, 1661 tests, all passed.
- npx tsc --noEmit: exit 0. npm run build (tsc and vite build): built in 3.08 s.
- Live Claude request count: exactly one, described in Required action 2. No other Claude request was made.
- Windows console mechanics probe, no Claude involved: a standalone rustc program in the temp dir showed that Rust's std::process::Command hands the PARENT's console handles to a CREATE_NEW_CONSOLE child (the child's marker appeared in the parent console), while a direct CreateProcessW with STARTF_USESTDHANDLES unset gives the child its own console (the marker did not appear in the parent). The shipped code therefore uses CreateProcessW. Both probe artifacts were deleted afterwards.
- Observed but not verified against this candidate: the local Claude CLI is 2.1.318, and the configured statusLine command is the installed LimitScope.exe with the ingest flag.
- Not run: the full Rust suite, the native app, the installer, a live interactive Claude session, and the repository secret scan.

## RISKS / BLOCKERS
- UNVERIFIED (above): the interactive session's hook behaviour. If a session does not run the hook, the button will keep reporting "Claude launched; quota pending"; the remedy is a different launch form, not more Claude calls.
- The one-shot session runs in the system temp directory, so Claude Code may ask once whether that folder may be trusted, and the request completes only after the operator accepts. This is deliberate: it keeps the session away from the user's projects.
- The button costs one subscription request per click. The label states it, and no automatic, scheduled or repeated call exists in the change.
- Only claude.exe is accepted. A claude.cmd shim from an npm install is not launched, because it is not a console program and running it would require a shell.
- The Claude console window stays open after the reply; the operator closes it.
- No independent review and no Human Acceptance have occurred.

## HUMAN ACCEPTANCE INSTRUCTIONS
1. Run the candidate from this worktree (no install): the Claude Code card of the dashboard shows one button, "Ping Claude (uses 1 request)".
2. Click it once. Expect a Claude Code console window that replies OK, and the card to report "Claude launched; quota pending". If Claude asks to trust the temp folder, accept once.
3. Expect the card's values and its existing provenance lines (last statusLine event, 5h usage changed, 7d usage changed) to follow once Claude has answered. Unchanged percentages are expected when Claude was not used since, and are not a failure.
4. Close the Claude window and confirm the button is usable again; a second click while an attempt is running is refused.

## NEXT ACTION
Independent reviewer reviews the commit containing this handoff and its launch, billing, privacy and ACL boundaries.

Handoff self-check: PASS. BOOTSTRAP present and within the line limit; candidate and dirty state disclosed; every common envelope field present; SESSION TYPE from the controlled vocabulary; evidence separated into observed, inferred and not-run; exactly one NEXT ACTION; worker role extension satisfied with per-criterion met and unmet evidence.
