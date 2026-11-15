## BOOTSTRAP
TASK: Persist completed 0.8.25 G6/G7 QA as one local QA/docs-only commit (2026-11-15).
Source candidate: 2c4e88a4a3b1df2116ef4a391d827a5e931fe228; branch integrate/0-8-25-parity-20261115.
RESULT: REPO_HANDOFF_PASS for persistence; native state remains G7_NATIVE_PASS / G6_PARTIAL.
Stable HOLD. No PR #36/#37 merge. No push, build, native run, host app/profile/registry access or changes.
NEXT ACTION: Operator separately authorizes one fresh isolated G6 test to confirm runtime data location and actual four-store readers.

## RESULT / STATE
STATUS: COMPLETE for repository persistence only; release acceptance remains incomplete.
ROLE: delivery. Only `scripts/qa/g67-guest.ps1` and this handoff are delivered.
Existing source-level G7 fix was independently reviewed before this task (operator-provided history); no new review claimed.

## TARGET / CANDIDATE
Existing worktree: `C:/Users/micha/.t3/worktrees/limitscope/integrate-0-8-25-parity-51869fdf4bbd6b71`.
Branch and base above verified clean, including untracked files, before edits on 2026-11-15.
This document is part of the single local child commit of that base; exact commit is available with `git log -1 --format=%H -- docs/qa-g67-handoff.md`.
Application source remains identical to base. Persistence does not install or activate anything.
UNSIGNED 0.8.25 NSIS SHA256 `8ae7ed458c977ac75d5484ebcdb7b373a3455687bcdeb41ea86ad0830428d049` was built from the BASE above, not this documentation commit; remains valid as that source artifact.

## EVIDENCE
Completed [T3 G67 rerun](https://app.t3.codes/threads/ffed14d0-5327-45b5-a000-137eeffd16da/mcp%3Alaunch%3Ac632126018c6331103bd8014f365574922fcfc80570c2a5209753409255430a1).
Prior [T3 harness fix](https://app.t3.codes/threads/ffed14d0-5327-45b5-a000-137eeffd16da/mcp%3Alaunch%3A0456ef52b51d9eb1d2cc69ae8bd2c7ad62bfe2b29d53434268d0da1523cc77d5).
Thread API returned `thread_not_found` during persistence; local artifacts below were readable. No remote execution evidence newly verified.

Local-only rerun directory: `C:/AI/LimitScope_G67_QA_20261115_2c4e88a_rerun_8d91c027f5aa/`:

- [result.json](C:/AI/LimitScope_G67_QA_20261115_2c4e88a_rerun_8d91c027f5aa/result.json), SHA256 `563C98087B2C09ADB3E5543B1A84445C34274E08BDC0668C1AABF5B7738A4304`.
- [evidence-sha256.json](C:/AI/LimitScope_G67_QA_20261115_2c4e88a_rerun_8d91c027f5aa/evidence-sha256.json), SHA256 `96BA6C47A594CEEDA8EC5BA265943AEB68806BFF454E36F9922C9FB82248DE4E`; links hashes of original captures. Host snapshots, screenshots, prepare.ps1, matching inlined WSB and reader-source-trace.json remain there, outside Git.
- Script copied from [validated fixed script](C:/AI/LimitScope_G67_QA_20261115_2c4e88a_3b2f7b56ec8b/guest-script-run-target-fixed.txt), SHA256 `629A43A07DB23261BC0834A0948F262357F2028DA02431FA051AD5CD5A4F8B93`, verified before copying; also recorded by rerun. Historical files untouched. No binaries, captures, manifests or production material committed.

Observed historical result: genuine 0.8.24 NSIS -> 0.8.25; single ARP 0.8.25; single correct installed Run target with `--hidden`, no registry repair: G7_NATIVE_PASS.
Direct 0.8.25 -> 0.8.24 NSIS rollback succeeded without fallback. Genuine 0.8.25 Settings write `Remaining` was read by 0.8.24: PASS for that setting only.
Four synthetic files survived all five snapshots with zero recovery sidecars in the seeded directory; actual store reads UNPROVEN because seed used LocalApplicationData. No G6 PASS or product data-loss verdict.
Guest integrity was High; Medium normal-user QA remains required. Old 0.8.24 candidate production origin/equivalence UNVERIFIED. Host unchanged per historical pre/post evidence; no new host inspection performed.

## SCRIPT / VALIDATION
Minimal changes to the tested script: guest/environment guard moved ahead of any writes/shutdown handling (from the tested loader); removed literal host computer name; seed base changed from LocalApplicationData to ApplicationData. No credentials found in copied script; fixtures are synthetic.
Static path proof: runtime.rs:4830-4870 opens all four stores with `app.path().app_data_dir()`; tauri.conf.json identifier is `com.ratelimits.desktop`, with no appDirectoriesOverride. Locked Tauri 2.12.0 `src/path/desktop.rs:251-256` calls dirs::data_dir() plus identifier; `src/path/mod.rs:398-425` uses default when no override. Tauri Cargo.toml selects dirs 7; dirs-7.0.0/src/win.rs:10 calls dirs_sys::known_folder_roaming_app_data(), whose dirs-sys-0.5.0/src/lib.rs:176-177 resolves FOLDERID_RoamingAppData. Sources inspected offline under `C:/Users/micha/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Thus .NET ApplicationData is the Windows roaming known-folder equivalent. This proves candidate source semantics, not actual guest reading or old installer provenance.
Quoted/unquoted ParseRun/RunAccepts, installed PE/path/existence checks, no fallback/repair and one-pass install/launch counts preserved. Final G6 verdict remains partial.
No launcher included: future operator must separately approve exactly one fresh sandbox, offline with only read-only C:\G67Input and empty writable C:\G67Results maps, no host profiles; C:\G67Local must not exist. Validate mappings outside guest; runtime cannot fully enumerate VSMB. Reuse pinned installer hashes; UI enable/observe/quit steps remain manual. Never invoke on host. No native test authorized by this document.
Offline Windows PowerShell 5.1 Parser.ParseFile: zero AST errors. Only extracted pure ParseRun and RunAccepts definitions evaluated: quoted target accepted, unquoted target accepted, stale target rejected (3/3); script body never executed. `git diff --check` and exact staged diff inspection required before commit; postcommit checks verify only these two paths, clean tree and no application-source delta.

## RISKS / BLOCKERS
G6 four-store reader and corrected guest path remain dynamically unverified. Static correction does not upgrade old results. Medium QA and production origin remain open; Stable HOLD, no PR #36/#37 merge. All original results remain local, not in Git. No source features proposed.
NEXT ACTION: Operator separately authorizes one fresh isolated G6 test to confirm runtime data location and actual four-store readers.

## EXECUTION METADATA
MODEL: p696c58/gpt-6.1-sol (runtime-reported).
REASONING / THINKING EFFORT: medium (runtime-reported).
HARNESS: T3 Code through Codex.
SESSION TYPE: delivery.
SUBAGENTS: 0. SUBAGENTS USED: 0.
Git: local commit only; no push, PR, tag, merge or release. No installation/runtime activation.
PROJECT_STATE_UPDATED: NOT_REQUIRED (bounded LimitScope repository persistence, no Agent Platform state change).
LINEAR_UPDATED: NOT_REQUIRED (no ticket work authorized).
MANUAL_ACTION_REQUIRED: Operator separately authorizes one fresh isolated G6 test to confirm runtime data location and actual four-store readers.
CAN_ARCHIVE: YES for this persistence task only; native/release acceptance remains open.
Handoff self-check: PASS; BOOTSTRAP <=15 lines, bounded task/base/guardrails/result, common fields, delivery metadata, evidence limits and one next action present. No canonical schemas duplicated.
