# Release Notes — Rate Limits v0.2.0

**Date:** 2026-09-27
**Artifact:** `src-tauri/target/release/bundle/nsis/Rate Limits_0.2.0_x64-setup.exe` (1,550,202 bytes)
**SHA-256:** `0f92de6f46ebf13e2807c98fa2814b9c79e1095c3f1c6e8ed30db3f78b1e1b42`

Rate Limits is a Windows 11 system-tray utility that shows AI provider
rate-limit usage in a compact dark dashboard. All credential handling lives in
the Rust process; the WebView only ever receives normalized quota windows.

## What's in v0.2.0

### Core app

- **System tray app** — left-click opens/focuses the dashboard; right-click
  gives Open / Refresh / Quit.
- **Single instance** — a second launch never starts a second process; it
  focuses the dashboard owned by the running instance.
- **Close-to-tray** — closing the window hides it and keeps the app running;
  the only exit path is the tray menu's Quit.
- **Start with Windows (opt-in)** — autostart via the official
  `tauri-plugin-autostart` plugin (default off, reconciled against the OS Run
  key at every launch). An autostart launch starts hidden in the tray.
- **Configurable auto-refresh** — 1 / 5 / 15 / 30 minutes (default 5), also
  driving the footer stale indicator (2 × interval without a successful
  refresh).
- **Refresh coalescing / single-flight** — at most one refresh cycle runs at a
  time; triggers landing mid-cycle are coalesced into one follow-up rather
  than stacking concurrent cycles.
- **One bounded retry** — transient network / HTTP 429 / 5xx failures get a
  single retry; auth, credential, and schema failures never retry.
- **Last-good retention** — when a provider's refresh fails, the card keeps
  the last good data of the session (status dot red, tooltip carries the
  reason) instead of blanking.

### Providers

- **OpenAI / Codex — live.** Real quota windows via the local Codex CLI login
  (`get_codex_usage`); the app never refreshes OAuth tokens itself.
- **Z.ai — live.** Real quota windows via the GLM Coding Plan key
  (`get_zai_usage`), resolved read-only from `ZAI_API_KEY`, the ZCode config
  (enabled `api.z.ai` providers), or the encrypted credential store. Base URL
  is pinned to `https://api.z.ai` (new in v0.2.0).
- **OpenCode Go — live.** Real quota windows via the local OpenCode auth key
  (`get_opencode_go_usage`).
- **Google Antigravity — integrated, passive cache.** Reads the quota cache
  the OpenCode Antigravity plugin maintains
  (`~/.config/opencode/antigravity-accounts.json`) — read-only, no Google API
  calls, no OAuth refresh, no cache writes. The snapshot's age is classified
  against a 24-hour threshold: a stale snapshot keeps its windows visible but
  the card reports status "stale" with a note, so cached data never reads as
  live.
- **Claude and xAI Grok** remain mock providers (small random walk on each
  refresh).

### Changes since v0.1.0

- Version bump 0.1.0 → 0.2.0 across all five manifests (installer product
  version and tray user-agent derive from these).
- Z.ai scheme pinning: a provider base URL must be exactly scheme `https` and
  host `api.z.ai` to qualify — closing the `http://` downgrade path flagged in
  the security review (F3).
- Google Antigravity integration completed: Rust command registered, adapter
  in the registry, dashboard subtitle updated.
- Codex schema-drift hardening: an upstream HTTP 200 that omits
  `rate_limit` (or carries no usable primary window) now maps to a structured
  `unexpected_response` error — matching the contract Z.ai and OpenCode Go
  already follow — instead of parsing as success with zero windows, which
  would have replaced the last-good card with an empty one. Schema failures
  are never retried.
- README reconciled with the shipped state.

No features were added in this release beyond the Google Antigravity
integration; the Codex change above is failure-handling hardening, not a
change to the normal data path.

## Verification

| Gate | Command | Result |
| --- | --- | --- |
| Rust tests | `cargo test` | **74 passed, 0 failed**; 4 live-network tests `#[ignore]`d |
| Frontend tests | `npm test` (vitest) | **47 passed, 0 failed** (5 files) |
| Typecheck + bundle | `npm run build` | Clean; 157.23 kB JS / 4.38 kB CSS |
| Release bundle | `npm run tauri build` | NSIS installer produced (release profile: LTO, strip, `panic="abort"`) |

The installer additionally passed an install → smoke-test pass on the
installed build (launch, tray presence, refresh, close-to-tray, single
instance, tray Quit) per `docs/release-checklist-v0.2.0.md`.

## Known limitations

- **Live providers use unofficial endpoints.** A response-shape change
  upstream surfaces as a structured error card (never a crash), recovering on
  the next successful refresh.
- **Antigravity is a passive snapshot, not a live fetch.** It reflects the
  OpenCode Antigravity plugin's last cache write; if that plugin has never
  run, the card shows a setup hint. Snapshots older than 24 h display as
  stale.
- **Codex OAuth is never refreshed by this app.** If the Codex session
  expires, run `codex login` once; the app picks up the fresh login
  automatically.
- **Autostart stores an absolute exe path.** After moving or reinstalling the
  app to a different folder, re-enable the setting once. Disabling via Task
  Manager's "Startup apps" is only picked up at the app's next launch.
- **Dev builds register their own autostart path and share the single-instance
  slot** with the installed build (same app identifier).
- **The tray icon lives in the Win11 overflow flyout by default** — drag it
  onto the taskbar to pin it.
- Not exercised in QA: autostart Run-key path verification end-to-end, live
  provider credential rotation.
