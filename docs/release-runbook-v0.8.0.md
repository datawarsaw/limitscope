# LimitScope v0.8.0 — Release Runbook (pre-RC contract)

- **Status:** PREPARED, NOT EXECUTED beyond the pre-RC phase. This runbook is
  the authoritative delivery procedure for the v0.8.0 RC and release. Only
  the pre-RC hardening (Phase 0) has been performed. Phases 1–5 are the
  required future steps to cut and ship `release/v0.8.0-rc`.
- **Provenance:** green integration candidate `integration/v0.8-product-rehearsal`
  @ `2720efd2bde7044537a68c88718d40988d23a8ef`; the PRE-RC hardening commit
  (`PRE_RC_SHA`, recorded in the delivery report) sits directly above it.
- **Contracts:** `docs/v0.8.0-release-candidate.md` (acceptance-target rules),
  `docs/v0.8-integration-rehearsal-acceptance-matrix.md` (sole acceptance
  source, planning worktree), `docs/a11y-contract-v0.8.md`,
  `docs/updater-production.md` (distribution architecture, secrets, rollback).

## Phase 0 — Pre-RC hardening (DONE, this branch)

Release tooling and documentation only; no product runtime change, no
version bump, no RC branch/tag/merge/release/publication:

- `.github/workflows/release.yml` hardened and pinned by
  `src/releaseWorkflowGuard.test.ts` (tag/ref/HEAD identity, four-way
  version-surface agreement, exact artifact selection, SHA-256/size
  manifest, `latest.json` strictly last, updater key/endpoint preserved,
  no publishing RC mode).
- Active release docs created: release notes, RC contract, this runbook.
- Stale active references fixed (Cargo.toml description, brand guard `.toml`
  scan, `docs/ci-and-release.md` artifact name + runbook pointers).

## Phase 1 — Cut the RC branch

1. From a clean worktree on the green integration head:
   `git worktree add C:\AI\Token_Monitor_v080_rc release/v0.8.0-rc`
   (branch created from the exact integration head; `2720efd…` + `PRE_RC_SHA`
   must be ancestors).
2. Bump all four version surfaces to `0.8.0` (and only them):
   `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, then
   run `cargo check` once (in `src-tauri/`) so `src-tauri/Cargo.lock`
   records `0.8.0` for the `rate-limits` crate; `git diff
   src-tauri/Cargo.lock` must show only that version change (no dependency
   drift, `--locked` builds keep working). `productName`/`mainBinaryName`
   stay `LimitScope`; crate name and identifier unchanged.
3. Full gate on the RC branch: `npm test`, `cargo test --locked`,
   `cargo check --locked --all-targets`, `npm run build`,
   `npm run scan:secrets`, `git diff --check`, native acceptance harness,
   visual capture set, and the deterministic matrix rows re-run from the
   acceptance matrix. Record counts in the RC record.

## Phase 2 — Signed installer and controlled RC updater test (local harness)

1. Build the signed RC installer:
   `npm run tauri build` with the signing key material from the operator
   key store (never from the repository). Record size + SHA-256.
2. **Controlled updater test via the local harness** — there is no publishing
   RC mode in the release workflow, by design:
   - `node scripts/update-harness.mjs overlay --version 0.7.0` → build the
     throwaway 0.7.0 client pinned to the local feed:
     `npm run tauri build -- --debug --config "state\update-harness\tauri.overlay.json"`.
   - Stage the RC installer as the advertised update:
     `node scripts/update-harness.mjs prepare --version 0.8.0 --notes "<RC notes>"`.
   - `node scripts/update-harness.mjs serve` and install the 0.7.0 harness
     build; use Settings → Updates → Check for updates → Update now; the app
     must verify the minisign signature, passive-install, and restart
     reporting `0.8.0` with settings/history intact.
   - The harness never contacts the production endpoint
     (`datawarsaw/limitscope-releases`) and never publishes anything.
3. Optional real-install rehearsal per the v0.7 pattern (passive `/P /R`
   upgrade of the installed production 0.7.0 lineage on the validation
   machine) — recorded as evidence, still not a publication.

## Phase 3 — Human Acceptance against the exact RC SHA

1. Record every HUMAN_REQUIRED row of `docs/a11y-contract-v0.8.md` §2 (DPI
   matrix 100/125/150/175/200% × 1080p/768p, 200% text, tray keyboard access
   with main hidden, WebView2 focus/Escape journeys, pointer-into-tooltip
   persistence, tray-Hide native flow) plus the matrix §4–§5 rows.
2. Human Acceptance is decided against the **exact current head SHA of
   `release/v0.8.0-rc`** — clean tree, all four surfaces at `0.8.0`.
   Acceptance binds to that SHA only; it does not transfer automatically to
   any later SHA (see the RC contract §2 for the re-confirmation rule).
3. Fill the matrix §7 decision record: reviewer target == acceptance target,
   decision, owner, date. Every S0/S1 row PASS; S2 rows PASS or named
   exception; no unexplained `NOT RUN`.

## Live provider verification gate (per-provider, dated)

Before a release is delivered under this runbook, live provider verification
is recorded for every production provider with a date, a result, and
evidence. The gate is a delivery precondition for Phase 4: run it on the
exact RC tree before Human Acceptance is recorded, and re-run it before
delivery if the tree changed afterwards. It reuses the existing sanctioned
mechanism — no new harness:

```powershell
powershell -NoProfile -File scripts/provider-quota-diagnostic.ps1
```

One invocation reports all five production providers in canonical registry
order (`openai-codex`, `zai`, `opencode-go`, `antigravity`, `grok`) through
the real production fetch paths, redacted by construction and secret-scanned
(schema and boundaries: `docs/provider-diagnostic-v0.6.md`). The filled table
is part of the release's own evidence: record it with the RC record and carry
it into the delivery record (the existing `Provider diagnostic` row in
`state/project-state.md`). Template:

| Provider | Checked at | Result | Evidence | Note |
| --- | --- | --- | --- | --- |
| OpenAI / Codex (`openai-codex`) | | | | |
| Z.ai (`zai`) | | | | |
| OpenCode Go (`opencode-go`) | | | | |
| Google Antigravity (`antigravity`) | | | | |
| Grok (xAI) (`grok`) | | | | |

### Result semantics

Exactly three states; no fourth state exists.

- **PASS** — the provider's diagnostic row reported health `live` (the
  expected normalized quota windows / provider response was observed) and the
  run's secret scan passed. Only a `live` row is PASS.
- **FAIL** — the verification ran and produced a release-relevant failure:
  health `error` with its normalized reason code, or any other observed
  condition judged release-relevant (for example an unexpected response
  shape). A failed redaction/secret scan fails the whole gate.
- **UNVERIFIED** — the live check could not be performed for that provider:
  required credentials, account access, network access, or provider
  availability were absent (diagnostic health `unavailable`, e.g.
  `credential_missing`), or the diagnostic was not run at all. UNVERIFIED is
  not PASS.

The diagnostic's remaining health categories (`stale`, `unknown`, `cooldown`)
are not live results and can never be recorded as PASS; record such a row as
FAIL or UNVERIFIED with the reason, per the operator's release-relevance
judgment.

### Non-waivable rule

- A FAIL remains FAIL until a later dated verification passes. The later pass
  is appended with its own date; the FAIL row is never overwritten, dropped,
  or relabeled — there is no "waived" state and the word "waived" must not
  appear in place of a result.
- UNVERIFIED stays visible in the release evidence exactly as UNVERIFIED; it
  is never converted into PASS or omitted.
- If a release proceeds while any row is FAIL or UNVERIFIED, the delivery
  record must say so explicitly, per provider, preserving the uncertainty.
  Whether a non-PASS row blocks a release is a release-policy decision
  reserved to the operator at Human Acceptance; this gate records state and
  grants no waiver.

### Recording rules

- One row per production provider, in the canonical registry order above; no
  provider is added, merged, or dropped.
- "Checked at" is the diagnostic row's own `checkedAt` stamp (RFC-3339 UTC).
- "Evidence" names the command plus a short result reference (health, and the
  normalized reason code when not PASS).
- Record only the safe summary. The diagnostic output carries masked account
  hints and is user-specific: never paste raw output, account hints, quota
  percentages beyond what a row needs, user paths, or tokens into release
  records. The script's full output stays under `%TEMP%` and is never
  committed.

### Current recorded verification

Recorded 2026-10-03 on branch `release-provider-verification-gate`
(post-v0.8.4 lineage `d2be211` → `5f0412b`). This is a standalone dated
execution of this gate — not a retroactive verification record for any
released version.

| Provider | Checked at | Result | Evidence | Note |
| --- | --- | --- | --- | --- |
| OpenAI / Codex (`openai-codex`) | 2026-10-03T09:26:52Z | PASS | provider-quota-diagnostic.ps1 — health `live` | live quota window returned |
| Z.ai (`zai`) | 2026-10-03T09:26:52Z | PASS | provider-quota-diagnostic.ps1 — health `live` | 5-hour and monthly windows returned |
| OpenCode Go (`opencode-go`) | 2026-10-03T09:26:52Z | PASS | provider-quota-diagnostic.ps1 — health `live` | 5-hour / weekly / 30-day windows returned |
| Google Antigravity (`antigravity`) | 2026-10-03T09:26:52Z | PASS | provider-quota-diagnostic.ps1 — health `live`, fresh | cached-source windows returned, labels sanitized |
| Grok (xAI) (`grok`) | 2026-10-03T09:26:52Z | PASS | provider-quota-diagnostic.ps1 — health `live` | weekly-credit window returned |

Diagnostic secret scan: PASS (no credential-shaped material in the output).

## Phase 4 — Delivery (only after ACCEPTED)

1. Merge `release/v0.8.0-rc` into `main` (merge commit, as v0.7's `b788fe1`).
2. Create the annotated tag `v0.8.0` on the release SHA on `main` and push
   branch + tag.
3. Dispatch `.github/workflows/release.yml` (Actions → Release → Run
   workflow) with `version=0.8.0` and
   `expected_head_sha=<full SHA of the tagged release commit>`, plus the
   final short updater notes string (draft: `LimitScope v0.8.0 — threshold
   forecasts, runtime trust, accessibility and export improvements.`). The
   workflow itself enforces tag/ref/HEAD identity, four-way version
   agreement, exact-artifact selection, SHA-256/size manifest, and
   `latest.json` strictly last — a mismatch fails the run before publishing.
4. After the run succeeds: verify
   `https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.0/LimitScope_0.8.0_x64-setup.exe`
   and `latest.json` are reachable, and the run's
   `limitscope-signed-release` artifact carries the manifest matching the
   published installer's SHA-256/size.
5. Real updater E2E on the delivery machine: installed production 0.7.0 →
   check for updates → upgrade to 0.8.0 via the live feed.

## Phase 5 — Closeout

- Update `state/project-state.md` with the v0.8 delivery record; mark the RC
  worktree/branch as archive candidates (v0.6 pattern).
- Rollback / key-compromise procedures: `docs/updater-production.md`
  (corrective `v0.8.1` release path; `latest.json` repoint).

## Explicitly NOT done in the pre-RC phase

No version bump to 0.8.0 (integration branch remains 0.7.0); no
`release/v0.8.0-rc`; no merge to `main`; no tag; no GitHub Release; no
`latest.json` publication; no deploy; no repository rename or visibility
change; no `v0.7.0` alteration; no Claude implementation; no product
runtime behavior change.
