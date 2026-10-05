# LimitScope v0.8.0 — Release Notes (prepared pre-RC)

- **Status:** PREPARED, NOT YET RELEASED. These notes were prepared on
  `integration/v0.8-product-rehearsal` at the green integration candidate
  `2720efd2bde7044537a68c88718d40988d23a8ef`, before the v0.8.0 release
  candidate exists. No version bump, no RC branch, no tag, no release, and no
  updater publication has happened. The final short notes string for the
  updater feed is chosen when the release workflow is dispatched (see
  `docs/release-runbook-v0.8.0.md`).
- **Companion documents:** `docs/v0.8.0-release-candidate.md` (RC contract and
  acceptance-target resolution), `docs/release-runbook-v0.8.0.md`
  (delivery runbook), `docs/a11y-contract-v0.8.md` (accessibility rows the RC
  gate must record).

LimitScope v0.8.0 — "Prediction you can act on" — adds forward-looking
threshold estimates, hardens the runtime against corrupt provider data, and
closes the top accessibility and desktop-ergonomics findings, on top of the
v0.7.0 production baseline. The canonical metric, history ownership,
prediction purity, provider set, updater endpoint, and all v0.7 behavior are
preserved.

## Highlights

- **Threshold forecasts you can act on (Lane A).** Each quota window now
  shows when the 80% and 95% used levels are projected to be reached, as an
  estimate anchored on the newest observation: `Est. 95% used in ~3h 30m`,
  `80% used reached` (observed fact), `95% used not expected before reset`,
  or `Estimate unavailable`. Estimates never imply certainty, never claim a
  cause, cost, or token amount, and never render at low confidence. They read
  identically in Used and Remaining modes — the wording always speaks in used
  terms.
- **Runtime trust (Lane B).** A provider-announced reset time is trusted only
  when it is plausible for the window label (per-label horizon table with a
  5-minute clock-skew tolerance); an implausible bound is dropped, not
  trusted, and the window keeps its data. A suspicious usage drop (>5 points
  with no announced reset) is withheld from history until a second sample
  confirms or refutes it, so a single glitched sample can no longer split a
  cycle or corrupt predictions, analytics, or history. A cached echo of a
  glitch can never serve as the confirming sample.
- **Accessibility and desktop ergonomics (Lane C).** Main window stays inside
  the monitor work area at high DPI (launch, DPI change, resize, monitor
  loss) and never enlarges; the settings drawer scrolls instead of pushing
  commands off a short viewport; the tray gained a state-consistent Show/Hide
  floating item (hiding never stops monitoring, Refresh never activates the
  main window); prediction tooltips persist while reading, survive pointer
  travel into the tooltip, pin on click/tap, and dismiss with Escape; the
  settings drawer takes focus on open and restores it to the Settings button
  on Escape; dismissing the floating bar returns focus to a surviving
  surface. The full HUMAN_REQUIRED native matrix (DPI scale ladder, 200%
  text, tray keyboard access) is recorded at RC in `docs/a11y-contract-v0.8.md`.
- **Usage data export (Lane D).** One-click CSV or JSON export of exactly the
  retained 7-day history: `providerId, providerName, account, windowLabel,
  usedPercent, observedAt, resetAt, resolution` — no credentials, raw
  payloads, settings, or non-observation data. Gaps are absent rows, never
  zero-filled; the JSON envelope documents the re-derivable reset-boundary
  rule.

## Continuity and boundaries

- Product identity stays `LimitScope`; the internal crate name (`rate-limits`),
  app identifier (`com.ratelimits.desktop`), settings/history storage, and
  updater endpoint (`datawarsaw/limitscope-releases`) are unchanged —
  upgrading from v0.7.0 keeps settings, history, and autostart.
- No new provider. Claude Code registration remains deferred behind its
  live-verification gate (plan §8); the production registry still carries
  exactly `openai-codex`, `zai`, `opencode-go`, `antigravity`, and `grok`.
- No cost/token accounting, no ML forecasting, no retention change (7-day
  horizon, 500/350 bounds, 30-minute compaction unchanged), no storage schema
  migration, no telemetry, no cloud sync.
- Release engineering (workflow-only, no product behavior change): the
  release workflow now verifies tag/ref/HEAD identity and four-way version
  agreement, selects the exact installer/signature by name, records SHA-256
  and sizes for the artifacts, and keeps `latest.json` strictly last.
  Controlled RC updater tests run through the local harness
  (`scripts/update-harness.mjs`), never through a publishing RC mode.

## Validation recorded at the integration candidate

Full pre-RC gate on the integration worktree (green candidate
`2720efd…` + the docs/tooling-only hardening commit): frontend vitest
712/712 across 53 files, `cargo test --locked` 450 passed / 0 failed /
6 ignored, `npm run build` PASS, `npm run scan:secrets` PASS, plus the
per-lane matrices of the architecture plan §12 already green at the lane
and integration heads. The authoritative per-row acceptance matrix lives in
the planning branch (`docs/v0.8-integration-rehearsal-acceptance-matrix.md`).

## Upgrade notes

Strictly newer versions only; equal versions and downgrades are rejected.
The installer is minisign-signed (integrity/authenticity against the
embedded public key; not Windows Authenticode). If an update fails
signature verification, the download is deleted and never executed.
