# Release Integrity Verifier

`scripts/verify-release-integrity.mjs` is a read-only verifier that automates the
release-integrity checks manually proven during the v0.8.4 and v0.8.5 deliveries
(process in `docs/release-runbook-v0.8.0.md` and `docs/updater-production.md`).
It verifies
the source tree, the published companion release, and the updater feed in one
invocation. It never tags, publishes, signs, or writes inside the repository.

## Usage

```bash
# Full verification of a released version (source at the tag + live companion release)
node scripts/verify-release-integrity.mjs --version 0.8.5

# Add cryptographic minisign verification of the installer (cargo-based helper)
node scripts/verify-release-integrity.mjs --version 0.8.5 --verify-signature

# Verify with locally staged evidence instead of a fresh installer download
node scripts/verify-release-integrity.mjs --version 0.8.5 --verify-signature \
  --installer .release-verify-v085/installer.exe --sig .release-verify-v085/installer.exe.sig

# Pre-dispatch candidate check (clean tree, version surfaces, no tag required yet)
node scripts/verify-release-integrity.mjs --version <next> --source-only

# Bind the tag to an exact expected product SHA (also catches a moved tag)
node scripts/verify-release-integrity.mjs --version 0.8.5 --expected-sha <full-sha> --expect-tag

# Offline feed verification against a local latest.json (no network)
node scripts/verify-release-integrity.mjs --version 0.8.5 \
  --metadata fixtures/release-integrity/latest-good.json

# Release candidate (opt-in): verify the RC prerelease release AND that the
# production stable feed still serves exactly the stable version
node scripts/verify-release-integrity.mjs --version 0.8.7-rc.1 --rc \
  --stable-feed-version 0.8.6
```

Exit codes: `0` all enabled checks passed (skips are printed with reasons),
`1` at least one check failed (fail closed), `2` could not verify (missing
tool or unreadable input). `--json` emits the structured report instead of the
human-readable lines.

## Checks

| ID | Check |
| --- | --- |
| S1 | `git worktree cleanliness` — enforced in candidate mode; skip in tag/SHA mode |
| S2 | expected source SHA exists |
| S3 | five version surfaces agree — `package.json`, `package-lock.json` (root and `packages[""]`), `tauri.conf.json`, `Cargo.toml`, `Cargo.lock` (`rate-limits` crate) — plus `productName`/`mainBinaryName` |
| S4 | annotated tag `v<version>` exists (when required) and dereferences to the product SHA |
| S5 | updater endpoint is exactly the production feed; identifier `com.ratelimits.desktop`; embedded trust-root key id `025FF36DE3FF44EB` |
| S6 | no dev URL outside `build.devUrl`; CSP uses the production IPC custom protocol |
| S7 | the five canonical providers (codex, opencode-go, zai, antigravity, grok) are registered |
| S8 | no experimental Claude registration (unless `--allow-experimental-claude`) |
| R1 | companion release exists, not draft/prerelease (stable mode) — **with `--rc`: must instead be a published (non-draft) prerelease** |
| R2 | exactly installer + `.sig` + `latest.json` assets |
| R3/R4 | feed version and artifact URL match the release and tag |
| R5 | feed signature equals the published `.sig` bytes |
| R6 | no private/local URLs or credential shapes in the feed (paths only, values never printed) |
| R7 | a successful `release.yml` run exists at the product SHA (`gh`) |
| R8 | the live `releases/latest/download/latest.json` endpoint serves this release's bytes (stable mode) — **with `--rc`: the stable feed must still resolve to (redirect to, and serve the exact bytes of) the `--stable-feed-version` release, never the RC** |
| G1 | `.sig` parses as a modern minisign signature (algorithm `ED`) keyed by the trust root |
| G3 | real minisign verification of the installer (`--verify-signature`) |
| M1 | optional `--manifest` cross-check of `release-artifacts-manifest.txt` |

## RC mode

`--rc` is opt-in and strict: the version must match `x.y.z-rc.N` exactly,
`--stable-feed-version <x.y.z>` is mandatory, and the release must be a
published prerelease (never a draft, never a full release). In addition to
the stable checks, RC mode proves stable-feed isolation: the production
`releases/latest` endpoint must redirect to the stable release named by
`--stable-feed-version`, serve bytes identical to that release's
`latest.json`, and reference nothing from the RC release. The channel is
derived from the version shape (no operator toggle), so malformed versions
fail closed in both modes. See docs/updater-production.md "Public RC channel".

## Signature verification method

Tauri's `.sig` is the base64 armor of a minisign signature; the signature is
Ed25519ph (`ED`), which Node's crypto cannot verify. G3 therefore compiles a
tiny ephemeral helper under the OS temp directory (never inside the repository)
using `minisign-verify = "=0.2.5"` with `allow_legacy=false` — the exact method
proven in the v0.8.4 delivery — and runs it against the decoded trust root from
`tauri.conf.json` at the verified tree. G3 never fabricates a verdict: without
cargo or the installer it reports SKIP, and a helper environment error is a
SKIP, not a pass.

## Guarantees and boundaries

- Read-only: `git`/`gh` read commands and anonymous HTTPS GETs only; temp files
  live outside the repository and are removed after the run.
- No secret output, no credential discovery, no private-key dependency.
- The pinned trust-root key id makes an unmanaged key change fail loudly;
  rotation is a managed procedure (`docs/updater-production.md`).
- Unit tests (`scripts/verify-release-integrity.test.mjs`, run by `npm test`)
  cover the check logic offline against `fixtures/release-integrity/`
  (structurally valid, deliberately non-cryptographic minisign fixture
  material): correct metadata, wrong version, lockfile version drift (root and
  `packages[""]`), wrong tag target, mismatched signature, private URL leak,
  wrong artifact URL, missing asset, CRLF `.sig` handling (autocrlf checkouts).
- `release.yml`, the signing trust root, the publisher, the public companion
  repo, and the updater contract are untouched by this tool.
