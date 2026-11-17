## BOOTSTRAP
TASK: Implement the Claude Code quota-freshness correction as a local candidate for independent review and Human Acceptance.
BASE: 890b8052abc7401fbf93dd8f9b0ec4f7c15ffe86; candidate is the commit containing this file.
RESULT: READY FOR REVIEW, subject to independent review and Human Acceptance; no release decision.
GUARDRAILS: isolated worktree only; no push, PR, tag, release, live Claude requests, credentials, or app-data inspection.
NEXT ACTION: Independent reviewer reviews the commit containing this handoff and its Claude freshness, history, notification, and compatibility boundaries.

## STATUS
READY FOR REVIEW. Implementation and local commit only; review, Human Acceptance and release remain separate.

## RESULT / VERDICT
ROLE: worker; TASK: bounded Claude statusLine freshness correction.
OBJECTIVE (verbatim): "Implement a small, backward-compatible LimitScope Claude Code quota-freshness correction as a tested local commit and short technical HANDOFF, ready for independent review and Human Acceptance. This is the v0.8.27 feature candidate, NOT a release."
The prompt did not label separate DELIVERABLE or DONE WHEN fields. Its required outcomes are addressed below; native app acceptance remains untested.
- Same-window equal/lower values and rounded reset variations do not advance timestamps; increases do. A different reset while the stored window remains open is ignored as ambiguous; a new window is accepted after expiry. Existing v1 timestamps cannot be retroactively corrected.
- Snapshot v1, persistent lock, atomic replacement and checks remain; per-window source time is internal and skipped by serialization. History and notification observations use each window's own time. Aggregate health uses the oldest displayed window.
- Claude surfaces show Recent / Last known and age; stale percentages remain visible until reset. Other providers keep their presentation. No timer was enabled or settings read/modified; a timer would rerun local ingestion, not prove a quota fetch.
- New tests cover replay after 15 minutes, independent timestamps, lower/rounded session replay, both resets, persistence, history provenance, alerts and UI. Existing tests cover missing/invalid windows and concurrent processes.

## EXECUTION METADATA
MODEL: p696c58/gpt-6.1-terra (runtime reported); upstream provider UNVERIFIED (no separate serving-provider evidence).
REASONING / THINKING EFFORT: high (runtime reported). HARNESS: T3 Code through Codex. SESSION TYPE: implementation.
SUBAGENTS: 0. SUBAGENTS USED: 0.

## TARGET / CANDIDATE
Repository/worktree: C:/Users/micha/.t3/worktrees/limitscope/feature-claude-freshnes-0dcb19af0e04c21c
Branch: feature/claude-freshness-20261117. Base: 890b8052abc7401fbf93dd8f9b0ec4f7c15ffe86. Date: 2026-11-17.
Candidate HEAD: the commit containing this handoff (resolve with `git log -1 --format=%H -- docs/claude-freshness-handoff.md`); at authoring HEAD was base and changes were uncommitted. Recheck HEAD and dirty state before review.
Changed: Claude ingestion, runtime projection, history provenance, Claude UI/setup, focused tests and mechanical `observed_at: None` initializers for other DTO constructors. No other provider behavior was intentionally changed.
Protected: C:/AI/limitscope and unrelated worktrees, settings, user data, release/version surfaces.

## EVIDENCE
- `cargo test --offline claude_`: 29 passed, including concurrent process lock and Claude alert restart.
- `cargo test --offline notifications::tests`: 52 passed (before the additional Claude-specific test, which passed in the 29-test run).
- `npm test -- src/components/ClaudeStatuslineProvider.test.tsx src/lib/v03Integration.test.ts src/components/FloatingQuotaBar.test.tsx`: 93 passed.
- `npm run build`: TypeScript and Vite passed. `git diff --check`: passed before commit.
- Full `rustfmt --check` is not clean: released baseline itself produced 466 lines of formatting differences in this module. Changed blocks were formatted without reformatting the entire file.
- Official [statusLine documentation](https://code.claude.com/docs/en/statusline) describes optional quota fields after responses and timer-driven command reruns; it does not establish timer-driven upstream quota fetches. The [Analytics API](https://platform.claude.com/docs/en/manage-claude/claude-code-analytics-api) concerns organization analytics, not individual Pro quota.
- Native installed app, real Claude activity, credentials, paid requests, app-data quota files, installer and public feed: not exercised.

## RISKS / BLOCKERS
Higher same-window usage is new information but is not proof of the current server state; unchanged real usage may conservatively age out even while Claude is active. The v1 file has no way to distinguish pre-fix inflated timestamps: they age out naturally, never rewritten by identical replays. A stale older sibling keeps the aggregate stale and prevents history/alerts for both until all displayed windows are recent. UI age is the oldest displayed window. No independent review or Human Acceptance has occurred.

## HUMAN ACCEPTANCE INSTRUCTIONS
After independent review, an operator may use a separately approved isolated synthetic fixture or their own opt-in Claude setup: verify Recent then Last known after 15 minutes of unchanged readings, retained percentages and age in Providers & Wallets and Floating Deck, and a later changed reading / reset. Avoid treating a rerun timer as server confirmation; verify missing windows do not display 0%. This candidate run made no live Claude request.

## NEXT ACTION
Independent reviewer reviews the commit containing this handoff and its Claude freshness, history, notification, and compatibility boundaries.

Handoff self-check: PASS; bounded contract, base/candidate identification, evidence, limits, metadata, and one next action present.
