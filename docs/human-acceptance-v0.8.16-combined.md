# LimitScope v0.8.16 Combined — Native Windows Human Acceptance

Manual checklist for the combined Phase A + B + C + D integration candidate.

## 1. Candidate

| Item | Value |
|---|---|
| Branch | `codex/v0.8.16-combined-integration` |
| Commit | `030dde91e10b7a22c9f57e92240520bd70297804` |
| Tree | `cf04545c7e1351e3ba4e27fcc1734df500ac31fb` |
| Base | `9ac6fe7fb6fb7ccebd081d79106d8e175e9a2110` (published v0.8.15) |
| Worktree | `C:/AI/limitscope-v0816-integration` |
| Executable | `C:/AI/limitscope-v0816-integration/src-tauri/target/release/LimitScope.exe` |
| Installer | `C:/AI/limitscope-v0816-integration/src-tauri/target/release/bundle/nsis/LimitScope_0.8.15_x64-setup.exe` |

## 2. Before you start

The **installed production app is currently running**
(`C:\Users\micha\AppData\Local\Rate Limits\LimitScope.exe`). LimitScope
uses a single-instance guard keyed on the app identifier
(`com.ratelimits.desktop-sim`), so a second instance of the SAME identifier
would hand off to the running one and exit.

- [ ] Quit the running production app from the tray (right-click → Quit).
- [ ] Run the candidate **standalone** — double-click
      `src-tauri/target/release/LimitScope.exe`. Do NOT run the installer
      unless you intend to replace the installed production build.
- [ ] Confirm the process path is the candidate path, not the installed one:
      `Get-Process LimitScope | Select-Object Path`

The candidate reuses the installed app's data directory
(`%LOCALAPPDATA%\com.ratelimits.desktop`), so your existing credentials and
settings carry over. That is what makes the authenticated checks below
possible.

## 3. Grok visibility (Phase A)

- [ ] Open the floating bar's **Grok** card. It shows the plan label
      (`Grok Bot · X Premium+`), the remaining percentage and the reset —
      read passively from the runtime, with no manual refresh.
- [ ] The floating Grok card and the main window's Grok card show the SAME
      percentage.
- [ ] The Grok segment's own number is still the xAI weekly-credit window,
      not the Grok Bot pool. The two are never summed.
- [ ] With no observation, the card shows the manual-read guidance and the
      Refresh action still works.

## 4. Resting dock width and wallet dot (Phase A + C)

- [ ] Dock the bar to the top edge with three visible providers. The resting
      strip measures 400×16 logical px at 100% scaling — no empty width on
      the right.
- [ ] The strip contains exactly three 3px meters and two separators.
- [ ] Hovering reveals a 400×64 bar. The top edge stays fixed and the
      horizontal centre does not move. No meter slides horizontally.
- [ ] With five visible providers, resting and revealed are both 600 px.
- [ ] With a wallet present, resting and revealed are both 646 px.
- [ ] Hide a provider in Settings while resting: the native window and the
      strip shrink together (400 → 300 for 3 → 2) and the meters reflow.
      Show it again and the strip returns to the wider footprint.
- [ ] The resting strip shows **one** wallet status dot, in the same slot the
      revealed bar's wallet occupies — to the right of the last provider.
- [ ] The dot's colour matches the revealed slot for the same state: live
      green, stale/cooldown amber, error red, unavailable muted.
- [ ] The dot NEVER shows a percentage, a filled meter or a quota colour.
- [ ] With no wallet, the strip shows no dot.

## 5. DeepSeek / OpenRouter shared popover (Phase C)

- [ ] The floating bar carries exactly ONE wallet slot, to the right of the
      providers — not one segment per wallet.
- [ ] Clicking it opens the shared **API Wallets** card listing both
      OpenRouter and DeepSeek.
- [ ] The OpenRouter row shows `USD 12.50` style — the amount with its own
      currency named.
- [ ] The DeepSeek row shows `CNY 110.00` — never `$110.00`.
- [ ] There is NO combined total across currencies. `122.50` must not appear.
- [ ] The card's Refresh action works and focuses correctly; Escape closes
      it and returns focus to the slot.

## 6. MiniMax segment and quota windows (Phase D)

- [ ] The main window lists a **MiniMax** provider row.
- [ ] MiniMax shows TWO windows: `5-hour` and `Weekly`.
- [ ] The MiniMax segment appears in the floating bar alongside the other
      providers.
- [ ] The floating MiniMax card shows both windows with the correct resets.
- [ ] The weekly window is the floating bar's primary window for MiniMax
      (`Weekly` is the designated long-term window).

## 7. Three-monitor sleep / wake recovery (Phase B)

Requires a real three-monitor machine.

- [ ] With the bar docked or placed on a specific monitor, sleep the display
      and wake it. The bar returns to its intended monitor and position.
- [ ] Disconnect one monitor. The bar does not strand on the wrong one; it
      falls back to a live monitor without overwriting the saved placement.
- [ ] Reconnect the monitor. The bar returns to the monitor it was on before
      the disconnection.
- [ ] Unplug and replug monitors in a staggered order (they reconnect at
      different moments). The bar settles in one place and does not flicker
      between monitors.
- [ ] While DRAGGING the bar, disconnect a monitor. The recovery does not
      fight the gesture; the bar stays where you put it.

## 8. Locate Floating Bar tray action (Phase B)

- [ ] Right-click the tray icon. The menu contains **Locate Floating Bar**,
      distinct from the Show/Hide floating toggle.
- [ ] With the bar hidden or stranded off-screen, choosing **Locate Floating
      Bar** shows the existing floating window — no second window, no
      restart — and pulses it with a brief outline so you can see which of
      several monitors it is on.
- [ ] With reduced motion enabled in Windows, the pulse is suppressed but the
      bar still appears in the right place.

## 9. Preserved behavior

- [ ] Semantic meter colours, 3px meter height, separators, surface, radius
      and reveal timing are unchanged.
- [ ] Drag, drag-away undock, saved anchor, menu actions and reduced-motion
      behaviour are unchanged.
- [ ] The main dashboard's existing provider rows and quota windows are
      unchanged.
- [ ] No second window, no new layout system, no credential change.
- [ ] The installed production build is still present and unmodified after
      the acceptance run.

## 10. Out of scope for this acceptance

- The pending 7-day reset-horizon candidate — not in this build.
- OpenDesign CLI and JEV experiments — not in this build.
- Version bump, tag, publication, release or deployment.

## 11. Decision

Record the operator's outcome against commit
`030dde91e10b7a22c9f57e92240520bd70297804` only. Acceptance binds to this
exact SHA and does not transfer to a later one.

- [ ] PASS — accepted for the next release step.
- [ ] FIX REQUIRED — list the failing check above.

