# Security & Release Review — Rate Limits (Token_Monitor)

**Date:** 2026-09-27 · **Scope:** full static inspection + local test run · **Verdict: PASS to release** (no code changes required; two documentation corrections applied in this pass, three optional hardening notes below)

## 1. What was verified, and how

| Check | Result |
| --- | --- |
| Rust test suite (`cargo test`) | **57 passed, 0 failed**, 4 live-network tests correctly gated behind `#[ignore]` |
| Frontend (`tsc --noEmit` + `vite build`) | Clean typecheck; production bundle builds (154 kB JS / 4.3 kB CSS) |
| Secret scan (sources, configs, docs, scripts) | No hardcoded keys/tokens/JWTs — only synthetic test fixtures |
| Credential-boundary claims in the README | Verified line-by-line against `codex.rs`, `zai.rs`, `opencode_go.rs`, `antigravity.rs` |
| Tray / single-instance / autostart lifecycle | Verified against `main.rs` and `useSettings.ts` / `useProviderUsage.ts` |

Everything below cites the code that backs it.

## 2. Credential boundaries — PASS

The app's core security claim — *credentials stay in the Rust process; the WebView only ever receives normalized windows* — holds for all four backends.

| Provider | Local source (read-only) | Endpoint the credential goes to | Returns to WebView |
| --- | --- | --- | --- |
| Codex | `~/.codex/auth.json` (access token + account id) | `chatgpt.com/backend-api/wham/usage` | windows + plan name |
| Z.ai | `ZAI_API_KEY` env → `~/.zcode/v2/config.json` → `enc:v1` entries in `credentials.json` (decrypted in-process) | `api.z.ai/api/monitor/usage/quota/limit` | windows only |
| OpenCode Go | `~/.local/share/opencode/auth.json` (`opencode-go.key`) | `opencode.ai/zen/go/v1/usage` | windows only |
| Antigravity | `~/.config/opencode/antigravity-accounts.json` (quota cache) | none — passive file read | windows only |

Mechanisms that hold the boundary (each independently tested):

- **Wire-format assertions**: `zai.rs` (`wire_format_is_camel_case_and_never_contains_the_key`, `key_debug_never_leaks_the_secret`), `opencode_go.rs:558`, `antigravity.rs:648` (asserts refresh token, session token, email, and the literal string `fingerprint` never appear on the wire).
- **Redacted `Debug` impls** on every secret holder (`CodexAuth` codex.rs:164, `ZaiKey` zai.rs:366, `OpenCodeGoKey` opencode_go.rs:225) so a stray `{:?}` cannot leak material.
- **No secret in error paths**: every error constructor embeds only codes, HTTP statuses, and upstream `msg` strings; file contents and keys are never formatted into messages. The frontend logs only these sanitized messages (`console.warn`).
- **Host pinning for Z.ai keys** (`provider_base_url_is_zai`, zai.rs:434): only providers whose base URL is `api.z.ai` qualify; the `builtin:zai-start-plan` session token (zcode.z.ai) and bigmodel keys can never be sent to api.z.ai. Credential-store fallback additionally requires the entry name to match `*coding-plan*zai-:api-key` (zai.rs:518).
- **Read-only everywhere**: the only filesystem access in the codebase is `fs::read_to_string`. The app never writes credential files and never refreshes OAuth tokens (Codex refresh-token safety is honored, as the README promises).

## 3. Threat model (stated explicitly)

This is a per-user Windows tool. Its trust boundary is the user's own profile: it reads files any same-user process can already read and adds no privilege. Two consequences worth keeping in docs (not code):

- The ZCode `enc:v1` fallback secret is **derived, not user-chosen** (`zcode-credential-fallback:{platform}:{home}:{username}`, zai.rs:323). That is the ZCode app's own scheme — machine-local obfuscation, not a secret. Decrypting it here grants nothing a same-user process doesn't already have. No action needed; just don't advertise it as stronger than file ACLs.
- The plaintext mirror in `config.json` is read **first** by design (the monitor endpoint accepts that key — live-verified, per the module header), so the strongest statement is: keys leave the machine only toward the four provider endpoints listed above, and nothing else is ever transmitted (no telemetry, no crash reporting, no updates).

## 4. Provider failure isolation — PASS

- Each TypeScript adapter (`codexProvider.ts` and siblings) catches its own invoke errors, keeps the last good data of the session (`status: "error"` + reason on the card), and **resolves even on failure**. `fetchAllUsage()` therefore never rejects, and one broken provider cannot blank the dashboard or stall `Promise.all`.
- The Rust Z.ai backend distinguishes credential rejections (`auth_invalid` / `not_entitled` → try next candidate key) from hard failures (network/schema → abort immediately) — zai.rs:626-633.
- The Z.ai endpoint's quirk of answering auth failures with **HTTP 200 + `{code: 401}`** is explicitly handled (`envelope_error`) and test-covered.
- Schema drift is treated as an error, not guessed at: empty/unknown response shapes map to `unexpected_response`; implausible `nextResetTime` magnitudes are dropped (zai.rs:210); percentages are clamped 0–100 on both sides of the IPC boundary; malformed reset timestamps drop the timestamp but keep the window.
- `unknown` vs `error` vs `ok` card states are driven off real data presence — no invented windows anywhere (enforced per-module and asserted in tests).

## 5. Lifecycle & window behavior — PASS

- `single-instance` is registered **first** (main.rs:46), and its callback correctly does not pop the window on an autostart relaunch (`--hidden` argv check, main.rs:49).
- Close-to-tray intercepts `CloseRequested` → `hide()` + `prevent_close()` (main.rs:107); Quit is the only exit path.
- Autostart preference is reconciled against the OS Run key at every launch (`useSettings`), with rollback on plugin error — matches the README's limitations section (dev builds register their own path; absolute exe path after moving).
- Re-open refresh: `visibilitychange` triggers a refresh when data is older than 60 s; the stale indicator fires at 2× the configured interval; tray refresh uses an event (`tray://refresh`) — all three paths funnel through one guarded `refresh()` with an in-flight lock (no overlapping fetches).

## 6. Packaging & app surface — PASS

- Versions consistent across `package.json`, `src-tauri/Cargo.toml`, `tauri.conf.json` (0.1.0).
- Tauri 2 security config is tight: CSP set (`default-src 'self'`, WebView connect only to `ipc:` / `ipc.localhost` — no remote origins), `withGlobalTauri` not enabled, capabilities grant only `core:default` + `autostart:default` scoped to the single `main` window. The webview has no shell, fs, or http capabilities — HTTP lives in Rust only.
- Release profile: `panic="abort"`, `strip`, `opt-level="s"`; single NSIS target; HKCU autostart matches the no-admin claim.
- `scripts/*.ps1` are window-scoped dev helpers (find window by title, click/screenshot/WM_CLOSE) — no global hooks, no credential access; not shipped in the bundle.
- Housekeeping (cosmetic): `final-shot.png` sits at the repo root and isn't matched by the `shot-*.png` ignore pattern.

## 7. Findings

| # | Severity | Finding | Status |
| --- | --- | --- | --- |
| F1 | P2 (docs) | README's Z.ai section described the old key resolution (credential store only, team-key fallback). The code resolves `ZAI_API_KEY` → enabled `api.z.ai` providers in `config.json` (coding-plan first) → `enc:v1` credential-store decryption as last resort, and loops **all** candidates. | **Fixed in this pass** (README updated) |
| F2 | P2 (docs) | Antigravity is half-integrated: `src-tauri/src/antigravity.rs` (command defined, deliberately not registered) and `src/providers/antigravityProvider.ts` (implemented, not in the registry) — both inert, so no runtime bug. Neither appeared in the README architecture map, and nothing warned that the backend command must be registered **before** the adapter joins the registry (frontend-first would produce "command not found" on every refresh). | **Documented in this pass**; integration checklist added to the README |
| F3 | P3 (hardening, optional) | `provider_base_url_is_zai` compares the host only and ignores the scheme; a `http://api.z.ai` entry in the user's own config would pass. Requiring `https` would close a downgrade path the user would have to misconfigure first. | **Fixed in v0.2.0** (scheme pinned to `https`; test `provider_base_url_requires_https_scheme_on_api_z_ai`) |
| F4 | P3 (docs) | `docs/provider-discovery.md` (endpoint evidence, per-provider risk ratings) was not linked from the README. | **Fixed in this pass** |
| F5 | P3 | Codex fast-fail on JWT `exp` uses the local clock; heavy clock skew could reject a token the backend would accept. The backend check remains authoritative, so impact is a spurious `auth_expired` card state until a successful refresh. | Acceptable; no change |
| F6 | Info | Dev and installed builds share one single-instance identifier (`com.ratelimits.desktop`) — already documented in the README's limitations; keeping it listed here so release QA checks the *installed* build, not the dev build. | Documented |

## 8. Release checklist (for `npm run tauri build`)

> The v0.1.0 checklist below is superseded by [`release-checklist-v0.2.0.md`](release-checklist-v0.2.0.md).

1. Build the **installer** artifact and smoke-test that one (`src-tauri/target/release/bundle/nsis/Rate Limits_0.1.0_x64-setup.exe`), not the dev build — per F6.
2. Confirm the dashboard subtitle ("Codex, Z.ai & OpenCode Go live · others mocked") still matches the registry on the day of release.
3. After enabling autostart from an installer build, verify the HKCU Run entry points at the installed exe (the plugin writes an absolute path).
4. The three live providers hit unofficial endpoints (Z.ai monitor, OpenCode Go usage) and Codex's semi-private usage backend — any of them may change shape without notice; the app's contract is to show an error card, never crash. That contract is test-covered.
