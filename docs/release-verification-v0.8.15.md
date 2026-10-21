# LimitScope v0.8.15 — Release Verification Record

Recorded after publication from `main` at `c048d9ac0b1ec0c9e43ff44ec929ed08d8a9809c`,
the release commit the annotated tag `v0.8.15` points at.

## What changed

API Wallets Phase 1 shipped as a first-class part of the product surface. The
prepaid USD balance of an OpenRouter account is read from the official credits
endpoint using the existing OpenCode auth entry, and the key never leaves the
process. The balance is presented in a separate `wallets` snapshot list, not
as a provider: it has no percentage, no reset window and no period, and it
never enters quota history, prediction or notifications. The five existing
providers keep their established behavior.

The floating bar gained a compact wallet slot that adds 44 px beside the
unchanged provider meters. The resting dock stays 600 px wide and shows meters
only; the floating window's declared native maximum is 646 px, matching that
widest layout, and a regression test pins the two together.

`src-tauri/src/last_good.rs` and `src-tauri/src/runtime.rs` gained the wallet
collection path together with a fail-closed correction: a balance that cannot
be verified never falls back to an unverified last-good value, so a stale or
partial read is reported as unavailable rather than presented as a confirmed
balance. `src-tauri/src/provider_error.rs` gained the matching error
classification, and `src-tauri/src/opencode_go.rs` the shared auth entry used
by the credits request.

No OpenDesign experiment, dynamic resting-dock work, DeepSeek, JEV or other
unrelated change is part of this release. The release commit touches only the
four version surfaces plus `package-lock.json`; every product change arrived
through the reviewed PR #18 merge.

## Human Acceptance

PR #18 (`feat: OpenRouter API Wallets Phase 1`) was merged into `main` as
`feaff647f8f6fd6c3ad0115129c4a6a614464311` on 2026-10-21. The operator
recorded acceptance for the v0.8.15 stable release against that merge, and the
release commit is a version-only change on top of it, so the accepted content
is exactly what was published. No separate native UI session was run for this
release beyond the candidate validation already recorded in
`state/project-state.md`.

## Release record

- Source: `c048d9ac0b1ec0c9e43ff44ec929ed08d8a9809c` on `main`, annotated tag
  `v0.8.15` dereferencing to that commit.
- Pull request: datawarsaw/limitscope#18, merged with a merge commit.
- Post-merge `main` CI: run 39652342640, success on all three jobs
  (`Detect bundling-input changes`, `Windows build & tests`,
  `NSIS bundling verification`).
- Post-release-commit `main` CI: run 39657052378, success on all three jobs.
- Signed release run: datawarsaw/limitscope 39657152118, success, dispatched
  with `version=0.8.15` and
  `expected_head_sha=c048d9ac0b1ec0c9e43ff44ec929ed08d8a9809c`.
- Published assets in datawarsaw/limitscope-releases v0.8.15 (a normal latest
  release, not a prerelease):

| Asset | Bytes | SHA-256 |
| --- | --- | --- |
| `LimitScope_0.8.15_x64-setup.exe` | 4,200,607 | `9333d25824c18a770439857d416bb1d9b79ddaa7c6a27ed27dc4b978e4723cb7` |
| `LimitScope_0.8.15_x64-setup.exe.sig` | 444 | `5b0ac7aba5fef10e859253511943b6851a582fb5513fb6415eb22e69c32703ca` |
| `latest.json` | 806 | `69406bf0fb05e1feec63526cccb9020779bf8dfc0f2444033de05951e03bcac6` |

These bytes are identical to the run's `limitscope-signed-release` artifact,
whose `release-artifacts-manifest.txt` records the same installer and
signature hashes with `source_sha: c048d9ac0b1ec0c9e43ff44ec929ed08d8a9809c`.
The published installer and signature were downloaded again after publication
and re-hashed independently; both match the manifest byte for byte.

- `verify-release-integrity.mjs --version 0.8.15 --expected-sha c048d9ac0b1e
  --expect-tag --verify-signature --manifest <run manifest>`: PASS
  (18 passed, 0 failed, 1 skipped). This covers the version surfaces, tag
  identity, the exact asset set, the feed bytes, the feed URL, byte equality
  between the live feed and the published `latest.json`, the updater trust
  root (key id `025FF36DE3FF44EB`), and `minisign-verify 0.2.5` with
  `allow_legacy=false` (Ed25519ph, the updater's own call) against the
  published installer.
- Updater availability: `https://github.com/datawarsaw/limitscope-releases/releases/latest/download/latest.json`
  serves `version: 0.8.15` and the canonical installer URL. The v0.8.15
  release is the repository's live latest release.
- Published feed notes: `OpenRouter API Wallets support, secure balance
  refresh and floating-bar improvements.`

## Environment note

On the release machine the verifier's anonymous HTTPS fetches fail with
`unable to verify the first certificate` unless Node is started with
`--use-system-ca`; the local Node 24 build does not read the Windows root store
by default. The two `fetch failed` rows in the first verification attempt came
from that, not from the release. Every feed-facing check passes with the system
CA enabled, and an independent Windows download of the same URLs returned the
archived bytes recorded above.

## Preserved provider state

The dated provider diagnostic recorded on this tree is unchanged and is not
waived by this release: Codex, Antigravity, Grok and Qwen PASS; Z.ai FAIL
(`unexpected_response`, explicitly out of scope); OpenCode Go UNVERIFIED
(`auth_invalid`); OpenDesign UNVERIFIED for the automated diagnostic, because
that path has no access to the user-owned authenticated page. No row was
converted to PASS and none was dropped.

## Limits of this record

This record covers the publication of v0.8.15 only. A real updater E2E on a
machine running an older installed build was not part of this release, and no
installation was performed on the operator's workstation.
