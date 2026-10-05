# Contributing

LimitScope is a Windows-first Tauri desktop app. Changes should stay focused and preserve its local-first, read-only treatment of provider credentials and data.

## Prerequisites

- Windows 11, Node.js 24, and npm.
- Stable Rust with the MSVC toolchain.
- Visual Studio C++ Build Tools and WebView2 runtime.

## Local development and checks

From the repository root:

```powershell
npm install
npm test
npm run build
npm run scan:secrets
```

Run Rust tests and launch the desktop app:

```powershell
Set-Location src-tauri
cargo test --locked
Set-Location ..
npm run tauri dev
```

Build the Windows installer with `npm run tauri build`.

## Scope and review

- Do not commit credentials, tokens, private signing keys, or real account data. Use synthetic fixtures.
- Provider integrations should remain bounded and read-only. Do not write, rotate, or refresh provider-owned credentials or stores unless a separately reviewed feature explicitly requires it.
- Keep credentials in the Rust backend; do not expose raw credential values to the webview, logs, diagnostics, or fixtures.
- Avoid unrelated product and dependency changes in documentation or provider work.
- For UI changes, run the app and visually verify the affected states and window sizes. Describe what you checked in the pull request.
- Explain the user-visible effect, privacy/security implications, and relevant validation in your pull request.
