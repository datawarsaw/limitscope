# Rate Limits v0.4.0 — Release Runbook

The standing, repeatable release process of [release-runbook-v0.3.1.md](release-runbook-v0.3.1.md)
applies unchanged to v0.4.0. This file records the v0.4.0 candidate's identity and scope and
adapts only the version-specific values — it is not a new release methodology.

Companions:

- Acceptance checklist: [human-acceptance-v0.4.0.md](human-acceptance-v0.4.0.md) (SHA-bound;
  `latest` or a branch name alone is never accepted)
- Acceptance helper: [../scripts/human-acceptance-v03.ps1](../scripts/human-acceptance-v03.ps1)
  (run with `-AppVersion 0.4.0`; report defaults to the git-ignored `artifacts/` directory)
- Policy background: [ci-and-release.md](ci-and-release.md) — CI never signs, publishes,
  tags, or releases; the release itself is a manual, gated process.

## Candidate identity

| Field | Value |
|---|---|
| Product | Rate Limits (`ProductName` `Rate Limits` — unchanged; QDock/LimitScope naming is v0.5 exploration only) |
| Version | `0.4.0` (package.json, package-lock.json, src-tauri/Cargo.toml, src-tauri/Cargo.lock, src-tauri/tauri.conf.json) |
| Candidate branch | `release/v0.4.0-rc` |
| Base | main `7411ea0c001633eafde0e89a0598914b719ff5a3` (v0.3.1 release closeout) |
| Source lanes | `feature/v0.4-foundation-preview` (`448f4a7`), `feature/compact-settings-drawer` (`a874f15`, ported semantically) |
| Excluded lanes | `feature/updater-foundation`, `feature/v0.5-floating-quota-window`, branding explorations, MiMo discovery doc |

## v0.4.0 feature scope (intended set)

- Production registry guardrail (exact production set pinned by test; mocks excluded by
  construction, explicit `withMockProviders()` only for dev/tests).
- Shared Rust `ProviderError` wire type (Codex, Z.ai, OpenCode Go; wire format and retry
  semantics unchanged from v0.3.1).
- Grok provider: local xAI OAuth credential read-only (Grok CLI / OpenCode / OpenCodex store
  precedence), no refresh, no inference, SuperGrok weekly credit pool on a provider-local
  cadence with last-good retention; production-registered after the live gate.
- Compact quota strip above the cards: one row per production provider, highest usable
  window, simulated providers omitted, click-to-card.
- Compact settings drawer: collapsed by default, one-row utility bar (update time,
  auto-refresh interval, Settings disclosure), expandable bottom drawer with the v0.3.1
  settings controls unchanged, keyboard/ARIA/reduced-motion preserved.
- Account-aware history (v0.3.1 contract) preserved: per-account partitioning for OpenCode Go
  and Grok; no credential material in persisted history.
- v0.4.0 version manifests aligned across all five authoritative files.

## Stage mapping (identical to the v0.3.1 runbook)

| Stage | v0.4.0 values |
|---|---|
| 1 — Candidate branch and CI green | PR from `release/v0.4.0-rc`; version already `0.4.0` before the RC build; STOP gate G0 unchanged |
| 2 — Human Acceptance | [human-acceptance-v0.4.0.md](human-acceptance-v0.4.0.md) with `-AppVersion 0.4.0`; gates G1/G2 unchanged |
| 3 — Merge | Record merge SHA; tree bridge `git rev-parse <sha>^{tree}` per gate G2b |
| 4 — Post-merge CI | `success` on the exact merge SHA (gate G3) |
| 5 — Artifact verification | `rate-limits-windows-installer` from the post-merge run; expected filename `Rate Limits_0.4.0_x64-setup.exe`; FileVersion/ProductVersion `0.4.0`; ProductName `Rate Limits`; gate G4 unchanged |
| 6 — Annotated tag | `v0.3.1`-style annotated tag `v0.4.0` on the verified merge SHA only after acceptance PASS; gate G5 unchanged |
| 7 — GitHub Release | Only after all four G6 preconditions; content-hash asset verification per lesson 9 |
| 8 — Final verification | Release record appended here when Done/Closed |

## Release record (v0.4.0 — open)

| Field | Value |
|---|---|
| Candidate branch / SHA | `release/v0.4.0-rc` / _(recorded at RC binding)_ |
| Tested commit SHA (checklist section 11) | pending Human Acceptance |
| Merge SHA | pending |
| Post-merge CI run ID / conclusion / head SHA | pending |
| Artifact ID / name | pending |
| Installer filename / size / SHA-256 | pending |
| FileVersion / ProductVersion / ProductName | `0.4.0` / `0.4.0` / `Rate Limits` |
| Tag / tag commit | pending |
| Release URL / asset digest match | pending |
| Result | Open — Human Acceptance PENDING |

## Explicitly out of scope for v0.4.0

The following lanes exist but are **not** part of this release and must not enter the
candidate branch:

- **Updater foundation** (`feature/updater-foundation`): signed in-app update flow with a
  public release feed — `READY FOR FUTURE INTEGRATION`, **NOT PART OF V0.4.0**. Its
  production activation still requires the public companion release repo, the final
  production signing key, and CI secrets; none of these exist yet. This run does not create
  the companion repo.
- **v0.5 floating quota window** (`feature/v0.5-floating-quota-window`) and all QDock /
  LimitScope branding exploration: v0.5 exploration only; the v0.4.0 product name stays
  `Rate Limits`.

## Provenance note

The RC consolidation was performed on `release/v0.4.0-rc` from the released v0.3.1 main
baseline; the deterministic validation record (test counts, bundle audit, installer hash)
lives in `state/project-state.md` and the acceptance record is bound to the exact tested
product SHA per the v0.3.1 pattern (TESTED PRODUCT SHA vs RC RECORD COMMIT distinction).
