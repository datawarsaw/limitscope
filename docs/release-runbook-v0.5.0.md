# LimitScope v0.5.0 — Release Runbook

Companion: [human-acceptance-v0.5.0.md](human-acceptance-v0.5.0.md) (SHA-bound; gates G1/G2
unchanged from the v0.3.1/v0.4.0 runbooks, which this file assumes unchanged otherwise).

## Candidate identity

| Item | Value |
| --- | --- |
| Candidate branch | `release/v0.5.0-rc` |
| Base | `feature/v0.5-limitscope-brand` at `178a942` (newest tip of the accepted v0.5 lane stack; itself rooted on `release/v0.4.0-rc` at `0323c0a`) |
| Consolidation commits | `97f988a` (chore(release): bump LimitScope to 0.5.0), `6cda2d3` (fix(release): installer migration survives real v0.4 state) |
| Tested product SHA | `6cda2d3` — product tree identical to the built installer; docs-only RC record commits may sit above it |
| RC installer | `LimitScope_0.5.0_x64-setup.exe`, 1,699,001 bytes, SHA-256 `6cc406cc7290939ae0df69a110e826ab05b6561d4b2a56757700d78e5b2f5acd` |
| FileVersion / ProductVersion / ProductName | `0.5.0` / `0.5.0` / `LimitScope` |
| Internal identifier | `com.ratelimits.desktop` (unchanged on purpose; legacy storage keys `rate-limits.*` retained for continuity) |
| Version manifests | `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (rate-limits entry only), `src-tauri/tauri.conf.json` — all `0.5.0` |

## v0.5.0 feature scope (accepted lanes, all present)

- Floating quota window + ergonomics: compact provider bar, hover/quick view/detail,
  Open action, context menu, position persistence, pin (always-on-top), no auto-hide,
  no duplicate fetch ownership.
- Shared Rust runtime: Rust is the sole provider refresh owner (one cycle at a time,
  bounded coalescing, centralized scheduler; TS invokes only runtime/history/window
  commands — pinned by `runtimeTriggerGuardrail.test.ts`).
- Rust-owned history: account-aware `(providerId, account, windowLabel)` persistence in
  `%APPDATA%\com.ratelimits.desktop\quota-history-v1.json`, `historyRevision` flow into
  TS predictions, one-time legacy localStorage import, clear-history intact.
- Runtime resilience: standard `Retry-After` parsing (delta-seconds and HTTP-date),
  cooldown with a hard ceiling, manual refresh respects cooldown, wake/suspend and
  reconnect refresh triggers, no fake history on cooldown skips.
- Threshold notifications: Rust-owned policy at 80% / 95%, crossing semantics with
  reset-cycle re-arm, per-account isolation, stale/error/cooldown exclusion, default
  OFF, per-cycle burst cap, persisted dedup state (`quota-notifications-v1.json`).
- Main window redesign: provider rail, primary panel, compact provider details, Needs
  Attention, preserved settings drawer, responsive reflow (no horizontal overflow at
  360–900 px), prediction gating preserved.
- LimitScope branding: product renamed from Rate Limits; Dock Tick glyph; refreshed app,
  tray, and installer icons; installer identity `LimitScope` with the preserved internal
  identifier; NSIS migration hooks for the Rate Limits → LimitScope in-place upgrade;
  current-brand string guardrail test.

Production provider registry (pinned, order-stable, guardrail-tested): `openai-codex`,
`zai`, `opencode-go`, `antigravity`, `grok`. Claude remains a simulated-only placeholder,
unreachable in production runtime; no mock code is registered in production.

## Validation record (automated, at the tested product SHA)

- Frontend: 252 tests passed across 25 files; `npm run build` PASS.
- Rust: 225 passed, 5 ignored (live/credential gates), 0 failed with `cargo test --locked`.
- Installer: `npm run tauri build` PASS (NSIS, x64 app, x86 NSIS stub as before).
- Upgrade smoke: PASS — Rate Limits 0.4.0 → LimitScope 0.5.0 RC on a live install with
  seeded non-default state (glass theme, 15-min auto-refresh, launch-at-startup on,
  populated history, autostart Run entry). Verified: single ARP entry, no side-by-side
  pair, in-place install dir, old binary removed, settings/history/notification-state/
  WebView profile continuity, Run-key migrated to `LimitScope`, single-instance still
  enforced. The smoke found and fixed two migration-hook defects (`6cda2d3`): quoted
  registry `InstallLocation`/`UninstallString` values corrupted `$INSTDIR`, and the old
  uninstaller's autostart cleanup raced the Run-key migration (now captured in
  PREINSTALL before the old uninstaller runs).
- Native smoke: main window opens and populates, manual refresh advances the update
  stamp, floating window renders pinned with live chips, tray menu (Open / Show floating
  quota bar / Refresh / Quit) works, settings drawer + notification toggle round-trip
  with persisted state, Graphite/Glass/OLED switch with correct backgrounds (OLED true
  black), restart preserves settings, no black screen, no duplicate instance.
- Responsive: no horizontal overflow at 360/400/520/700/900 px widths (measured in the
  running installed app).
- Bundle audit: no mock-provider registration strings, no `claude`, no secret-shaped
  material (JWT/Bearer/api-key patterns), no v0.6 feature strings, no old product name;
  the only `simulated` occurrences are defensive exclusion guards in shared paths.
  The string `Claud…` seen in the Antigravity card is live provider window-label data
  (Antigravity reports Claude/Grok weekly pools), not product identity.

## Stage mapping

| Stage | v0.5.0 values |
| --- | --- |
| 1 — Candidate branch and CI green | PR from `release/v0.5.0-rc`; version already `0.5.0` before the RC build; STOP gate G0 unchanged |
| 2 — Human Acceptance | [human-acceptance-v0.5.0.md](human-acceptance-v0.5.0.md) with `-AppVersion 0.5.0`; gates G1/G2 unchanged |
| 3 — Post-acceptance validation | re-run the deterministic suites on the acceptance SHA; gate G3 unchanged |
| 4 — Merge | PR `release/v0.5.0-rc` → `main` after PASS; gate unchanged |
| 5 — Artifact verification | `rate-limits-windows-installer` from the post-merge run; expected filename `LimitScope_0.5.0_x64-setup.exe`; FileVersion/ProductVersion `0.5.0`; ProductName `LimitScope`; gate G4 unchanged |
| 6 — Annotated tag | annotated tag `v0.5.0` on the verified merge SHA only after acceptance PASS; gate G5 unchanged |

## Release record (v0.5.0 — open)

| Field | Value |
| --- | --- |
| Candidate branch / SHA | `release/v0.5.0-rc` / `6cda2d3` (tested product SHA; docs-only record commits above) |
| Human Acceptance | PENDING |
| Merge / tag / GitHub release | none — this run ends at RC |

## Explicitly out of scope for v0.5.0

- `feature/updater-foundation` (signed in-app update flow with public release feed):
  READY FOR FUTURE INTEGRATION — NOT PART OF V0.5.0. Its one commit (`04bd4a0`) is
  verified NOT an ancestor of the candidate; no signing keys or CI secrets were created.
- All `feature/v0.6-*` work (redacted diagnostics export, persisted disk last-good
  cache, provider hide/order preferences): the three v0.6 branches carry zero commits
  beyond the v0.5 tree; a source and bundle sweep confirms none of the features leaked.
- Notification threshold changes, further UI redesign, updater rollout.

## Provenance note

The RC consolidation was performed on `release/v0.5.0-rc` created fresh from
`feature/v0.5-limitscope-brand` (`178a942`). All seven accepted v0.5 lanes are linear
ancestors of that tip (floating-quota-window `808d690` → floating-ergonomics `04b6385`
→ merge `059adc0` onto the v0.4 RC tree → shared-runtime `dc48d11` → rust-history
`6f56016` → runtime-resilience `6e054ed` → threshold-notifications `fa30e5e` →
main-window-redesign `06f6629` → brand `178a942`), so no merge commits were
manufactured for the RC. `main` (`7411ea0`, the v0.3.1 release closeout) and
`release/v0.4.0-rc` (`0323c0a`) are both verified ancestors of the candidate; no v0.5
tag existed before this branch. The upgrade-smoke evidence above was gathered on the
target machine against the real installed Rate Limits 0.4.0 product.
