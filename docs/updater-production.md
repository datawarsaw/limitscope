# LimitScope v0.6 Production Updater

Architecture, release engineering, and operational contracts for the
LimitScope Windows updater.

## Architecture

```
source repo: datawarsaw/limitscope            public companion: datawarsaw/limitscope-releases
------------------------------------         ------------------------------------------------
CI (.github/workflows/release.yml):          GitHub Release v<version> assets:
  npm test + cargo test --locked               1. LimitScope_<v>_x64-setup.exe
  npm run build + scan:secrets                 2. LimitScope_<v>_x64-setup.exe.sig
  tauri build (TAURI_SIGNING_PRIVATE_KEY)      3. latest.json (uploaded LAST)
  -> signed NSIS installer + .sig                               |
  -> latest.json metadata                                       | unauthenticated HTTPS
  -> atomic publish to companion repo  ------>                  v
                                             https://github.com/datawarsaw/limitscope-releases/
installed LimitScope client:                 releases/latest/download/latest.json
  Settings > Updates > Check for updates  <--                   |
  -> downloads signed NSIS installer                            |
  -> minisign signature verified against embedded public key
  -> passive NSIS upgrade (/P /R)
  -> app restarts on new version; settings/history preserved
```

## Source and Public Companion Repository Boundary

The application source and release workflow live in the source repository
`datawarsaw/limitscope`. The companion repository remains a distribution
channel for signed update artifacts only.

The installed desktop client must require zero credentials to check for and
download updates:
- Direct releases from private repositories require an authenticated GitHub PAT
  or bearer token. Embedding such credentials into client binaries would expose
  source repository read access to anyone unpacking the desktop installer.
- Therefore, release distribution uses a dedicated public companion repository:
  `datawarsaw/limitscope-releases`.
- Only unauthenticated, public release artifacts are hosted on the companion:
  1. Signed NSIS installer (`LimitScope_<version>_x64-setup.exe`)
  2. Minisign signature (`LimitScope_<version>_x64-setup.exe.sig`)
  3. Update feed metadata (`latest.json`)
- No source code, pull requests, issues, or internal commit logs exist on the
  public companion repository.

## Signing Trust Model

- Integrity verification uses Tauri's minisign updater signature.
- **Critical Distinction:** Tauri updater signing is **not** Windows
  Authenticode signing. Minisign guarantees artifact integrity and authenticity
  relative to the embedded public key, preventing tampered or unauthorized
  update binaries from running. It does not provide an Authenticode certificate
  or satisfy Windows SmartScreen reputation.
- The public key is embedded in the application binary via
  `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`).
- The private signing key and password live strictly outside the repository
  and are never committed, embedded in bundles, or published to feeds.

## Required CI Secrets

The release workflow (`.github/workflows/release.yml`) requires three
repository secrets configured under GitHub Actions:

| Secret | Scope / Purpose |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Minisign private key string (from external `limitscope.key`). Used by `tauri build` to sign the installer. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Decryption password for the minisign private key. |
| `RELEASES_PAT` | Fine-grained GitHub Personal Access Token scoped strictly to **Contents: Read and write** on `datawarsaw/limitscope-releases` only. It does not need access to the source repository. |

## Atomic Release Publication Ordering

To prevent installed clients from fetching an update feed that points to
missing or still-uploading installers, the release workflow enforces strict
atomic publication ordering:

1. **Upload installer binary:** `LimitScope_<version>_x64-setup.exe`
2. **Upload signature:** `LimitScope_<version>_x64-setup.exe.sig`
3. **Verify availability:** Check that release assets are present and intact on
   the companion repository.
4. **Publish feed LAST:** Upload `latest.json` as the final step.

Because clients poll `https://github.com/datawarsaw/limitscope-releases/releases/latest/download/latest.json`,
publishing `latest.json` last guarantees that by the time any client discovers
the new version, the matching installer and signature are already downloadable.

## Public RC Channel

Release candidates are published to the **same companion repository** as
regular releases, distinguished only by being GitHub prereleases. There is no
second feed, repository, or downgrade channel.

- **Version convention:** an RC version is `x.y.z-rc.N` (e.g. `0.8.7-rc.1`),
  tagged `v0.8.7-rc.1`, with every canonical version surface in agreement.
  The release channel is **derived from the version shape** — there is no
  operator prerelease toggle; malformed versions fail the release workflow
  and the verifier.
- **Isolation principle:** GitHub's `releases/latest` excludes prereleases
  (and `make_latest=false` is pinned as a second lock — the REST API refuses
  to mark drafts/prereleases as latest). `release.yml` creates the companion
  release with `--prerelease` from the initial creation call and asserts
  `isPrerelease == true && isDraft == false` **before** uploading any asset,
  so there is never a window in which the RC exists as a full release.
- **Stable clients never consume prereleases:** the stable endpoint
  `releases/latest/download/latest.json` keeps serving the newest stable
  release. Publication order for an RC is identical to stable (installer →
  signature → verify → `latest.json` last), and the RC's `latest.json` is an
  asset of that RC release only — there is no global RC feed pointer.
- **RC installers are manual-download artifacts:** users on stable builds are
  never offered an RC; an RC build is downloaded and installed by hand from
  the RC release page.
- **RC installations move to stable automatically:** semver orders
  `0.8.7-rc.1 < 0.8.7`, so an installed RC is offered the final release as
  soon as it ships — no uninstall/reinstall, no extra migration step.
- **First live RC:** requires an explicit release event (tag + dispatch +
  verification) — the local harness never substitutes for it. After
  publication, verify with
  `node scripts/verify-release-integrity.mjs --version <rc> --rc --stable-feed-version <stable>`
  which fails closed unless the production stable feed still serves exactly
  the supplied stable version.
- **Rollback:** delete the RC release on the companion repository (and the
  mirror tag, if any). Stable clients are unaffected by construction; RC
  installs simply stay where they are until the next offer.
- **Never remove the prerelease status** of a published RC in the GitHub UI:
  flipping an RC to a full release would make it the live latest release and
  point every stable client at it. Delete and republish instead.

## Update Feed Contract (`latest.json`)

The `latest.json` feed contains only the metadata required by Tauri:

```json
{
  "version": "0.6.0",
  "pub_date": "2026-09-29T18:00:00Z",
  "notes": "Stability improvements and new provider support.",
  "platforms": {
    "windows-x86_64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6...",
      "url": "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.6.0/LimitScope_0.6.0_x64-setup.exe"
    }
  }
}
```

Rules:
- Contains no secrets, access tokens, or private URLs.
- The `url` field is an immutable HTTPS download link referencing the specific
  tagged release.
- Only strictly newer semver versions trigger updates. Equal versions and
  downgrades are rejected.

## In-App Update UX

The updater interface lives inside the settings drawer under the **Updates**
section:

1. **Quiet Startup Check:**
   - Runs silently once on app launch without blocking startup.
   - If the app is up to date, no dialog, banner, or notification is shown.
   - If a newer version is available, it surfaces an indicator within the
     Updates block with release notes and action buttons.
   - Startup network errors or timeouts fail silently without alerting the user.

2. **Manual Check:**
   - The user can click "Check for updates" at any time.
   - Transitions through explicit states: idle -> checking -> up to date /
     update available / error.
   - Errors display concise, user-friendly guidance (e.g. network failure,
     server error) with technical detail available in the tooltip.

3. **Explicit User Installation (No Forced Updates):**
   - The app never downloads or installs updates without explicit user consent.
   - The user clicks "Update now" to initiate download and install, or "Later"
     to dismiss for the current session.
   - During installation, the signed installer runs in passive mode (`/P /R`),
     presenting a clean progress bar, and automatically restarts LimitScope.

4. **Data & Settings Continuity:**
   - The installer uses the stable application identifier
     `com.ratelimits.desktop`.
   - Settings (WebView2 profile in localStorage), quota history (SQLite in
     `%LOCALAPPDATA%\com.ratelimits.desktop`), autostart Run registry entry,
     and notification preferences remain intact across updates.

## Failure Semantics

Updater failures must never degrade normal application operation:
- **Feed unreachable / network offline:** Safe error message in manual check;
  silent ignore on startup check. Normal quota monitoring continues unaffected.
- **Malformed `latest.json`:** Rejected by the validator; safe error
  displayed.
- **Invalid signature:** Hard reject in Rust before file execution. The corrupted
  download is deleted and never executed.
- **404 / Missing asset:** Safe error message; app remains fully operational.
- **Equal or older version:** Treated as "You're up to date". Downgrades are
  never performed.

## Rollback Procedure

Automatic downgrades are explicitly out of scope and prevented by semver
guardrails. If a bad release (e.g. `v0.6.0`) is mistakenly published:

1. Immediately stop publishing `v0.6.0` as `latest.json` if in progress.
2. Build and sign a corrective release with an incremented version number
   (e.g. `v0.6.1`).
3. Follow standard atomic publication to publish `v0.6.1` assets and point
   `latest.json` to `v0.6.1`.
4. Installed clients (both on older versions and on the problematic `v0.6.0`)
   will detect `v0.6.1` as strictly newer and upgrade cleanly.

## Key Rotation & Compromise Response

Because the updater public key is compiled into installed clients:

- **Rotation Limitation:** Changing the signing key requires distributing a
  client update signed by the *currently trusted* private key that introduces
  the new trust anchor.
- **Casual Rotation Discouraged:** Do not rotate the production key without a
  managed migration release.

### If Key Compromise Occurs:

1. Cease building releases with the compromised key.
2. Revoke CI secrets `TAURI_SIGNING_PRIVATE_KEY` and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Generate a new keypair in an isolated environment using `tauri signer generate`.
4. Prepare an urgent migration release:
   - Sign the migration installer using the *old* key so existing clients can
     verify and install it.
   - In that migration release, update `tauri.conf.json` to embed the *new*
     public key.
5. In subsequent releases, sign exclusively with the new private key.
6. Remove compromised release assets and `latest.json` from the public companion
   repository immediately.
