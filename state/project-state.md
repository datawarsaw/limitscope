# Project State

## Identity

- Repository: `datawarsaw/limitscope`.
- Runtime: Windows Tauri desktop app, LimitScope.
- Distribution: signed releases and updater feed in `datawarsaw/limitscope-releases`.

## Current status

- v0.8.14 released: OpenDesign is a standard provider on `main`. Release
  commit `d640f052762e409461746b6ee7e2538c641a3b77`, annotated tag `v0.8.14`,
  published in `datawarsaw/limitscope-releases`. See
  `docs/release-verification-v0.8.14.md`.
- Technical baseline for the release: `67c2df3e6e37d48e4c329a06c9465116d5c0d460`.
- Integrated branch: `codex/v0.8.14-opendesign`, merged as PR #16.
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

## Evidence and limits

- Verified on 2026-10-17 at the accepted candidate SHA: frontend 1116 PASS;
  Rust 843 PASS / 15 intentionally ignored live probes;
  `cargo check --locked --all-targets` PASS. Release integrity verification on
  2026-10-18: 17 PASS / 0 FAIL.
- Focused provider/component tests cover window selection, reset text,
  missing/stale/error states, explicit refresh and provider compatibility.
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
