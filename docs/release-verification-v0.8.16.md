# LimitScope v0.8.16 — Release Verification Record

Recorded after publication from `main` at
`c597bc1f318bc9227bc64b48087b61ecb89dd31b`, the release commit the annotated
tag `v0.8.16` points at.

## What changed

The combined v0.8.16 candidate shipped four phases behind one merge. Phase A
(`codex/floating-dock-phase-a`, `a3bb6a6`) added the passive Grok runtime
observation to the floating bar, the dynamic resting-dock width and the wallet
status dot. Phase B (`codex/monitor-recovery`, `50ac9bf`) added the native
display-topology observer, the persisted monitor identity, the pure reconciler
and the tray Locate Floating Bar action. Phase C
(`codex/deepseek-api-wallet`, `0d01380`) added the DeepSeek prepaid balance to
the shared API Wallets list with per-currency amounts. Phase D
(`codex/minimax-quota-phase-d`, `919f41e`) added the `minimax-token-plan`
provider with its 5-hour and Weekly windows.

Each phase was merged `--no-ff` inside the candidate, so every phase commit
remains an ancestor with its own provenance as a second parent. The candidate
was merged into `main` as PR #19, merge commit
`0a984dced406933007b4074fad1529c4d60dd629`.

The release commit touches only the five version-surface files
(`package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`,
`src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`) and contains no functional
source change; every product change arrived through the reviewed PR #19 merge.
The merge commit and the accepted candidate `404ad731847ab0c95c6518b8a36ce8012788d70a`
carry the identical tree `acda7a5a9f5be148cfdefc22438aef80225e308a`, so the
merge introduced no content drift.

## Human Acceptance

PR #19 (`feat: LimitScope v0.8.16 — Floating UX, Monitor Recovery, DeepSeek and
MiniMax`) was merged into `main` as
`0a984dced406933007b4074fad1529c4d60dd629`. The operator recorded
Human Acceptance (APPROVED) for the v0.8.16 stable release against the
candidate, and the release commit is a version-only change on top of that
merge, so the accepted content is exactly what was published.

An independent fresh-context review of the exact candidate (head `404ad731`,
tree `acda7a5a`, base `9ac6fe7`) returned PASS with no P0 or P1 findings; its
trusted review receipt is stored outside the repository under the MIC-314
review-receipts directory. The one non-blocking P2 observation it raised — the
MiniMax `AuthStore` deriving `Debug` over the raw OAuth record — is tracked
separately as Linear MIC-315 and was deliberately not fixed in this release,
because the release commit is version-only and no release policy requires the
remediation.

## Release record

- Source: `c597bc1f318bc9227bc64b48087b61ecb89dd31b` on `main`, annotated tag
  `v0.8.16` (tag object `e9c741e7b5822ae29ed43c95181d14cbfb385939`)
  dereferencing to that commit.
- Pull request: datawarsaw/limitscope#19, merged with a merge commit.
- Post-merge `main` CI: run 39757580360, success on all three jobs
  (`Detect bundling-input changes`, `Windows build & tests`,
  `NSIS bundling verification`).
- Post-release-commit `main` CI: run 39760178911, success on all three jobs.
- Signed release run: datawarsaw/limitscope 39761497435, success, dispatched
  with `version=0.8.16` and
  `expected_head_sha=c597bc1f318bc9227bc64b48087b61ecb89dd31b`.
- Published assets in datawarsaw/limitscope-releases v0.8.16 (a normal latest
  release, not a prerelease):

| Asset | Bytes | SHA-256 |
| --- | --- | --- |
| `LimitScope_0.8.16_x64-setup.exe` | 4,232,534 | `1beb7ca77014ffb0a59d3a709fdc8ed80e5983bb5195b6bee5f9a298fc329bb4` |
| `LimitScope_0.8.16_x64-setup.exe.sig` | 444 | `3e2910c64c77eaa3cd63dbb7f9dd22d767038e737e762bedfe88c1d0a080a68b` |
| `latest.json` | 810 | `4c1a5f86206d86b8381090a81d509922e06e84f998c86f71248a88e31f8f0631` |

These bytes are identical to the run's `limitscope-signed-release` artifact,
whose `release-artifacts-manifest.txt` records the same installer and
signature hashes with `source_sha: c597bc1f318bc9227bc64b48087b61ecb89dd31b`.
The published installer and signature were downloaded again after publication
and re-hashed independently; both match the manifest byte for byte.

- Installer metadata: FileVersion `0.8.16`, ProductVersion `0.8.16`,
  ProductName `LimitScope`.
- `verify-release-integrity.mjs --version 0.8.16 --expected-sha c597bc1f318b
  --expect-tag --verify-signature --manifest <run manifest>`: PASS
  (18 passed, 0 failed, 1 skipped). This covers the version surfaces, tag
  identity, the exact asset set, the feed bytes, the feed URL, byte equality
  between the live feed and the published `latest.json`, the updater trust
  root (key id `025FF36DE3FF44EB`), and `minisign-verify 0.2.5` with
  `allow_legacy=false` (Ed25519ph, the updater's own call) against the
  published installer.
- Updater availability: `https://github.com/datawarsaw/limitscope-releases/releases/latest/download/latest.json`
  serves `version: 0.8.16` and the canonical installer URL. The v0.8.16
  release is the repository's live latest release. The installer, signature
  and feed all answer HTTP 200 anonymously.
- Published feed notes: `Floating dock improvements, monitor recovery,
  DeepSeek wallet and MiniMax quota provider.`

## Environment note

On the release machine the verifier's anonymous HTTPS fetches fail with
`unable to verify the first certificate` unless Node is started with
`--use-system-ca`; the local Node 24 build does not read the Windows root store
by default. Every feed-facing check passes with the system CA enabled, and an
independent Windows download of the same URLs returned the archived bytes
recorded above.

## Preserved provider state

The dated provider diagnostic recorded on this tree is unchanged and is not
waived by this release: Codex, Antigravity, Grok and Qwen PASS; Z.ai FAIL
(`unexpected_response`, explicitly out of scope); OpenCode Go UNVERIFIED
(`auth_invalid`); OpenDesign UNVERIFIED for the automated diagnostic, because
that path has no access to the user-owned authenticated page. No row was
converted to PASS and none was dropped.

## Limits of this record

This record covers the publication of v0.8.16 only. A real updater E2E on a
machine running an older installed build was not part of this release, and no
installation was performed on the operator's workstation.
