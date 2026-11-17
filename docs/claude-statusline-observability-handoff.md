## BOOTSTRAP
TASK: Implement the v0.8.28 Claude statusLine observability feature as one tested local commit and technical handoff.
BASE: 0ad8260d1bd69fa29ab21ded7ce6d9095a63cd85; candidate is the commit containing this handoff.
STATE: READY FOR REVIEW; independent review and Human Acceptance have not occurred.
GUARDRAILS: isolated worktree only; no private data, live Claude requests, settings, push, PR, release, tag or installer.
NEXT ACTION: Independent reviewer reviews the commit containing this handoff and its receipt, quota, privacy and compatibility boundaries.

## STATUS
READY FOR REVIEW; local implementation only, no release.

## RESULT / VERDICT
ROLE: worker. TASK: bounded Claude statusLine receipt and per-window change observability.
OBJECTIVE (verbatim): "Implement a small, backward-compatible LimitScope v0.8.28 feature: show independently (1) time of last valid Claude Code statusLine event received locally, (2) last defensibly changed 5-hour quota, and (3) last defensibly changed 7-day quota. Produce one TESTED LOCAL feature commit and technical HANDOFF; separate review and Human Acceptance follow. No release."
DELIVERABLE / ACCEPTANCE: The objective above specifies the local commit, handoff and separate subsequent gates; the prompt has no separately labelled DONE WHEN section. Required actions are evidenced below.

Receipt stores only a timestamp in a separate sidecar under the existing persistent lock, at most once per 60 seconds. A usable quota window is accepted as before; with no usable window, a document must contain nonempty `session_id` and `model.id` or `model.display_name`. These fields are inspected, never retained. Writes are atomic and best effort; quota storage and health are independent. Future timestamps display Unknown until the clock catches up, preserving monotonicity. Corrupt sidecars recover on the next valid event.

The optional Claude-only DTO carries the receipt and each displayed window's existing source time. Both detailed surfaces show all three ages, including Unknown, and explain that a local event does not establish a quota refresh. Aggregate freshness and v1 `deny_unknown_fields` remain unchanged. Expired or absent windows show Unknown rather than zero. Existing v1 observations retain their prior limitations.

## EXECUTION METADATA
MODEL: p696c58/gpt-6.1-terra (runtime reported); served provider UNVERIFIED (no independent provider evidence).
REASONING / THINKING EFFORT: high (runtime reported). HARNESS: T3 Code through Codex. SESSION TYPE: implementation.
SUBAGENTS: 0. SUBAGENTS USED: 0.

## TARGET / CANDIDATE
Worktree: C:/Users/micha/.t3/worktrees/limitscope/feature-claude-statusli-384ef08fe3d16df3
Branch: feature/claude-statusline-observability-20261117. Base and pre-edit HEAD: 0ad8260d1bd69fa29ab21ded7ce6d9095a63cd85; origin/main verified at that SHA on 2026-11-17. Candidate: commit containing this handoff, resolvable by `git log -1 --format=%H -- docs/claude-statusline-observability-handoff.md`. At authoring, changes were uncommitted; recheck HEAD and cleanliness before review.
Changed: Claude ingest and runtime, Claude-facing UI and tests, optional DTO constructor initializers, this handoff. No other provider behavior intended.

## EVIDENCE
- `cargo test --locked --offline --no-run`: PASS.
- `cargo test --locked --offline claude_statusline::tests:: -- --skip ingest_directory_is_the_directory_the_app_resolves`: PASS, 21 tests; fixture directories and child processes only.
- Individually selected synthetic Rust tests for receipt projection, stale history gate, per-window history and replay notification: PASS, 4 tests.
- `npm test -- src/components/ClaudeStatuslineProvider.test.tsx src/lib/v03Integration.test.ts src/components/FloatingQuotaBar.test.tsx`: PASS, 94 tests.
- `npm run build`: PASS after `npm ci`; initial attempt failed because this worktree lacked `tsc`. `npm ci` reported 6 dependency vulnerabilities (3 moderate, 1 high, 2 critical); no dependency updates attempted.
- `git diff --check`: checked at closeout. Full unfiltered cargo test, mock app resolution test, real user cache, native app/installer and live Claude: SKIP by safety scope. Repository `scan:secrets` reads private signing keys and was not run.
- Official reference: https://code.claude.com/docs/en/statusline. CLI events are local execution evidence, not evidence of an upstream quota refresh.

## RISKS / BLOCKERS
No independent review or Human Acceptance. A far-future but syntactically valid sidecar timestamp stays Unknown until the system clock catches up; replacing it earlier would violate monotonicity. Receipt may lag by up to 60 seconds while events continue, or remain older if a write fails. Unchanged real quota values can remain Last known. No private data, settings, release surfaces or other worktrees were inspected or modified.

## NEXT ACTION
Independent reviewer reviews the commit containing this handoff and its receipt, quota, privacy and compatibility boundaries.

Handoff self-check: PASS; candidate identity, scope, evidence, limitations, metadata and one next action recorded.
