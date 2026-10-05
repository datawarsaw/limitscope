# Release Notes — Rate Limits v0.2.1

**Date:** 2026-09-27
**Artifact:** `src-tauri/target/release/bundle/nsis/Rate Limits_0.2.1_x64-setup.exe`

v0.2.1 is a hardening release on top of v0.2.0: no new providers, no UI
changes. It makes provider failures fully structured and moves every retry
decision onto that structure.

## What's in v0.2.1

### Structured provider-error contract

- The live production backends (Codex, Z.ai, OpenCode Go) report failures as
  a structured payload: a stable `code`, a display-safe `message`, and —
  where the failure class supports it — `httpStatus` (the offending HTTP
  status of a non-success response) and `transient` (the backend's retry
  verdict; see the `*Error` structs in codex.rs / zai.rs / opencode_go.rs).
  Messages never embed tokens or file contents.
- Retry classification reads only that structured metadata. Provider
  messages are never regex-parsed, so their wording can change freely.
- HTTP 429 and 5xx responses are classified transient and get the single
  bounded retry; transport failures (`network`) likewise. Auth, credential,
  schema, and other deterministic failures carry an explicit
  `transient: false` verdict and are never retried, whatever their message
  says.
- The frontend adapters share one `CommandError` type
  (`src/types.ts`) instead of per-provider local error shapes.

### Antigravity cache conservatism

- `schema_changed` is now reported as `unexpected_response`, so the failure
  taxonomy is uniform across backends and no frontend branch keys on a
  cache-specific name.
- The frontend adapter coerces any absent/unknown freshness verdict to
  "stale": an indeterminate snapshot can never read as fresh, even if the
  Rust contract widens later. A stale snapshot keeps its windows visible but
  surfaces as status "stale", so cached quota data can never read as live.

### CI

- New Windows GitHub Actions workflow (`.github/workflows/ci.yml`): Rust
  tests (`cargo test --locked`, live tests `#[ignore]`d), frontend tests
  (`npm test`), frontend build (`npm run build`), and the full Tauri build
  with the NSIS installer uploaded as an artifact — on every push to main
  and every pull request.
- `docs/ci-and-release.md` documents the pipeline and the release flow.

### Housekeeping

- Version bump 0.2.0 → 0.2.1 across all five manifests (package.json,
  package-lock.json, src-tauri/Cargo.toml, src-tauri/Cargo.lock,
  src-tauri/tauri.conf.json); the installer product version and tray
  user-agent derive from these.
