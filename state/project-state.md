# Project State

## Identity

- Repository: `datawarsaw/limitscope`.
- Runtime: Windows Tauri desktop app, LimitScope.
- Distribution: signed releases and updater feed in `datawarsaw/limitscope-releases`.

## Current status

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
- Technical baseline for the release: `feaff647f8f6fd6c3ad0115129c4a6a614464311`.
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
- The compact wallet slot adds 44 px beside the unchanged provider meters
  (revealed bar 646 px). The resting dock stays 600 px and shows meters only.
  The floating window's declared native maximum is 646 px, matching that
  widest layout; a regression test pins the two together.

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
