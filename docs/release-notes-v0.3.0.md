# Rate Limits v0.3.0

This release candidate adds the frozen v0.3 scope to the released v0.2.1 baseline.

## Included

- Graphite, Glass, and OLED themes with persisted selection. Glass is a frontend treatment with an opaque fallback; the native window remains opaque.
- Local reset countdowns with absolute reset times, expired-timestamp handling, and a 30-second clock tick that does not refetch providers.
- Bounded local quota history with 24-hour retention, 500 newest observations per provider/window, duplicate collapse, corruption recovery, and a clean local clear action.
- Deterministic quota projection from current reset-cycle observations.
- Prediction visibility limited to medium/high confidence, with projected-at-reset, burn rate, optional likely-exhaustion, confidence, and current-cycle basis.
- Honest Live, Cached, Stale, Refresh failed, Mock, and global Refresh overdue presentation. Failed refreshes retain last-good windows with their observation time; cached and stale data never presents as live.

## Privacy and boundaries

History stores only `providerId`, `windowLabel`, `usedPercent`, `observedAt`, and optional `resetAt`. It contains no credentials, account identity, prompts, responses, model names, prices, or cost data. Prediction and history are local-only and best-effort: storage failure never blocks provider refresh, tray behavior, or last-good rendering. Simulated providers are excluded from history and prediction.

## Not included

Claude and Grok remain mock-only. This release has no Claude live integration, Grok live integration, native Mica/Acrylic, cost accounting, long-term analytics, cloud sync, ML forecasting, or provider-framework rewrite.

## Validation

- `npm ci`: passed.
- `npm test`: 165 tests passed across 10 files.
- `npm run build`: passed (`tsc` and Vite production build).
- `cargo test --locked`: 81 passed, 4 ignored live/credential tests, 0 failed.
- `npm run tauri build`: passed; produced `Rate Limits_0.3.0_x64-setup.exe`.
- Local installer inspection: NSIS bundle is a PE executable and reports product/file version `0.3.0`.

The Windows desktop smoke matrix remains environment-blocked because this runtime has no native Computer Use surface for launching or inspecting the app, tray, or installer UI. No smoke result is claimed beyond the executable/build and structural artifact checks.
