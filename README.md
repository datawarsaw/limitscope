# LimitScope

LimitScope is a Windows 11 tray app that brings AI provider quota windows into one compact, local dashboard.

> **Early access · Windows 11 · stable release v0.8.6**
>
> LimitScope was previously developed under working name **Rate Limits**; the technical package and crate name remains `rate-limits`.
>
> This checkout includes source changes made after the public `v0.8.6` release. The ZCode reset-card row below describes the current source and is not included in the `v0.8.6` installer.
>
> The current screenshots in `docs/` show older interfaces and provider states. A current screenshot will be added after visual acceptance; do not treat those images as a preview of this release.

## Why LimitScope

AI coding tools keep usage limits in different apps and local credential stores. LimitScope brings supported quota information together, shows when a value is live or cached, and keeps the dashboard available from the Windows system tray.

## Features

- Compact dashboard and optional floating quota window.
- Automatic and manual refresh, quota history, reset estimates, and notifications.
- Clear live, cached, stale, unavailable, and refresh-failed states.
- Local settings and data controls; no LimitScope telemetry or account service.
- Signed in-app updates distributed through a separate public release repository.

## Supported providers

| Provider | Data source | Integration | Notes |
| --- | --- | --- | --- |
| OpenAI / Codex | Existing local Codex login and OpenAI usage endpoint | Live | Usage and reset-credit observations come from the same response when available. |
| Z.ai | Existing local Z.ai coding-plan credentials and Z.ai quota endpoint | Live | Credentials are discovered from supported local stores. |
| OpenCode Go | Existing OpenCode or OpenCodex local credentials and OpenCode Go usage endpoint | Live | Reads the existing key; does not create or change it. |
| Google Antigravity | Google quota endpoint using the existing local login; local plugin cache as fallback | Live-first, cached fallback | Cache age is surfaced; stale cache is not represented as live data. |
| Grok (xAI) | Existing local Grok CLI or supported OpenCode credentials and xAI usage endpoint | Live | Refresh is cadence-limited and may serve a recent in-memory result. |
| ZCode reset cards | Existing local ZCode credentials and the read-only reset-status endpoint | Live, supplementary read | Independent of Z.ai quota success; does not spend or modify cards. |

Codex reset credits are parsed from the Codex usage response and are shown only when available and fresh. Both reset features are supplemental observations, not separate quota providers.

Claude is not a supported provider. The repository contains discovery work, but Claude is not registered in the application runtime. Provider endpoints and local storage formats are unofficial and may change; check the app's status and error details if a provider changes its interface.

## Privacy & security

- Provider credentials are discovered from supported local credential stores and used inside the Rust backend. The webview receives normalized quota data, not raw credentials.
- LimitScope does not upload credentials to a LimitScope-operated service. Provider requests go to the relevant provider; update checks go to GitHub's public release companion.
- LimitScope reads provider credentials and local provider caches. It does not create, rotate, refresh, or edit those credentials or provider-owned cache files.
- Credential-bearing requests refuse redirects for the adapters that enforce this policy. This is defense in depth, not a guarantee against every provider, operating-system, or network threat.
- Known credential values are scrubbed from provider error messages before they reach the interface. Diagnostics and exports apply additional redaction, but users should review an export before sharing it.
- LimitScope does not persist provider credentials. It does persist app-owned settings and quota/history data locally; these can be managed in Settings.
- Updates use Tauri's embedded minisign public key to verify the signed installer before installation. This is updater artifact verification, not a Windows Authenticode signature or a general security certification.

See [SECURITY.md](SECURITY.md) and the detailed [trust and verification notes](docs/trust.md).

## Installation

Download the latest stable Windows installer from the [LimitScope releases page](https://github.com/datawarsaw/limitscope-releases/releases/latest). LimitScope currently supports Windows 11. The in-app updater checks signed artifacts; the installer is minisign-signed and the app verifies its updater signature before installation. Windows SmartScreen reputation is separate from minisign verification.

## Updates

LimitScope checks a public, credential-free update feed hosted by [`datawarsaw/limitscope-releases`](https://github.com/datawarsaw/limitscope-releases). Startup checks are quiet. You can check manually in Settings. An update is downloaded and installed only after you choose **Update now**. The app verifies the signature against the public key embedded in the application before running the installer.

## Development

This is a Windows-first Tauri desktop app. You need Node.js 24, stable Rust with the MSVC target, Visual Studio C++ Build Tools, and the WebView2 runtime. See the [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/) if native build dependencies are missing.

```powershell
npm install
npm test
npm run build
npm run scan:secrets
```

Run Rust tests from `src-tauri`, then start the desktop app from the repository root:

```powershell
Set-Location src-tauri
cargo test --locked
Set-Location ..
npm run tauri dev
```

`npm run build` builds and type-checks the frontend. `npm run tauri build` creates the Windows installer. See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidance.

## Architecture

```text
React / WebView
      │ Tauri IPC
Tauri runtime (Rust)
      │
Provider adapters ── local credentials / provider endpoints
```

The frontend renders normalized provider data. Rust owns local credential discovery, provider requests, and app-owned persistence.

## Releases

The source repository contains the application and its build/release workflow. A signed CI release publishes only the installer, its minisign signature, and `latest.json` to the public [`limitscope-releases`](https://github.com/datawarsaw/limitscope-releases) companion. That repository is the updater distribution channel, not a source mirror.

Release candidates (versions such as `0.8.7-rc.1`) are published to the same companion repository as GitHub prereleases. Stable clients are never offered an RC, and an RC installer is downloaded manually from its release page.

GitHub Releases in the companion repository are the canonical release history. Browse [all releases](https://github.com/datawarsaw/limitscope-releases/releases).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, checks, and scope expectations.

## Security

Please report suspected vulnerabilities privately as described in [SECURITY.md](SECURITY.md). Do not post credential material or vulnerability details in a public issue.

## License

LimitScope is licensed under the [MIT License](LICENSE).
