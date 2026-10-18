# LimitScope v0.8.14 — Release Verification Record

Recorded after publication from `main` at `d640f052762e409461746b6ee7e2538c641a3b77`,
the release commit the annotated tag `v0.8.14` points at.

## What changed

OpenDesign moved from a supplementary dashboard card with its own Tauri
command to the standard provider identity `opendesign`, registered after
`qwen-token-plan`. The dashboard rail, quota details, visibility/order
preferences and the floating bar now read the shared runtime snapshot, so the
provider has one presentation path instead of two. See
`docs/opendesign-provider.md` for the provider contract.

Both source allowances stay independent. `5-hour limit` and `7-day limit` map to
separate `LimitWindow` entries; the collector's remaining share is expressed in
the shared used-percent field as `usedPercent = 100 - remainingPercent`. No
average, sum or substitution exists in the path, and the floating bar
designates the 7-day window as its long-term quota.

Source reset countdowns are retained verbatim in the additive optional
`LimitWindow.resetText` field and rendered with an `At observation:` prefix.
OpenDesign never sets `resetAt`, so a cached relative countdown cannot present
itself as a running clock. Other providers keep their absolute-reset behavior.

`src-tauri/src/opendesign.rs` gained one correctness fix: a scan error is now
distinguished from a missing source, so an accessibility failure is no longer
reported as "page absent". A complete, unambiguous pair is still the only
accepted reading, and the last complete pair keeps its original observation
timestamp through a failed read. Missing or duplicate groups, unreadable trees
and timeouts never become 0%.

## Refresh boundary

Automatic refresh was evaluated and remains unsupported, and the UI says so.
The collector depends on an authenticated Plan & balance page that only the
user controls, and a runtime timer must not drive browser accessibility work.
Automatic provider cycles project the last manual result from process memory
only. Both main and floating details expose a manual Refresh. No navigation,
focus change, reload, typing, credential read, private API request or
background browser interaction was added; the existing read-only UIA reader,
watchdog and single-flight protection are reused unchanged.

## Human Acceptance

The operator accepted the candidate UI at `10d8cd16df73912c027a43a239a9a3f5d6073d8e`,
the tip of `codex/v0.8.14-opendesign`. Acceptance was recorded against that
exact SHA, and the merged commit `d640f052` carries identical source content
(`git diff --stat 10d8cd16 origin/main` is empty), so the acceptance transfer
holds without a further UI change. `docs/ui-direction.md` gates push, merge,
tag and publication on that acceptance; it was obtained before any of them.

Live validation ran against the authenticated OpenDesign page in Chrome. Each
row is the running desktop app's own UI Automation tree, so the recorded values
are the ones a user sees.

| Window | Chrome page | LimitScope |
| --- | --- | --- |
| 5-hour | 93% left, `Resets in 2 hr 33 min` | 92% remaining, `At observation: Resets in 2 hr 30 min` |
| 7-day | 86% left, `Resets in 6 days 6 hr` | 86% remaining, `At observation: Resets in 6 days 6 hr` |

The one-point 5-hour difference matches the collector's raw 92.708% value at
the earlier observation instant; the 7-day window agrees exactly. Dashboard and
floating bar both selected 7-day as the primary quota, the collapsed floating
item showed `OD 86%`, and the `Cached` label was consistent with the snapshot's
own `sourceUpdatedAt`.

Stale and error behaviour was exercised live with the OpenDesign page closed:
the cached pair and its original observation timestamp stayed visible, the
provider showed an explicit failure, the floating item read `Refresh failed`
with `error, stale`, and no value was reported as 0%.

## Automated validation at the candidate SHA

- `npm test`: 1116 passed.
- `cargo test --locked`: 843 passed, 15 intentionally ignored live probes.
- `cargo check --locked --all-targets`: passed.
- `npm run build`, `npm run scan:secrets`: passed.
- Focused coverage in `src/components/OpenDesignProvider.test.tsx`:
  provider registration, window mapping, floating-bar long-term selection,
  relative reset display, stale/error states, existing-provider compatibility.

## Release record

- Source: `d640f052762e409461746b6ee7e2538c641a3b77` on `main`, annotated tag
  `v0.8.14` dereferencing to that commit.
- Pull request: datawarsaw/limitscope#16, merged with a merge commit after CI
  run 39081068258 succeeded on all three jobs (`Detect bundling-input changes`,
  `Windows build & tests`, `NSIS bundling verification`).
- Post-merge `main` CI: run 39082265953, success.
- Signed release run: datawarsaw/limitscope 39082296389, success, dispatched with
  `version=0.8.14` and `expected_head_sha=d640f052762e409461746b6ee7e2538c641a3b77`.
- Published assets in datawarsaw/limitscope-releases v0.8.14 (a normal latest
  release, not a prerelease):

| Asset | Bytes | SHA-256 |
| --- | --- | --- |
| `LimitScope_0.8.14_x64-setup.exe` | 4,170,778 | `f29287aecbc7f8ed19d9c2da605c599ce8de9e461034f1d6fcdb9f7d66fdfb53` |
| `LimitScope_0.8.14_x64-setup.exe.sig` | 444 | `a41cb5af55aaf69886f7afc26f019cf2e9a650f7b753333af44308a32d133049` |
| `latest.json` | 936 | `974a361888b86596112bab6723a2285a1455ffd9b652bd0fadbe55fa25f04944` |

These bytes are identical to the run's `limitscope-signed-release` artifact,
whose `release-artifacts-manifest.txt` records the same installer and signature
hashes with `source_sha: d640f052762e409461746b6ee7e2538c641a3b77`.

- `verify-release-integrity.mjs --version 0.8.14 --expected-sha d640f052762e
  --expect-tag --verify-signature`: PASS (17 passed, 0 failed, 1 skipped). This
  covers the version surfaces, tag identity, the exact asset set, the feed
  bytes, the feed URL, byte equality between the live feed and the published
  `latest.json`, and `minisign-verify 0.2.5` with `allow_legacy=false`
  (Ed25519ph, the updater's own call) against the published installer.
- Updater availability: `https://github.com/datawarsaw/limitscope-releases/releases/latest/download/latest.json`
  serves `version: 0.8.14` and the canonical installer URL. The release is the
  repository's live latest release.

## Environment note

On the release machine the verifier's anonymous HTTPS fetches fail with
`unable to verify the first certificate` unless Node is started with
`--use-system-ca`; the local Node 24 build does not read the Windows root store
by default. The two initial `fetch failed` rows in the first verification
attempt came from that, not from the release. Every feed-facing check passes
with the system CA enabled, and an independent `curl` download of the same URLs
returned the archived bytes.

## Preserved provider state

The dated provider diagnostic recorded on this tree is unchanged and is
reproduced here as-is rather than waived: Codex, Antigravity, Grok and Qwen
PASS; Z.ai FAIL (`unexpected_response`, explicitly out of scope for this
release); OpenCode Go UNVERIFIED (`auth_invalid`); OpenDesign UNVERIFIED for
the automated diagnostic, because that path has no access to the user-owned
authenticated page — the manual UIA reading is the only sanctioned source, and
it is verified above. No row was converted to PASS and none was dropped.

## Limits of this record

The runtime verdict for the OpenDesign feature stays CONDITIONAL: the read is
reliable only while Chrome runs with an authenticated OpenDesign Plan & balance
page open, and refresh is manual by design. That is a product condition of the
UIA approach, and the panel and floating detail both state it plainly.
