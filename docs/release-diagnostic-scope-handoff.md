## BOOTSTRAP
TASK: Implement the prospective active-only release diagnostic gate without release or runtime changes.
STATE: Local implementation candidate; independent review and Human Acceptance are pending.
REVISION: base 79c09961455573547970826cbe2583981766a31e; candidate is the commit containing this handoff.
GUARDRAILS: no live probes, auth/quota reads, push, PR, merge, release, tag, installer, or workflow edits.
NEXT ACTION: Independent reviewer: review the commit containing this handoff against the release-scope safety boundary.

## STATUS
IN REVIEW — implementation candidate only; not independently reviewed or accepted.

## RESULT / VERDICT
ROLE: worker. TASK: prospective release diagnostic scope.
OBJECTIVE (operator wording): “Implement the operator-approved **active-only release diagnostic gate** as a narrowly scoped, safe FOLLOW-UP on a NEW isolated branch based on the already merged v0.8.29 main. No production release or merge in this task. This rule must make provider live checks optional ONLY when the provider has been explicitly declared inactive, retaining true results and safe defaults.”
DELIVERABLE / ACCEPTANCE (operator wording): “Add concise technical HANDOFF, then one local feature commit, clean worktree and evidence-based closeout. Independent review will be separate. DO NOT push or release.”

The local declaration validates against actual registry families before scheduling. Missing declarations default to REQUIRED. Inactive Codex covers both account IDs; each retains an UNVERIFIED row. Reasons are never printed. Rust scans the report before printing; the wrapper also withholds output until its scan passes. Required non-live outcomes never become PASS.

## EXECUTION METADATA
MODEL: p696c58/gpt-6.1-terra
REASONING / THINKING EFFORT: high
HARNESS: T3 Code / Codex
SESSION TYPE: implementation
SUBAGENTS USED: 0

## TARGET / CANDIDATE
Repository: datawarsaw/limitscope; branch: chore/release-gate-provider-scope-20261117.
Base observed 2026-11-17: 79c09961455573547970826cbe2583981766a31e, matching origin/main.
Before handoff/amend, observed HEAD 2fe04cb with uncommitted subsequent safety changes; candidate revision is the commit containing this handoff. Reverify HEAD and clean state after commit.
Changed paths: scripts/provider-quota-diagnostic.ps1; src-tauri/src/diagnostic_report.rs; docs/release-runbook-v0.8.0.md; this handoff.

## EVIDENCE
- Filtered `cargo test --locked --bin rate-limits diagnostic_report::tests::release_scope -- --test-threads=1`: 2 passed, 0 failed, 1203 filtered; no live probe.
- PowerShell parser: PASS. `npm run scan:secrets`: PASS, but scans only src-tauri/tauri.conf.json. Added-line credential-shape scan: PASS; `git diff --check`: PASS before handoff.
- No full frontend suite: no frontend changes; operator directed targeted tests. No live provider diagnostic or auth/quota content read.
- The first fixture version called `production_specs()`; it can canonicalize Codex home directory metadata. The fixture was replaced with pure IDs, and final tests use only fixtures. The existing production registry construction can also inspect that directory metadata before selection; excluded fetch functions are not called.
- Runtime provider adapters and release workflow are unchanged. No release or delivery action was performed.

## RISKS / BLOCKERS
The declaration is an operator assertion, not detection of activation. It must be recertified on the current UTC day and checked whenever activation changes. Scope reasons stay in the local file; keep it secure. Diagnostic PASS is valid only after both secret scans pass. Independent review and Human Acceptance have not occurred. No user-specific declaration is committed. Existing `production_specs()` construction may resolve Codex-home directory metadata as described above; it does not invoke excluded fetches.

## NEXT ACTION
Independent reviewer: review the commit containing this handoff against the release-scope safety boundary.

Handoff self-check: PASS for envelope and worker fields; final commit identity and clean status must be verified after persistence.
