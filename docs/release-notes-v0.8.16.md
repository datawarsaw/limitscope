# LimitScope v0.8.16 — Release Notes

This release ships the combined floating-experience, monitor-recovery,
DeepSeek-wallet and MiniMax-quota work as one stable release.

## Highlights

- **The floating bar recovers.** A native Windows display-topology observer
  (`src-tauri/src/floating_monitor.rs`) watches `WM_DISPLAYCHANGE`,
  `WM_DEVICECHANGE`, `WM_SETTINGCHANGE` and `WM_POWERBROADCAST`, so the bar
  returns to its intended monitor after a display sleep/wake and after a
  staggered multi-monitor reconnection. The saved placement now records the
  monitor identity, not just a point, and a fallback position can no longer
  overwrite the intended one.
- **The tray can find the bar.** A **Locate Floating Bar** action shows the
  existing floating window and pulses its outline, so a bar that is hidden or
  stranded off-screen is recoverable without a restart.
- **DeepSeek joins the shared API Wallets card.** The prepaid balance is read
  from the official balance endpoint (`GET https://api.deepseek.com/user/balance`)
  and shown in its own currency (CNY). Amounts are never summed, converted, or
  shown as USD when they are not.
- **MiniMax is a standard provider.** `minimax-token-plan` reads the official
  `GET https://www.minimax.io/v1/token_plan/remains` endpoint and reports the
  `5-hour` and `Weekly` windows, with `Weekly` as the floating bar's primary
  window. The upstream percentages are remaining, not used, and the adapter
  converts explicitly.
- **The resting dock keeps one footprint.** It takes the active bar's width,
  shows meters only, and adds a single wallet status DOT in the revealed bar's
  own wallet slot — a wallet is an absolute money balance, so it never gets a
  meter, a fill, or a percentage.
- **The Grok observation is shared.** The floating Grok card shows the same
  passive runtime observation as the main dashboard, so both surfaces
  describe one pool from one source. The xAI weekly-credit window is never
  summed with the Grok Bot pool.

## Operational conditions

The MiniMax credential is the MiniMax Code CLI's stored OAuth **access**
token, exchanged for the account's `sk-cp-` subscription key. The stored
refresh token is deliberately never used, because MiniMax rotates it on use
and presenting it would invalidate the CLI's own copy. When the access token
expires, LimitScope reports a deterministic `auth_expired` state and asks the
user to sign in again; it never takes over the session.

The DeepSeek balance requires a usable credential in the OpenCodex provider
config or the OpenCode auth file. When it is absent or the endpoint cannot be
reached, the wallet reports that state explicitly and shows no amount; it
never substitutes a stale or estimated figure.

The updater feed carries these notes after the signed release workflow verifies
the exact tagged source SHA.
