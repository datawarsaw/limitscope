# Release Checklist — Rate Limits v0.2.0

**Date:** 2026-09-27 · **Artifact:** `src-tauri/target/release/bundle/nsis/Rate Limits_0.2.0_x64-setup.exe` (1.48 MiB, built 2026-09-27 18:35) · **Verdict: ALL CHECKS PASSED**

This checklist covers the v0.2.0 release hardening: version bump, Z.ai HTTPS
scheme pinning (security-review finding F3), repo-root housekeeping, and a
full build → install → smoke-test pass on the **installed** build (per the
v0.1.0 review's note F6). It supersedes section 8 of
[`security-and-release-review.md`](security-and-release-review.md).

## 1. Changes in this release

| Change | Files | Notes |
| --- | --- | --- |
| Version 0.1.0 → 0.2.0 | `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tauri.conf.json` | Tray user-agent (`rate-limits/<version>`) and the installer product version derive from these automatically |
| Z.ai scheme pinning to HTTPS (F3) | `src-tauri/src/zai.rs` (`provider_base_url_is_zai`) | A provider baseURL must now be exactly scheme `https` **and** host `api.z.ai` to qualify; `http://api.z.ai`, scheme-less bases, and other schemes are rejected before any key is sent. Provider semantics otherwise unchanged. Test added: `provider_base_url_requires_https_scheme_on_api_z_ai` |
| `final-shot.png` moved out of repo root | → `docs/final-shot.png` | Unreferenced dev screenshot; `docs/screenshot.png` remains the README image |
| Release-QA tray helpers | `scripts/tray-uia-overflow.ps1`, `scripts/tray-quit.ps1` | Locate the tray icon in the Win11 overflow flyout; drive the tray menu to Quit. Dev helpers only, not shipped |

No features added, no provider semantics changed, no providers integrated.

## 2. Manifest consistency — PASS

| Manifest | Field | Value |
| --- | --- | --- |
| `package.json` | `version` | 0.2.0 |
| `package-lock.json` (both records) | `version` | 0.2.0 |
| `src-tauri/Cargo.toml` | `version` | 0.2.0 |
| `src-tauri/Cargo.lock` (`rate-limits`) | `version` | 0.2.0 |
| `src-tauri/tauri.conf.json` | `version` | 0.2.0 |
| `src-tauri/tauri.conf.json` | `productName` / `identifier` | `Rate Limits` / `com.ratelimits.desktop` |
| `src-tauri/capabilities/default.json` | permissions | `core:default`, `autostart:default`, main window only (unchanged) |
| Dashboard subtitle vs provider registry | — | Match (see §4 caveat) |

## 3. Build & test gates — PASS

| Gate | Command | Result |
| --- | --- | --- |
| Rust tests | `cargo test` | **66 passed, 0 failed**; 4 live-network tests correctly `#[ignore]`d |
| Frontend tests | `npm test` (vitest) | **44 passed, 0 failed** (5 files) |
| Typecheck + bundle | `npm run build` | Clean; 157.23 kB JS / 4.38 kB CSS |
| Release bundle | `npm run tauri build` | NSIS produced `Rate Limits_0.2.0_x64-setup.exe` in 2 m 55 s (release profile: LTO, strip, `panic="abort"`) |

## 4. Smoke test (installed build) — PASS

Installed silently (`/S`, per-user, no admin): `%USERPROFILE%\AppData\Local\Rate Limits\rate-limits.exe`;
HKCU uninstall registry reports `DisplayVersion 0.2.0`.

| Step | Method | Result |
| --- | --- | --- |
| Launch | Start installed exe | Process up from the install path; window visible; all provider cards render with live data (Codex, Z.ai, OpenCode Go live; Google Antigravity stale-cache snapshot; Claude mocked) |
| Tray | UIA probe (`scripts/tray-uia-overflow.ps1`) | Icon present as `SystemTray.NormalButton` "Rate Limits" (40×40) in the notification-area overflow flyout |
| Refresh | In-window refresh button (`scripts/click-client.ps1`), before/after screenshots | Footer `Updated 6:37 pm` → `6:38 pm`; mock re-seeded (84 % → 82 %); OpenCode Go reset times rolled forward |
| Close-to-tray | `scripts/close-window.ps1` (WM_CLOSE) | Window hidden, **process survived** |
| Reopen | Launch exe again (second instance) | Still exactly **1 process** (single-instance callback showed the existing window) |
| Quit | Tray icon right-click → menu → `Quit` (`scripts/tray-quit.ps1`; popup verified on-screen, `{UP}`+`{ENTER}` activates the last item) | **Process exited** — the app's only exit path works |

Not exercised (unchanged since v0.1.0 QA, no code touched): autostart Run-key
path verification, live provider credential rotation.

### Caveats

1. **Concurrent Antigravity integration is in this artifact.** While this
   checklist was being prepared, a parallel work stream landed the Antigravity
   integration in the same tree at 18:26–18:28 (registry + Rust command
   registration + subtitle). The entire gate in §3 ran *after* that landing,
   and the installer provably matches the tree (no source file is newer than
   the artifact; `cargo test` covered the merged state). The integration is
   **included** in v0.2.0. At release time the README still described
   Antigravity as "not yet registered (in progress)" — that documentation
   drifted behind the code and should be reconciled by that work stream.
2. **Refresh click isolation is approximate:** auto-refresh runs at the 1-minute
   default, so the before/after diff proves data flows through the refresh
   path, not that the click alone triggered it. All three refresh triggers
   (button, tray event, visibilitychange) funnel through one guarded
   `refresh()` verified by unit tests.
3. The Win11 tray icon lives in the **overflow flyout** by default; UIA-driven
   QA must open the chevron first (`scripts/tray-uia-overflow.ps1`).

## 5. Post-release notes

- The three live providers hit unofficial endpoints; the contract — error
  card, never a crash — remains test-covered (Rust wire-format and
  schema-drift assertions; frontend per-adapter error containment).
- `docs/security-and-release-review.md` finding F3 is now **closed** by the
  HTTPS pinning in this release.
