## BOOTSTRAP
TASK: Persist completed 0.8.25 G6/G7 QA as one local QA/docs-only commit (2026-11-15).
Source candidate: 2c4e88a4a3b1df2116ef4a391d827a5e931fe228; branch integrate/0-8-25-parity-20261115.
RESULT: REPO_HANDOFF_PASS for persistence; native state is G7_NATIVE_PASS / G6_NATIVE_READERS_PASS (High-token functional only).
Stable HOLD. No PR #36/#37 merge. No push, build, native run, host app/profile/registry access or changes.
NEXT ACTION: Operator separately decides Medium nonadmin guest environment preparation (see LOOKAHEAD); no UAC/token tricks.

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

## G6 NATIVE READERS PASS (2026-11-15, one offline High-token Sandbox)
Completed [T3 G6 one-guest QA](https://app.t3.codes/threads/ffed14d0-5327-45b5-a000-137eeffd16da/mcp%3Alaunch%3A1ab2ac786d35ded189593f6df6064f8833420b69ec8ff672163208ccc28c7e3f).
Local-only evidence directory `C:/AI/LimitScope_G6_QA_20261115_818229f_23eff2c15265/` (no local symlink; not in Git):
- [host-closeout.json](C:/AI/LimitScope_G6_QA_20261115_818229f_23eff2c15265/host-closeout.json) — external host verdict `G6_NATIVE_READERS_PASS` with observed UI/JSON evidence; SHA256 `551C8AEDC93F1B472612B9F4199D3EABF7895DB850D7D674D085F08B8D2D2BC4`.
- [evidence-sha256.json](C:/AI/LimitScope_G6_QA_20261115_818229f_23eff2c15265/evidence-sha256.json) — SHA manifest over all captures; SHA256 `0D8A229AEBAFA77B8974749BC4A744560F59AA27F6AC75A2E5E579AE394C3820`.
- [guest-private.ps1](C:/AI/LimitScope_G6_QA_20261115_818229f_23eff2c15265/guest-private.ps1) — validated guest script with the fixture corrections now brought into the versioned harness; SHA256 `6B8E05AF8511EC5586B4425F7DA6FAA7EF94A7FEA1263559CA3B21576923B8FA`.
- [static-validation.json](C:/AI/LimitScope_G6_QA_20261115_818229f_23eff2c15265/static-validation.json) — PS 5.1 AST 0 errors, exact four-line fixture diff, decoded byte-identical seed sample, wallet signal `deepseek:false differs from default true`.

Observed result: G7_NATIVE_PASS — real 0.8.24 -> 0.8.25 same-brand installer Run/ARP transition, single ARP entry, exact `--hidden` Run target, no registry repair. G6_NATIVE_READERS_PASS under one offline High-token Sandbox: four distinguishable old/new native readers verified — provider cache 42.5 used -> 58 remaining; wallet `deepseek:false` (distinct from `true` default); usage-intelligence enabled; quota-history export exact row `zai/5-hour/usedPercent=30.0` from old 0.8.24 after direct rollback ([limitscope-usage-20261115T150907Z.json](C:/AI/LimitScope_G6_QA_20261115_818229f_23eff2c15265/results/limitscope-usage-20261115T150907Z.json), SHA256 `50C8E51945136E78C421D0F74582BB14148EDA894C99857C97EC0A3BC8482276`). `Remaining` writer 0.8.25 -> reader 0.8.24 PASS. Exactly 1 guest, 3 installs, 3 app starts, 0 guest processes remaining, all 10 host invariants PASS ([host-comparison.json](C:/AI/LimitScope_G6_QA_20261115_818229f_23eff2c15265/host-comparison.json)).
Scope limits: G6 native PASS covers functional compatibility in the High guest only; Medium normal-user QA and exact production 0.8.24 binary lineage remain unresolved. The raw [guest-result.json](C:/AI/LimitScope_G6_QA_20261115_818229f_23eff2c15265/results/guest-result.json) intentionally retains its conservative script-level `G67_PARTIAL` marker (script cannot observe manual UI) and is unchanged; the verified result lives only in host-closeout.json. One optional shutdown.txt marker missing is a minor P3 gap. Stable HOLD, PR #36/#37 DO NOT MERGE.

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
2026-11-15 harness fixture update: exactly the four `$seed` table lines from the validated private copy were brought into `scripts/qa/g67-guest.ps1` — wallet `deepseek:false` (distinguishable from the `true` default) and guest-local dynamic UTC timestamps at seed time for provider last-good, quota history and usage-intelligence `collectionStartedAt` (UtcNow minus 5 minutes observation, plus 4 hours future reset) so fixtures survive 7-day retention instead of the stale 2026-09-29 dates. Fixtures remain synthetic healthy schemas (`zai`, `Account A`, `key:1234` placeholder; no real identifiers or credentials). The quoted/unquoted `HKCU Run` path parser, mapping/isolation/fail-closed gates and single Sandbox launch semantics are untouched; no other private-copy lines were adopted.
Static path proof: runtime.rs:4830-4870 opens all four stores with `app.path().app_data_dir()`; tauri.conf.json identifier is `com.ratelimits.desktop`, with no appDirectoriesOverride. Locked Tauri 2.12.0 `src/path/desktop.rs:251-256` calls dirs::data_dir() plus identifier; `src/path/mod.rs:398-425` uses default when no override. Tauri Cargo.toml selects dirs 7; dirs-7.0.0/src/win.rs:10 calls dirs_sys::known_folder_roaming_app_data(), whose dirs-sys-0.5.0/src/lib.rs:176-177 resolves FOLDERID_RoamingAppData. Sources inspected offline under `C:/Users/micha/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Thus .NET ApplicationData is the Windows roaming known-folder equivalent. This proves candidate source semantics, not actual guest reading or old installer provenance.
Quoted/unquoted ParseRun/RunAccepts, installed PE/path/existence checks, no fallback/repair and one-pass install/launch counts preserved. G6 verdict is now the externally observed G6_NATIVE_READERS_PASS above; script-level marker stays conservative by design.
Offline validation 2026-11-15 (this commit's script): Windows PowerShell 5.1 Parser.ParseFile — 0 AST errors. Pure ParseRun/RunAccepts extraction evaluated: quoted target accepted, unquoted target accepted, stale target rejected (3/3). Static fixture checks: `deepseek:false` present, dynamic UTC replacement in place for all three timestamped stores, static September timestamp literals removed from the seed table. Script body never executed.
Transparency note: the first host-local validation accidentally evaluated a broader fragment of the guest script via Invoke-Expression and immediately hit the guest guard `WRONG_GUEST_IDENTITY` before any app/installer/registry operations; no guest was launched and no side effects observed. The corrected parser-only extraction passed 3/3. The whole script was not rerun on the host.
`git diff --check` and the exact two-path diff were inspected before the single local commit; postcommit checks verify only these two paths, clean tree and no application-source delta.
No launcher included: future operator must separately approve exactly one fresh sandbox, offline with only read-only C:\G67Input and empty writable C:\G67Results maps, no host profiles; C:\G67Local must not exist. Validate mappings outside guest; runtime cannot fully enumerate VSMB. Reuse pinned installer hashes; UI enable/observe/quit steps remain manual. Never invoke on host. No native test authorized by this document.
Prior-rerun offline validation 2026-11-15: Windows PowerShell 5.1 Parser.ParseFile — zero AST errors; only extracted pure ParseRun/RunAccepts definitions evaluated (quoted accepted, unquoted accepted, stale rejected, 3/3); script body never executed.

## RISKS / BLOCKERS
G6 native readers are verified in the High guest only; Medium normal-user QA and production 0.8.24 origin remain open; Stable HOLD, no PR #36/#37 merge. All raw results remain local, not in Git. No source features proposed.
LOOKAHEAD (2026-11-15): readiness for a true Medium nonadmin guest QA is BLOCKED — no installed VM-management tooling is present on this host per a read-only GLM agent check. Enabling it will need separately approved environment preparation (e.g. Hyper-V/VM platform decision) or a proven existing standard-user path. Do not instruct risky UAC/token tricks and do not accept a token that is not genuinely Medium/nonadmin. Catalog provider identity and any served upstream model mapping remain unverified and must not be presented as such.
NEXT ACTION: Operator separately decides and authorizes Medium nonadmin guest environment preparation or an existing standard-user path.

## EXECUTION METADATA
MODEL: p696c58/gpt-6.1-sol (runtime-reported).
REASONING / THINKING EFFORT: medium (runtime-reported).
HARNESS: T3 Code through Codex.
SESSION TYPE: delivery.
SUBAGENTS: 0. SUBAGENTS USED: 0.
Git: local commit only; no push, PR, tag, merge or release. No installation/runtime activation.
PROJECT_STATE_UPDATED: NOT_REQUIRED (bounded LimitScope repository persistence, no Agent Platform state change).
LINEAR_UPDATED: NOT_REQUIRED (no ticket work authorized).
MANUAL_ACTION_REQUIRED: Operator separately decides and authorizes Medium nonadmin guest environment preparation or a proven existing standard-user path; no risky UAC/token tricks.
CAN_ARCHIVE: YES for this persistence task only; native/release acceptance remains open.
Handoff self-check: PASS; BOOTSTRAP <=15 lines, bounded task/base/guardrails/result, common fields, delivery metadata, evidence limits and one next action present. No canonical schemas duplicated.
