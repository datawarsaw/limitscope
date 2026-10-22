# Project State

## Identity

- Repository: `datawarsaw/limitscope`.
- Runtime: Windows Tauri desktop app, LimitScope.
- Distribution: signed releases and updater feed in `datawarsaw/limitscope-releases`.

## Current status

- v0.8.16 released: the combined floating-experience, monitor-recovery,
  DeepSeek-wallet and MiniMax-quota work is on `main`. Release commit
  `c597bc1f318bc9227bc64b48087b61ecb89dd31b`, annotated tag `v0.8.16`, published
  in `datawarsaw/limitscope-releases` as the live stable release. See
  `docs/release-verification-v0.8.16.md`.
- v0.8.15 released: API Wallets Phase 1 (OpenRouter prepaid balance) is on
  `main`. Release commit `c048d9ac0b1ec0c9e43ff44ec929ed08d8a9809c`, annotated
  tag `v0.8.15`, published in `datawarsaw/limitscope-releases` as the live
  stable release. See `docs/release-verification-v0.8.15.md`.
- v0.8.14 released: OpenDesign is a standard provider on `main`. Release
  commit `d640f052762e409461746b6ee7e2538c641a3b77`, annotated tag `v0.8.14`,
  published in `datawarsaw/limitscope-releases`. See
  `docs/release-verification-v0.8.14.md`.
- API Wallets Phase 1 merged as PR #18, merge commit
  `feaff647f8f6fd6c3ad0115129c4a6a614464311`.
- The combined v0.8.16 candidate merged as PR #19, merge commit
  `0a984dced406933007b4074fad1529c4d60dd629`, carrying the identical tree of
  the accepted candidate `404ad731847ab0c95c6518b8a36ce8012788d70a`.
- Technical baseline for the release: `0a984dced406933007b4074fad1529c4d60dd629`.
- Historical dirty checkout `C:/AI/Token_Monitor` is outside the mutation scope.

## Architecture and boundaries

- Rust owns provider collection, normalized status and runtime snapshot fan-out.
  The dashboard and floating window consume the same provider entries.
- OpenDesign `opendesign` is appended after Qwen in registry order. Its only
  collection path is explicit, read-only Chrome UIA refresh. Automatic cycles
  read process-local cached results and never drive browser interaction.
- The two independent windows are `5-hour` and `7-day`; floating primary is
  `7-day`. Optional `resetText` preserves source-relative text. No absolute
  reset is invented. See `docs/opendesign-provider.md`.
- Existing providers, ZCode Reset Cards, OpenCode Go and signing infrastructure
  retain their established behavior. Z.ai `unexpected_response` is out of scope.
- Prepaid API wallets are a separate snapshot list (`wallets`), not providers:
  no percentages, reset windows or period, and they never enter quota history,
  prediction or notifications. OpenRouter reads the official credits endpoint
  with the OpenCode auth entry; the key never leaves the process.
- API Wallets Phase C (candidate): DeepSeek's prepaid balance joins OpenRouter
  in the same wallet list, read from the official balance endpoint
  (`GET https://api.deepseek.com/user/balance`). The credential is resolved
  read-only from the OpenCodex provider config, falling back to the OpenCode
  auth file; no credential record is created or modified. Money is carried per
  currency (USD, CNY): amounts are never summed, converted, or read as USD
  when they are not, and the floating bar keeps exactly one shared 44 px
  wallet slot whose card lists every wallet with its own currency.
- The compact wallet slot adds 44 px beside the unchanged provider meters
  (revealed bar 646 px). The floating window's declared native maximum is
  646 px, matching that widest layout; a regression test pins the two
  together.
- The resting dock has no width of its own: it takes the active bar's width
  for the same visible providers and the same wallet slot, so resting,
  revealed and pinned share one footprint and the resting meters sit exactly
  on the revealed bar's meters. The dock shows meters only, plus one wallet
  status DOT in the revealed bar's own wallet slot — a wallet is an absolute
  money balance, so it never gets a meter, a fill, or a percentage.
- The floating bar's Grok detail card shows the runtime's passive Grok Bot
  observation — the same one the main dashboard renders — so both surfaces
  describe one pool from one source. The manual accessibility read stays the
  explicit fallback and never displaces a live passive reading.

## Evidence and limits

- Verified on 2026-10-17 at the accepted candidate SHA: frontend 1116 PASS;
  Rust 843 PASS / 15 intentionally ignored live probes;
  `cargo check --locked --all-targets` PASS. Release integrity verification on
  2026-10-18: 17 PASS / 0 FAIL.
- Focused provider/component tests cover window selection, reset text,
  missing/stale/error states, explicit refresh and provider compatibility.
- Wallet candidate verified on 2026-10-21 at `faa87c9378507f3daf72f6ac818a82bf94a5beab`
  (`0e829c0` plus the 646 px maximum-width correction): frontend 1157 PASS;
  Rust 875 PASS / 16 intentionally ignored live probes; `cargo check --locked
  --all-targets` PASS; `npm run build` PASS; `npm run scan:secrets` PASS.
  Native Tauri visuals and a live authenticated OpenRouter balance remain
  Human Acceptance checks.
- Release integrity verification on 2026-10-22 for v0.8.15: 18 PASS / 0 FAIL
  (1 skipped: local worktree cleanliness). The published installer and
  signature were re-downloaded after publication and re-hashed; both match
  the run manifest, and `minisign-verify 0.2.5` with `allow_legacy=false`
  passes against the published installer.
- Release integrity verification on 2026-10-22 for v0.8.16: 18 PASS / 0 FAIL
  (1 skipped: local worktree cleanliness). The published installer and
  signature were re-downloaded after publication and re-hashed; both match
  the run manifest, and `minisign-verify 0.2.5` with `allow_legacy=false`
  passes against the published installer.
- The MiniMax `AuthStore` derives `Debug` over the raw OAuth record. This is
  a latent, non-blocking P2 finding tracked as Linear MIC-315; it was not
  fixed in v0.8.16 because the release commit is version-only and no release
  policy requires the remediation.
- Authenticated OpenDesign validation requires the user-owned Plan & balance
  page in Chrome. Agent validation is separate from Human Acceptance, which the
  operator recorded against the candidate SHA before the release.
- Native validation covered the registry, the seven-provider floating layout,
  authenticated quota values (5-hour 92% / 7-day 86%) and the stale/error path.
- Dated provider diagnostic: Codex, Antigravity, Grok and Qwen PASS;
  Z.ai FAIL (`unexpected_response`), OpenCode Go UNVERIFIED (`auth_invalid`),
  OpenDesign UNVERIFIED (manual authenticated page unavailable). These results
  require the existing operator release decision and are not waived.
- Release follows `docs/release-runbook-v0.8.0.md`, `docs/ui-direction.md` and
  the existing signed workflow and integrity verifier. Required gates remain.
- Execution/release status is tracked in Linear `MIC-311`; this file records
  technical state and does not replace that backlog.

## Phase D — MiniMax subscription quota (in progress)

- Branch `codex/minimax-quota-phase-d`, cut from the published v0.8.15
  baseline `9ac6fe7fb6fb7ccebd081d79106d8e175e9a2110` in the isolated
  worktree `C:/AI/limitscope-minimax-phase-d`.
- `minimax-token-plan` is a standard provider: official read-only endpoint
  `GET https://www.minimax.io/v1/token_plan/remains`, Bearer subscription key
  (`sk-cp-…`). The credential is resolved read-only from the MiniMax Code
  CLI's managed-login store through the account route; the CLI's OAuth
  refresh token is never used, because MiniMax rotates it on use and doing
  so would sign the user out of their own desktop app.
- Two windows: `5-hour` and `Weekly`. MiniMax reports REMAINING percent, so
  the adapter converts to used percent explicitly; `end_time` /
  `weekly_end_time` are absolute epoch milliseconds, while `remains_time` /
  `weekly_remains_time` are relative seconds and are ignored. The shared
  `general` pool is reported alone, so model-specific and media pools are
  never double-counted.
- Verified on 2026-10-22 against the user's own one-month plan: live quota
  read succeeded (both windows, correct resets); Rust 921 PASS / 16 ignored
  live probes; frontend 1160 PASS; `cargo check --locked --all-targets` PASS;
  `npm run build` PASS; `npm run scan:secrets` PASS. See
  `docs/minimax-provider.md`.
- Merged and published as part of v0.8.16; see the release record below.

## Combined v0.8.16 integration (released)

- Branch `codex/v0.8.16-combined-integration`, worktree
  `C:/AI/limitscope-v0816-integration`, cut from the published v0.8.15
  baseline `9ac6fe7fb6fb7ccebd081d79106d8e175e9a2110` (= `origin/main`).
  All four phases share that baseline as their merge base.
- Four `--no-ff` merges preserve each phase's provenance as a second parent:
  Phase A `a3bb6a6` (`codex/floating-dock-phase-a`), Phase B `50ac9bf`
  (`codex/monitor-recovery`), Phase C `0d01380`
  (`codex/deepseek-api-wallet`), Phase D `919f41e`
  (`codex/minimax-quota-phase-d`). All four commits are ancestors of the
  integration head.
- Three real conflicts, all resolved by preserving both phases' behavior:
  `src/lib/floatingQuota.ts` (import union — Phase A's `GrokBotUsage` /
  `grokBotFor` with Phase C's wallet model),
  `src/components/FloatingQuotaWindow.test.tsx` (Phase A's dock-dot
  assertions rewritten onto Phase C's shared-wallet vocabulary), and
  `src-tauri/src/runtime.rs` (import union — Phase C's `WalletAmount` with
  Phase D's `minimax_token_plan`).
- One integration defect fixed beyond the textual conflicts: the resting
  dock's wallet dot read `wallets[0]`, which is Phase A's single-wallet
  model. Phase C replaced it with ONE shared wallet slot covering every
  wallet, so the dot now uses `floatingWalletSlotState` /
  `floatingWalletSlotLabel` — the same aggregate state and spoken label the
  revealed slot uses. The two placements can no longer disagree.
- Verified on 2026-10-22 at `030dde91e10b7a22c9f57e92240520bd70297804`:
  frontend 1257 PASS (71 files); Rust 966 PASS / 18 intentionally ignored
  live probes; `cargo check --locked --all-targets` PASS (3 pre-existing
  dead-code warnings in `zcode_reset` / `zcode_plans`, untouched by every
  phase); `npm run build` PASS; `npm run scan:secrets` PASS;
  `npm run tauri build` PASS; release integrity verifier 7 PASS / 0 FAIL /
  1 skipped.
- Human Acceptance of the combined surfaces was recorded by the operator
  (APPROVED); an independent fresh-context review of the exact candidate
  returned PASS with no P0/P1 findings. The candidate was merged into `main`
  as PR #19 and published as v0.8.16 — see
  `docs/release-verification-v0.8.16.md`.
