# CI and Release

This document describes the Windows CI pipeline (`.github/workflows/ci.yml`), the
local equivalents of each CI gate, where build artifacts land, which tests are
intentionally excluded, and the known Windows/Tauri caveats.

CI proves on every push to `main` and on every pull request that the application
builds and tests cleanly on a fresh `windows-latest` GitHub Actions runner.
CI does **not** sign binaries, publish releases, or push tags.

## CI stages

### Always-on gates (`windows` job)

| # | Gate | Command |
|---|------|---------|
| 1 | Install frontend dependencies (lockfile-exact) | `npm ci` |
| 2 | Rust tests (offline; live tests `#[ignore]`d) | `cargo test --locked` (in `src-tauri/`) |
| 3 | Frontend unit tests | `npm test` (`vitest run`) |
| 4 | Frontend build (type-check + bundle) | `npm run build` (`tsc && vite build`) |
| 5 | Secret scan of build outputs | `npm run scan:secrets` |

These run on every pull request and every push to `main`, produced no signed
or published artifacts, and are the only checks that should ever be marked as
required in branch protection.

### Bundling verification gate

The full `npm run tauri build` NSIS bundle used to run on every CI execution.
It is the slowest stage (a release-profile LTO Rust build plus the NSIS
package) and its only product is an **unsigned** installer artifact that
ordinary CI does not consume -- releases are built, signed and published by
`.github/workflows/release.yml`.

Bundling now runs in the separate `bundling-verification` job, only when a
bundling-specific input changed:

| Gate | Command |
|------|---------|
| Full Tauri release build + NSIS bundling | `./node_modules/.bin/tauri build -- --locked` |
| Upload installer artifact | `src-tauri/target/release/bundle/nsis/*-setup.exe` (`if-no-files-found: error`) |

Two jobs implement this:

1. `bundling-inputs` (**Linux**) resolves the paths changed by the run and
   prints `true`/`false` from `scripts/ci-bundling-gate.mjs`. It handles both
   events: for a pull request it diffs against the base branch tip
   (`github.base_ref`); for a push it diffs against the previous tip of the ref
   (`github.event.before`). When no base can be resolved (unborn ref, force
   push, fetch failure) it prints a warning and emits `true`, so bundling
   verification fails closed rather than silently skipping.
2. `bundling-verification` (**Windows**) carries the Tauri
   bundling step and the installer upload. Its job-level
   `if: needs.bundling-inputs.outputs.run_bundling == 'true'` means the job is
   **skipped**, not left pending, when nothing relevant changed.

Bundling inputs (see `BUNDLING_INPUTS` / `BUNDLING_INPUT_PREFIXES` in
`scripts/ci-bundling-gate.mjs`):

| Input | Why it selects the bundle |
|-------|---------------------------|
| `src-tauri/tauri.conf.json` | `bundle` config: `targets`, `icon`, NSIS options, product/binary identity |
| `src-tauri/installer-hooks.nsh` | `installerHooks` NSIS script compiled into the installer |
| `src-tauri/installer/English-custom.nsh` | `customLanguageFiles` NSIS script |
| `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/build.rs` | Rust crate inputs for the bundled binary |
| `src-tauri/src/**` | Rust sources compiled into the bundled binary |
| `src-tauri/icons/**` | bundler icons |
| `src-tauri/capabilities/**` | Tauri capability config generated into the bundle |
| `package.json`, `package-lock.json`, `.github/workflows/ci.yml` | build toolchain and the workflow that defines the bundling path |

The Tauri CLI downloads NSIS at build time, so the repository vendors no NSIS
sources; nothing else in the tree is a bundle input. `scripts/ci-bundling-gate.test.mjs`
asserts that every configured input still exists, so a rename that is not
mirrored in the gate fails the suite instead of silently skipping bundling.

### Required checks and branch protection

The workflow is **not** skipped wholesale by `paths:`/`paths-ignore:` filtering,
because a workflow skipped that way leaves its required checks pending and can
block a merge. Gating is per job instead: `windows` always runs, and
`bundling-verification` is a job that is genuinely skipped.

When branch protection is configured for this repository, mark only the
always-on **`windows`** check (name `Windows build & tests`) as required.
Do not mark `Detect bundling-input changes` or `NSIS bundling verification` as
required: the latter is skipped when nothing relevant changed, and a skipped
job never reports a pending status. Treating a conditional job as a required
check is the failure mode this design avoids.

Runner: `windows-latest` (both Windows jobs), `ubuntu-latest`
(`bundling-inputs`; the changed-path computation is shell/node only).
Toolchains: Node 24 (`actions/setup-node`), stable Rust MSVC
(`dtolnay/rust-toolchain@stable`) on the Windows jobs. Caching: npm cache via
`setup-node`, cargo registry + `src-tauri/target` via `Swatinem/rust-cache@v2`.
The `windows` job has a 45-minute timeout, `bundling-inputs` 10 minutes, and
`bundling-verification` 45 minutes. The workflow cancels superseded runs on the
same ref.

## Local equivalent commands

Run the same sequence on a Windows machine with Node 24+ and stable Rust (MSVC
target) installed:

```powershell
npm ci
cargo test --locked        # from src-tauri/
npm test
npm run build
npm run scan:secrets
./node_modules/.bin/tauri build -- --locked   # only when a bundling input changed
```

The installer appears under `src-tauri/target/release/bundle/nsis/`.
The Tauri build internally re-runs `npm run build` as Tauri's
`beforeBuildCommand`, so the ordinary `npm run build` gate is a fast-failing
type-check and the bundling job repeats it by design. The Tauri CLI is invoked
directly (`./node_modules/.bin/tauri build -- --locked`) because `npm run`
swallows `--` separators, which would drop the `--` that hands `--locked` to
Cargo.

To see the bundling decision locally, pipe changed paths into the gate:

```powershell
"src-tauri/tauri.conf.json" | node scripts/ci-bundling-gate.mjs   # true
"src/App.tsx"             | node scripts/ci-bundling-gate.mjs   # false
```

## Artifact location

When the bundling verification runs and succeeds, the NSIS installer is
uploaded as the workflow artifact **`rate-limits-windows-installer`**
(Actions -> run -> Artifacts). The file is named
`LimitScope_<version>_x64-setup.exe` (the product/binary name; the internal
crate and identifier keep the `rate-limits` / `com.ratelimits.desktop`
lineage). Artifacts are retained with the
repository default retention policy and are unsigned.

## Tests intentionally excluded from CI

CI runs only offline tests. The following Rust tests are marked
`#[ignore]` and therefore skipped by plain `cargo test` (use
`cargo test -- --ignored` locally if you want to run one deliberately):

| Test | Reason it cannot run in CI |
|------|---------------------------|
| `codex::tests::live_fetch_returns_windows` | requires a real local Codex login and network access |
| `claude::tests::live_fetch_returns_windows` | requires Claude Code credentials on the machine |
| `grok::tests::live_fetch_returns_windows` | requires an unexpired local xAI OAuth credential |
| `zai::tests::live_fetch_returns_windows` | requires a real local Z.ai plan key |
| `opencode_go::tests::live_fetch_returns_windows` | requires a real local OpenCode Go key |
| `antigravity::tests::live_fetch_returns_windows` | reads the real local Antigravity account cache |
| `qwen_token_plan::tests::live_token_plan_returns_the_monthly_window` | requires the official Bailian CLI and an international Singapore console login |

CI therefore needs **no provider credentials of any kind** (Codex, Z.ai,
OpenCode, Antigravity, Claude, Grok, Qwen) and no repository secrets. All frontend
vitest suites are offline unit tests and run everywhere.

## Known Windows/Tauri caveats

- **No `rust-toolchain.toml`.** The repo does not pin a Rust version; CI uses
  the `stable` channel, which floats. Locally developed on 1.98.1. If a future
  stable breaks the build, pin one with a `rust-toolchain.toml` (repo-wide) and
  `dtolnay/rust-toolchain@<version>` in the workflow.
- **Node version is assumed, not declared.** `package.json` has no `engines`
  field and there is no `.nvmrc`. CI pins Node 24 (matching the development
  machine, v24.19.0); bump both places together when upgrading Node.
- **NSIS is downloaded at build time** by the Tauri CLI during bundling; no
  preinstalled NSIS is required on the runner. First uncached runs are slow
  (cargo cold build of the Tauri stack + NSIS download), typically well over
  10 minutes; `rust-cache` keeps later runs faster. This cost is now paid only
  on bundling runs.
- **WebView2**: preinstalled on GitHub `windows-latest` runners, so the app
  builds without extra setup. End-user machines need the WebView2 runtime,
  which the NSIS installer bootstraps by default.
- **The installer is unsigned.** Expect SmartScreen/Defender warnings when
  running the artifact locally. Signing is deliberately out of scope and would
  require repository secrets.
- **`dist/` and `src-tauri/gen/` are gitignored** and fully regenerated by the
  build; nothing in the build depends on stale local output.
- **`scripts/*.ps1` are manual workstation helpers** (screenshot/tray
  automation against a running app). CI never invokes them.

## Repository boundaries

- `datawarsaw/limitscope` is the canonical source repository: active product
  development and release source happen here.
- `datawarsaw/limitscope-releases` is the public updater/distribution
  companion: it hosts published installers, signatures, and `latest.json`
  only; no product source lives there.
- `datawarsaw/rate-limits` is legacy/historical and must not be used for new
  product mutations.

## Release policy

CI produces unsigned installer artifacts only. Releases remain a manual
process -- the last executed release runbook is
`docs/release-runbook-v0.6.0.md`; the active v0.8 process is
`docs/release-runbook-v0.8.0.md` with the updater architecture in
`docs/updater-production.md`; each delivery under that runbook records the
dated per-provider live verification gate (PASS / FAIL / UNVERIFIED) defined
in its gate section. Release closeout is verified GitHub release provenance
(release commit, tag, workflow run, published companion release -- see the
v0.8 runbook's Phase 5); this repository keeps no separate post-release state
ledger. Do not add release publishing, tag pushing, or signing to this
workflow without explicitly revisiting that policy and the required secrets.


## Release trust boundary (0.8.24 local candidate)

The manually dispatched release uses separate GitHub-hosted Windows runners.
`validation` receives no signing key or companion publication token and uploads
no artifacts or caches; its success is required before `windows` can start.
The release runner checks out the workflow SHA, verifies the operator SHA and
version tag, installs the locked dependencies with npm 12 strict script approval,
and builds with Cargo `--locked`. Both runners pin Node 24.19.0 and npm 12.0.2.
CI uses the same npm policy and locked Cargo resolution for the Tauri bundle.

The build runs without signing credentials. Only the signing step receives the
key; the companion token is limited to the publication step. The exact installer
and updater signature must verify cryptographically against the embedded trust
root before publication. The publication step also checks the artifact hashes,
feed URL and signature, then stages and re-downloads all three assets from a
private draft before activation. Stable and RC behavior remains unchanged.

This boundary assumes GitHub-hosted runner isolation and trusts the source,
runner image, actions, registries, approved install scripts and build tools.
A compromised build process on the signing runner could persist or tamper with
subsequent steps, including access to secrets; separation of the validation job
only contains the test process and does not remediate tinypool or accept any
remaining vulnerability. Release remains HOLD, with Human Acceptance unchanged.
