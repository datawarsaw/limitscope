# Wallet configuration (v0.8.17 feature branch)

Per-wallet configuration for the prepaid API wallets (OpenRouter, DeepSeek,
and the registered OpenAI placeholder): two independent controls per wallet,
available in the main window's settings drawer.

## The two controls

**Monitor wallet** — whether the runtime collects the wallet at all.

- Off means no automatic polling and no API request for that wallet: no job
  is spawned in the runtime cycle, on any trigger (interval, wake,
  reconnect, reset-wake, or a manual refresh).
- Disabling monitoring never deletes data. The wallet's last known balance
  stays in the persisted last-good store and keeps its original observation
  stamp; history is untouched (wallet balances never enter history — pinned
  by `a_wallet_balance_never_enters_history_or_notifications`).
- Turning monitoring back on resumes collection with the ordinary cadence:
  the next cycle collects the wallet like any other.

**Show in Floating Bar** — whether the wallet appears in the floating API
Wallets card. Presentation only, exactly like provider hiding: a hidden
wallet keeps being monitored, and a monitored wallet stays displayable with
no other configuration change. If every wallet is hidden, the shared API
Wallets slot drops out of the floating bar and the bar's width math sizes
the window to the providers alone.

A wallet whose monitoring is off but which is still visible presents as
**paused**: a muted dot (never the live pulse), a "Monitoring off" status
chip, and its last known balance with a "Last known <time>" line. No manual
refresh is offered for a paused wallet, because a manual read is collection
too.

## Where the settings live

The canonical values live in the WebView settings lane
(`rate-limits.settings.v1`, `walletPreferences: { monitored, floatingBar }`)
and are mirrored into a persisted Rust store (`wallet-preferences-v1.json`
in the app-data directory) on attach and on every change — the same pattern
as the notification lane's enabled flag. The mirror exists because the
runtime's first collection cycle starts before any webview attaches: the
store is loaded in `runtime::start()` before the scheduler spawns, so a
wallet the user turned off is not polled at startup either.

An id with no stored entry falls back to the registry's per-wallet default
(`WalletSpec::monitored_by_default`); ids the registry does not know are
ignored at the gate. Unknown fields from other lanes survive settings
writes, per the v0.6 settings contract.

## Defaults

- OpenRouter and DeepSeek: monitored and visible. Existing users and new
  installs keep today's behavior unchanged.
- OpenAI: **registered as a disabled-by-default placeholder, not a
  collector.** The public OpenAI API documents organization usage and cost
  endpoints, but no prepaid remaining-credit balance endpoint. Reading a
  remaining credit from undocumented billing routes, browser sessions, or
  page scraping is out of bounds, and an estimate must never be presented
  as a verified balance. The placeholder's fetch is deliberately offline
  and fails closed with a source-absent failure if it is ever invoked; its
  settings row shows both switches disabled with an explanation instead of
  offering a toggle that cannot work. A future documented balance endpoint
  can replace the stub without touching the cycle machinery.

## Runtime wiring (for reviewers)

- Gate: `RuntimeCore::wallet_monitored` (`src-tauri/src/runtime.rs`), applied
  in `run_cycle` where wallet jobs are spawned and in the lost-task
  containment loop (a configuration skip is never projected as a failure;
  the wallet's snapshot entry is carried forward frozen).
- Store: `src-tauri/src/wallet_prefs.rs` (temp-then-rename writes, tolerant
  parse, corrupt file → per-spec defaults).
- Command: `runtime::set_wallet_monitoring` — the settings lane pushes the
  whole monitor map; resolution to known ids happens in the runtime.
- Presentation: `src/lib/walletPreferences.ts` (defaults, sanitize, pure
  updates), `floatingWalletItems(wallets, prefs)` in `src/lib/floatingQuota.ts`
  (paused mapping), and the "API Wallets" group of the Settings page's
  Providers & Wallets section in `src/components/settings/SettingsView.tsx`
  (moved there from the former settings drawer by the v0.9 Usage-first
  main-window architecture; App.tsx still wires the wallets and the two
  preference setters into the page).

## Wallet card conventions (shared with the subscription cards)

The wallet card's observation line uses the subscription cards' convention:
the wallet's own last successful observation stamp, absolute and local
("Checked 10:44 pm"), muted and not emphasized (`formatTime`, no bold). A
retained last-good reading reads "Last known <time>". A fresh wallet carries
no "Live" chip — its stamp already says the reading is current. Stale,
cooldown, rate-limit, refresh-failure, and unavailable states keep their
chips and hints; the monetary balance stays the prominent figure and the
masked credential identity stays the muted footer line.
