## BOOTSTRAP
TASK: Deliver a read-only, account-scoped Codex Reset Bank in the expanded floating card.
STATE: Local implementation candidate; independent review and Human Acceptance remain separate gates.
BASE: a16e2d563a93539ea004285abd9612a6d33bb947 (released v0.8.28).
CANDIDATE: The commit containing this handoff; obtain its exact SHA with `git rev-parse HEAD`.
GUARDRAILS: No push, PR, merge, tag, signing, publish, deploy, install, version change, auth access, extra endpoint, redemption, or backend mutation.
PROTECTED: Do not mutate C:\AI\limitscope or C:\AI\Token_Monitor, or other worktrees.
NEXT ACTION: Reviewer independently review the local candidate commit in this worktree.

## STATUS
IMPLEMENTATION CANDIDATE; NOT REVIEWED OR ACCEPTED. Inherited release remains v0.8.28; Windows UAT status is unknown.

## RESULT / VERDICT
ROLE: worker. TASK: account-specific floating Codex Reset Bank, using existing credit formatter and its 15-minute freshness guard. Account selection remains the existing selected floating item. Expiry is not reported by the current DTO and is labelled accordingly.

## EXECUTION METADATA
MODEL: p696c58/gpt-6.1-terra (runtime reported)
REASONING / THINKING EFFORT: high (runtime reported)
HARNESS: T3 Code / Codex
SESSION TYPE: implementation
SUBAGENTS USED: 0

## TARGET / CANDIDATE
Worktree: C:\Users\micha\.t3\worktrees\limitscope\feature-codex-account-r-9bc685f4a082d010
Branch: feature/codex-account-reset-bank-20261117. Observed 2026-11-17.
Base HEAD: a16e2d563a93539ea004285abd9612a6d33bb947; clean before edits.
Candidate identity: containing commit (self-referential SHA cannot be embedded); verify with `git rev-parse HEAD`.
Tracked changes: `src/components/FloatingProviderPopover.tsx`, `src/components/FloatingProviderPopover.resetCredits.test.tsx`, `src/floating.css`, `src/lib/floatingQuota.ts`, `src/lib/resetCredits.ts`, and this handoff.
Ignored temporary `node_modules` copy and `dist` build output may remain in this worktree: cleanup was rejected by tool policy. They are not committed. Inspect status before continuing.

## ACCEPTANCE CRITERIA
OBJECTIVE (verbatim): "Implement the smallest value-delivering **account-scoped Codex Reset Bank read-only UI** in LimitScope, based on the previously recovered E2 prototype, as one isolated local candidate commit and technical handoff. Do NOT publish or deploy."

DELIVERABLE: No separately labelled deliverable was provided; the objective above specifies the local commit and technical handoff.
DONE WHEN / ACCEPTANCE CRITERIA: No separately labelled DONE WHEN was provided. The following scope instructions are verbatim:

> 2. Add a small section to the **expanded floating Codex account-specific detail card** showing account-bound banked reset credits and separately optional currentlyApplicable. Use existing formatter and freshness guard; missing/stale/malformed credit data must show a conservative 'unavailable' or omit section according to current UI convention, never present 0 by inference. Distinguish 0 applicable from 0 banked and unknown applicability. Account/alias from selected current provider ONLY (primary vs CoWork); never merge accounts, never fall back to primary for secondary, and never infer active credit from count. Preserve floating current width/shell behavior or use the minimal safe layout adjustment; do not affect compact bar, other provider cards, Settings, histories, notification semantics or persisted data.
>
> 3. DO NOT add a redemption command/button, bank credit consumption, extra GET endpoint, expiry data, provider auth or backend mutations. If expiry not available in current wire contract, explicitly display no expiry rather than pretending; no unsupported dates. Keep UI labels concise, consistent with production styling and honest semantics ('banked' vs 'currently applicable').
>
> 4. Add targeted component/formatter tests using synthetic fixtures: two independent account values, missing secondary observation, 0 vs unknown applicability, stale >15min, no path for non-Codex, identity swaps, no misleading 'resets available' wording. Prefer small deterministic safe tests; do not run unfiltered Rust native tests or live authenticated tests.
>
> 5. Run focused tests and frontend build only as needed to establish correctness. Produce one local feature commit and repository technical HANDOFF. Do not push, PR, merge, tag, sign, publish, install or change version. Keep user data, source roots and other worktrees untouched.

Criteria 2–4: implemented and covered by focused synthetic tests; shell sizing is inherited and Windows visual UAT remains unverified. Criterion 5: tests and build passed; commit is the containing commit. No release actions or app installation performed. Independent review and Human Acceptance are follow-up gates, not implied by implementation.

## EVIDENCE
- `git status --porcelain=v1 -b` and `git rev-parse HEAD` before mutation: clean branch at required base.
- `npm test -- src/components/FloatingProviderPopover.resetCredits.test.tsx src/lib/resetCredits.test.ts src/hooks/useProviderUsage.test.ts`: PASS, 42 tests in 3 files, 0 failures, 0 skipped.
- `npm run build`: PASS (TypeScript and Vite).
- `git diff --check`: PASS before handoff creation.
- Read historical 094527bd6553eeca898683e1eafe26f86d0506a4 selectively; no cherry-pick or extra endpoint.
- Dependencies copied from existing local node_modules into this worktree for validation; no package installation. No private auth, quota caches, or settings read. No live authenticated or Rust tests run.
- Modern web guidance runner was unavailable without installing; existing card patterns used.

## RISKS / BLOCKERS
Windows visual sizing and UAT are unverified. Independent review has not occurred. No evidence of Human Acceptance or release for this candidate. Cleanup of ignored local validation artifacts was rejected by tool policy; source checkouts were not modified.

## NEXT ACTION
Reviewer independently review the local candidate commit in this worktree.

Handoff self-check: PASS for bootstrap length, envelope, execution metadata, source provenance, worker criteria, and single next action. Candidate SHA is identified by containing commit and must be verified on checkout; no exact-head review is claimed.
