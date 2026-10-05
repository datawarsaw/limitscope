# Rate Limits roadmap after v0.3.0

- **Status:** Proposed plan, not yet committed scope
- **Date:** 2026-09-28
- **Baseline:** v0.3.0 (tag on `7de252b`, PR #3 merge from `release/v0.3.0-rc` at `713db95`)
- **Evidence inputs:** `docs/v0.3-product-spec.md`, `docs/v0.3-ui-acceptance.md`, `docs/provider-contract-audit.md`, `docs/claude-provider-discovery.md`, `docs/grok-provider-discovery.md`, `docs/mimo-provider-discovery.md`, `docs/human-acceptance-v0.3.0.md` + run sheet + helper docs, `docs/ci-and-release.md`, `docs/release-checklist-v0.2.0.md`

---

## Current state

**Released.** v0.3.0 shipped the frozen v0.3 scope: Graphite/Glass/OLED themes with persistence, local reset countdowns, bounded 24-hour quota history (500 newest observations per provider/window), deterministic confidence-gated projection, and honest Live / Cached / Stale / Refresh failed semantics with last-good retention. Validation at RC: 171 frontend tests, 81 Rust tests (4 live-credential tests ignored), green CI installer build, SHA-256-recorded installer `Rate Limits_0.3.0_x64-setup.exe`.

**Live providers:** Codex, Z.ai, OpenCode Go (local-logon APIs) and Antigravity (passive cache, freshness-aware). Claude and xAI Grok render as clearly-labeled mock cards.

**Built but not landed (untracked working-tree assets on `codex/mimo-provider-discovery`):**

| Asset | State | Live-verified? |
|---|---|---|
| Claude adapter (`src-tauri/src/claude.rs`, `src/providers/claudeProvider.ts` + tests) | Isolated: not in invoke handler, registry still serves the mock; 19 Rust + 19 TS tests green | **No** — no Claude Code install/credential exists on the dev machine |
| Grok adapter (`src-tauri/src/grok.rs`, `src/providers/grokProvider.ts` + tests) | Isolated, same non-registration; 22 Rust + 18 TS tests green | Failure paths only (live 401 probes 2026-09-27); positive quota path never observed — the only local xAI credential is expired, and refreshing it is forbidden by policy |
| Provider contract audit (`docs/provider-contract-audit.md`) | Read-only audit of all six providers | Verdict: no blockers, 4 SHOULD-FIX items, no framework rewrite justified |
| Human Acceptance triad (`scripts/human-acceptance-v03.ps1`, checklist, run sheet, helper doc) | Used for the v0.3.0 acceptance run | Proven: pre-flight caught a stale v0.2.0 dev binary being tested in place of the v0.3.0 build |
| MiMo discovery (`docs/mimo-provider-discovery.md`, committed in `7dd1ddd`) | Discovery only | No safe API-key quota endpoint exists; the console surface is cookie-gated and rejected |

**Known accepted follow-ups recorded in `docs/v0.3-ui-acceptance.md` but not shipped in the RC:** the faint-text contrast bump (`--text-faint: #71717c`, 3.4:1 — below WCAG AA for normal text at 10.5 px), `prefers-reduced-motion` support, and the dedicated `:focus-visible` guardrail test. All three exist proven on `feature/v0.3-ui-hardening`; none is in the RC.

**Process notes from the v0.3 release:** acceptance was formalized (40 blocking checks, mechanical PowerShell helper, SHA-256 installer verification); the release runtime has no native Computer Use surface, so the desktop smoke matrix stays environment-blocked and the helper is the workaround; the checklist precondition SHA drifted from the actual tested RC head and had to be reconciled in the run sheet; releases remain manual by policy (`docs/ci-and-release.md`).

---

## v0.3.1 — stabilization

A small patch release: accessibility correctness, one contract-alignment fix, and release hygiene. No provider registration, no retry-contract changes, no new features. Everything below is either porting already-proven hardening-branch work or an audit-recommended one-liner.

### 1. Text contrast hardening

- **Goal:** Raise `--text-faint` from `#71717c` (3.4:1 on Graphite cards at 10.5 px) to `#8a8a95` (≥4.8:1 on all three themes), as recorded in `docs/v0.3-ui-acceptance.md`.
- **Why now:** It is the one shipped-theme element that fails WCAG AA for normal text, and the fix is a token change already designed and verified on the hardening branch. Shipping it in a patch closes the only known a11y correctness gap in v0.3.0.
- **Acceptance criteria:** Faint-tier text meets ≥4.5:1 on Graphite, OLED, and the opaque Glass fallback; no layout shift at 360 px and 400 px; `src/themeCss.test.ts` asserts the token value; status colors and hierarchy are otherwise unchanged.
- **Risk:** Very low (CSS token only; dim text becomes slightly brighter, no data or behavior change).
- **Effort:** S (≤ half a day including visual verification in all three themes).

### 2. `prefers-reduced-motion` support

- **Goal:** Under `prefers-reduced-motion: reduce`, disable the refresh spinner animation and usage-bar transitions.
- **Why now:** Recorded as accepted follow-up work when the RC was cut; the implementation already exists on `feature/v0.3-ui-hardening`, so this is a port, not a design task.
- **Acceptance criteria:** With the OS preference set, the spinner does not animate and usage bars render without transitions while remaining accurate; without the preference, behavior is byte-identical to v0.3.0; a guardrail test in `themeCss.test.ts` fails if the media query is dropped.
- **Risk:** Very low (CSS + tests only).
- **Effort:** S.

### 3. Focus-visible guardrail test (test-only)

- **Goal:** Port the dedicated `:focus-visible` guardrail test from the hardening branch into the RC's `themeCss.test.ts`.
- **Why now:** Product-spec acceptance item 19 promises visible keyboard focus in all themes; the RC has the CSS rule but not the dedicated test that pins it, so a future CSS refactor could silently remove it.
- **Acceptance criteria:** The test fails if the shared `:focus-visible` outline rule is removed or scoped away from the tabbable controls (refresh button, selects, startup switch, Clear history, prediction block); no runtime change whatsoever.
- **Risk:** None (test-only).
- **Effort:** S.

### 4. Antigravity freshness — residual hardening

- **Goal (as originally scoped):** Flip `antigravityProvider.ts`'s default mapping for an absent/null `dataFreshness` from fresh to stale (contract audit SHOULD-FIX 2).
- **Correction:** the frontend default was **already** correct before the v0.3.0 RC. Only the v0.2-era adapter defaulted an absent verdict to `fresh`; the RC shipped the corrected `=== "fresh" ? "fresh" : "stale"` mapping, so the "indeterminate must never read as fresh" rule already held on the frontend. What v0.3.1 actually lands is the residual hardening of the paths that could still turn an undatable snapshot into a fresh verdict: a `fresh` verdict is trusted only when it arrives with a parseable `sourceUpdatedAt`, and the Rust half degrades a source stamp more than five minutes ahead of the machine clock to `stale` instead of reading it fresh indefinitely.
- **Acceptance criteria:** Unit tests pin fresh, stale, missing, invalid, and malformed stamps, the 24-hour boundary, and the future-skew tolerance boundary; undatable data is not prediction-eligible; all existing Antigravity tests pass.
- **Risk:** Low (defensive-path change).
- **Effort:** S.

### 5. Release hygiene bundle

- **Goal:** Add `artifacts/` to `.gitignore` (the helper doc explicitly warns not to commit personal acceptance reports); commit the v0.3 evidence set to `main` (acceptance helper script + docs, run sheet, Claude/Grok discovery docs, contract audit, this roadmap); add a v0.3.1 release checklist derived from the v0.3.0 run sheet.
- **Why now:** The v0.3 acceptance tooling and the two provider discovery documents exist only as untracked files on a side branch — a knowledge-loss risk and a provenance gap for the next release. `artifacts/` currently risks accidental personal-data commits.
- **Acceptance criteria:** `git status` no longer lists `artifacts/`; the listed docs and `scripts/human-acceptance-v03.ps1` are tracked on `main`; a v0.3.1 checklist exists with the SHA-256 and post-merge steps pre-templated.
- **Risk:** None (docs, script, and gitignore only — no application code).
- **Effort:** S.

**Explicitly not in v0.3.1:** the structured `transient`/`httpStatus` error field (touches the retry contract across four backends — wrong size for a patch; see v0.4), the dev scenario harness (dev-only convenience, stays deferred), Claude/Grok registration (v0.4), and the dead `planType` wire field (fold into v0.4 cleanup).

---

## v0.4 — next capability

Two themes. The product capability is provider expansion; the engineering theme is the bounded cleanup the contract audit conditions on that expansion. No framework rewrite, no new product surfaces.

### Theme 1 — Land the built-but-unverified providers (Claude first; Grok when a credential exists)

- **Goal:** Replace the Claude and xAI Grok mock cards with the already-implemented, already-tested live adapters.
- **User value:** Two more real quota providers on the dashboard — the most requested kind of value this product has, and the adapters already exist (38 + 40 passing tests). This is expansion at the cost of verification and wiring, not new implementation.
- **Ordering and gate:**
  1. **Cadence decision first** (audit SHOULD-FIX 4): the Claude usage endpoint destabilizes below ~180 s per token and 429s aggressively, but the app's refresh interval is a global setting with a 1-minute option. Either gate Claude's polling to ≥3 minutes or explicitly accept documented 429 churn (last-good + single retry already contain it). Decide before flipping the registry.
  2. Register `get_claude_usage` in `tauri::generate_handler![]`, drop the `#[allow(dead_code)]`, swap `MockProvider({id:"claude"})` for `new ClaudeProvider()` (ids already match; no `App.tsx` change).
  3. **Live verification — the hard gate:** on a machine with logged-in Claude Code, run `cargo test -- --ignored live_fetch` once and observe a real refresh cycle at the chosen cadence. The endpoint is undocumented; schema drift is a live risk until this passes. Do **not** register without it.
  4. Grok follows the same path when an unexpired xAI credential exists (`grok login`, or re-auth xAI in OpenCode): run its ignored live test, confirm the real payload matches `docs/grok-provider-discovery.md` §3.1, then swap. Note the card shape legitimately changes (mock "5-hour"/"30-day" → the real Credits/on-demand windows).
- **Acceptance criteria:** Live cards show real windows with reset times and enter the standard failure/last-good taxonomy; mocks are gone from the registry; the dashboard subtitle no longer says "others mocked"; human acceptance run executed on the installed build with the updated checklist.
- **Risk:** Medium — undocumented endpoints on both providers; contained by last-good retention, structured errors, token redaction, and the existing failure matrices. The bigger operational risk is credential availability, which is scheduling, not engineering.
- **Effort:** M per provider once a credential exists (wiring + cadence gating + live verification + checklist updates).

### Theme 2 — Provider-layer hardening after the fifth adapter

- **Goal:** Land the contract audit's SHOULD-FIX/cleanup list once Theme 1 confirms the adapter pattern a fifth time — and nothing more.
- **Contents, in audit order:**
  1. Add a structured `transient` (or `httpStatus`) field to command error payloads and read it in `isTransientCommandError`, deleting the message-regex retry contract (SHOULD-FIX 1). This also de-risks every future provider.
  2. Introduce the small TS adapter factory `createInvokingProvider(id, name, command)` plus a freshness-aware variant — ~400 lines of near-identical adapters collapse to ~80. Only after the Claude swap; explicitly a helper, not a framework. No Rust trait (audit: the five backends share boilerplate, not substance).
  3. Error-code unification (`schema_changed` → `unexpected_response`, `not_logged_in` → `credential_missing`), one monthly-label convention, drop the dead `planType` wire field.
- **Acceptance criteria:** Retry behavior is byte-identical (every existing retry/last-good test passes against the structured field); no provider semantic changes; each sub-item lands as its own reviewable commit.
- **Risk:** Medium for the retry-field change (it is the contract), contained by the tests that currently pin the message wording — those tests get rewritten, not deleted. Low for the rest.
- **Effort:** M total, cleanly splittable; item 1 can also land early in v0.4 as preparation for the provider swaps.

**Deliberately not a v0.4 theme:** prediction/history UX evolution (no field evidence yet — the v0.3 acceptance run could not naturally reach medium confidence in its 10–15 minute window, so real-world prediction visibility is unproven; collect feedback first), tray glanceability (RADAR WATCH), and any architecture work beyond Theme 2.

---

## WATCH backlog

Each item stays out of v0.3.1/v0.4 until its trigger fires. Promotion needs the stated evidence, not opinion.

| Item | Trigger condition | Evidence needed to promote |
|---|---|---|
| **MiMo (Xiaomi Token Plan)** — `NEEDS_UPSTREAM_ENDPOINT` | Xiaomi publishes a documented, API-key (`tp-`/`ttp-`) authenticated Token Plan quota endpoint with a JSON schema | Official docs or resolution of `XiaomiMiMo/MiMo-Code` issue #2495; one bounded read-only GET against the user's own key confirming schema; isolated parser tests. The cookie-gated console surface stays rejected permanently unless Xiaomi replaces it with a documented API-key surface |
| **Native Mica/Acrylic transparency** | A dedicated shell experiment passes the full matrix | Tray show/hide, startup, resize/drag, close-to-tray, single-instance, autostart-hidden, Windows 10/11, and WebView2 fallback — all with a readable opaque fallback (per v0.3 spec §3) |
| **Prediction / history UX evolution** | Field feedback after v0.3.0 usage | Evidence that users hit the confidence gate's sharp edges (e.g., prediction never reaching medium at the default 5-minute cadence) or concrete requests; then scope the smallest change, not a redesign |
| **Historical charts / analytics** | Sustained user demand plus a retention/privacy decision | A separate retention and visualization contract beyond the 24-hour prediction store |
| **Installer signing** | A code-signing certificate is acquired and the repo-secrets policy is revisited | Cert + CI secret handling decision documented against the current "unsigned by policy" stance in `docs/ci-and-release.md` |
| **Glanceable tray modes** (menu-title/icon usage hints) | User demand for at-a-glance quota without opening the dashboard | Concrete demand; RADAR keeps this WATCH, not ADOPT |
| **ML forecasting** | Real persisted history shows the deterministic baseline is inadequate | Measurable improvement over the deterministic engine on representative local data |
| **Account switching** | Proven multi-account demand | RADAR WATCH stands; no current evidence |
| **Release automation** (`release.yml`) | Release cadence pain from the manual process | Two+ releases where the manual checklist was the bottleneck rather than the code |
| **Toolchain pins** (`rust-toolchain.toml`, `engines`) | A stable-channel or Node drift actually breaks CI | The breakage itself (per `docs/ci-and-release.md` caveats — don't pre-pin) |
| **Dev scenario harness** (`?uiScenario=`) | Repeated manual-acceptance friction staging backend states | The v0.3.1 acceptance run shows the helper + checklist still leaves painful gaps |

---

## Provider roadmap

| Provider | Today | Next step | Disposition |
|---|---|---|---|
| Codex / Z.ai / OpenCode Go | Production, live | Maintain; inherit audit cleanups | ADOPT (keep) |
| Antigravity | Production, passive cache | v0.3.1 freshness hardening (residual indeterminate paths) | ADOPT (keep) |
| Claude Code | Isolated adapter, untracked; mock card in registry | v0.4 Theme 1: cadence decision → register → live-verify → swap | **TEST** — implementation complete (19+19 tests), **not live-verified**, endpoint undocumented |
| xAI Grok | Isolated adapter, untracked; mock card in registry | v0.4 Theme 1 second: needs unexpired xAI credential → live payload check → swap | **TEST** — failure paths live-verified (401 probes), positive quota path never observed |
| Xiaomi MiMo | Discovery only, committed | Nothing until upstream changes | **WATCH** — `NEEDS_UPSTREAM_ENDPOINT`; console/cookie surface rejected |

---

## Release-process improvements

Lightweight only — formalize what worked in v0.3, add nothing heavyweight. Releases stay manual per `docs/ci-and-release.md` policy.

1. **Keep the acceptance triad per release.** Checklist + run sheet + PowerShell helper worked (the helper's pre-flight caught a stale v0.2.0 binary being tested as if it were the RC). Generalize the helper's versioned name (`human-acceptance-v03.ps1`) or copy it per release, and make a pre-flight file-version mismatch against the expected release a loud, explicit warning.
2. **Codify the post-merge gate.** The v0.3 sequence (merge PR → post-merge CI green on main → re-verify installer artifact → tag) lives only in the v0.3.0 run sheet. Put it in a standing `docs/release-checklist-template.md` so v0.3.1+ starts from a template, not a memory.
3. **Artifact hash + annotated tag + GitHub Release.** Keep SHA-256 verification as a required step (it already exists in the helper's `-InstallerPath` inspection); adopt annotated tags carrying the release-notes body; publish a GitHub Release per version with the installer attached and its SHA-256 in the notes. Still manual — revisit a `release.yml` only if the WATCH trigger above fires.
4. **Record the tested SHA at acceptance start.** The v0.3 checklist precondition recorded `505f1b9` while the actually-tested RC head was `713db95`; the run sheet had to reconcile. Generate the run sheet's source-SHA row from the artifact being tested, once, at run start.
5. **Accept the tooling boundary explicitly.** The release runtime has no native Computer Use surface for the app, tray, or installer UI (release notes: smoke matrix environment-blocked); browser automation also cannot reach authenticated desktop surfaces. The PowerShell helper is the durable workaround — invest there rather than in browser tooling, and keep stating the limitation in every release's validation section.
6. **Asset naming quirks stay documented, not "fixed".** The space in `Rate Limits_0.3.0_x64-setup.exe` and the 32-bit NSIS stub inside an x64-target bundle are known quirks; record them in each checklist rather than churning the bundle config.

---

## Explicitly deferred

Carried forward as excluded, with the reason the exclusion stands after v0.3.0:

- **Provider framework / Rust provider trait** — the contract audit re-confirmed after five backends: they share boilerplate, not substance; only the small TS factory is justified.
- **Cost accounting, pricing catalog** — different provider semantics; would dominate the compact quota/reset core.
- **Cloud sync, telemetry, monitoring/diagnostics** — local-first is a privacy promise; the honest error/status presentation already shipped is the diagnostics surface.
- **ML forecasting** — deterministic engine is explainable and tested; no data showing inadequacy.
- **Native Mica/Acrylic** — unproven shell experiment; WATCH trigger stands.
- **Long-term analytics, historical charts** — no retention/privacy decision, no demand evidence.
- **Mobile companion / browser dashboard** — out of product scope.
- **Prediction/history UX changes** — deferred for evidence, not rejected (WATCH above).
- **Decorative glass as a dependency, 3D/mascot novelty** — IGNORE dispositions unchanged.

---

## Recommended next action

One concrete first task: **create branch `chore/v0.3.1-stabilization` from the v0.3.0 line and land roadmap items 1–4** (contrast token, reduced-motion CSS + guardrail tests, focus-visible test port, Antigravity freshness hardening) as one small PR with `npm test`, `cargo test --locked`, and `npm run build` green. The release-hygiene item (5) can ride along or follow immediately as a docs-only commit.

---

## RADAR dispositions

**v0.3.1:** **ADOPT** — text contrast hardening, `prefers-reduced-motion`, focus-visible guardrail test, Antigravity freshness hardening, release hygiene. (All are proven or audit-recommended one-liners; nothing here is experimental.)

**v0.4 themes:** **TEST** — Claude live integration (implementation done; live verification is the gate). **TEST** — Grok live integration (failure paths live-verified; positive path pending a credential). **ADOPT** — structured transient/httpStatus error field and error-code unification (audit-recommended contract fixes). **TEST** — TS adapter factory (only after the fifth adapter confirms the pattern).

**MiMo:** **WATCH** — unchanged; `NEEDS_UPSTREAM_ENDPOINT`. No safe API-key quota endpoint exists; the cookie-gated console surface is permanently rejected.

**Claude:** **TEST** — isolated implementation complete and unit-tested (19 Rust + 19 TS), never live-verified (no local Claude Code installation or credential), not registered. Promotion to ADOPT requires: the ≥180 s cadence decision, registry swap, and one successful live verification on a machine with logged-in Claude Code.

**Grok:** **TEST** — isolated implementation complete and unit-tested (22 Rust + 18 TS); endpoint existence, routing header, and expired/missing-auth failure shapes live-verified 2026-09-27; the positive quota path has never been observed (only local credential is expired; OAuth refresh that writes back to files owned by other tools is forbidden by policy). Promotion to ADOPT requires an unexpired xAI credential (`grok login` / OpenCode re-auth), one live payload check against §3.1 of its discovery doc, then the registry swap.

**DESTINATION:** Rate Limits
