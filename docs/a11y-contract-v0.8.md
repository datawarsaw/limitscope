# LimitScope v0.8 — Accessibility & Desktop Ergonomics Contract (Lane C)

- **Status:** Lane C implementation checklist. Enumerates every automated DOM/CSS assertion this lane added (reproducible via `npm test`) and every HUMAN_REQUIRED native row that the RC gate must record. The historical "22/22 browser-contract checks" was a recorded result without an artifact; this file is the artifact, so the v0.8 RC records reproducible counts instead of a bare number.
- **Scope discipline:** A02's component-level halves (pinned persistence, Escape dismissal + suppression) live in `PredictionBlocks.tsx` — single-owner Lane A. Lane C landed only the parallel-safe CSS slice and defers the rest to the A02 sequencing point. No other finding in this file is deferred.

## 1. Automated contract (vitest, all green at lane head)

| # | Requirement | Finding | Pinned by |
|---|---|---|---|
| A1 | Work-area clamp never enlarges; shrinks/repositions only on violation; supports negative coordinates and degenerate-input no-ops | W01 | `src/lib/windowBounds.test.ts` (14 cases incl. 200% DPI shrink, stranded window, monitor-loss fallback) |
| A2 | Main-window clamp applies at attach and re-checks on scale change, resize, and focus regain; converts outer intent through the decoration delta; falls back to the primary monitor | W01 | `src/lib/mainWindowBounds.test.ts` (8 cases) |
| A3 | Settings drawer yields space instead of pushing the utility bar off-viewport: `flex: 0 1 auto; min-height: 0; overflow-y: auto` on the drawer, `flex: none` retained on the utility bar | W02 | `src/dashboardStyles.test.ts` |
| A4 | Prediction tooltip stays hit-testable (`pointer-events: auto`) while revealed; hover/focus reveal selectors retained | A02 (parallel-safe slice) | `src/dashboardStyles.test.ts` |
| A5 | Drawer opens with focus on the first setting (explicit entry, nonmodal — no trap, no `aria-modal`) | A01/K02 | `src/App.settingsDrawer.test.tsx` |
| A6 | Escape inside the drawer closes it and restores focus to the Settings invoker; closing never strands focus on a detached node | A01/K02 | `src/App.settingsDrawer.test.tsx` |
| A7 | Escape outside the drawer content leaves it open (component-level handler, no global listener) | A01/K02 | `src/App.settingsDrawer.test.tsx` |
| A8 | Floating self-hide announces `floating://visible-changed(false)`; tray Show/Hide listener pair flips the persisted `visible` pref symmetrically; attach and close-request announce state | A10 | `src/lib/floatingWindowChrome.test.ts` |
| A9 | Self-dismissal of the floating bar restores focus to the main window only when main is visible; hidden/missing main never steals focus | A09 | `src/lib/floatingWindowChrome.test.ts` |
| A10 | Overlay shift contains the floating bar on both axes (horizontal + vertical, 8px bottom gap retained) and restores the parked position | A11/W01 | `src/lib/floatingWindowChrome.test.ts` |
| A11 | Tray label is a strict Show/Hide pair distinct from Quit (`"Show/Hide floating quota bar"`), action decided from real window visibility, event names shared with the webview listeners (`tray://show-floating`, `tray://hide-floating`, `floating://visible-changed`) | A10/T01 | `src-tauri/src/main.rs` `#[cfg(test)]` (4 cases via `cargo test --locked`) |
| A12 | Full cross-cutting gate: `npm test` (655), `cargo test --locked`, `npm run build`, `npm run scan:secrets`, guardrails (`runtimeTriggerGuardrail`, `localDataBoundary`, `brand`) | lane gate | CI/local run at lane head |

## 2. HUMAN_REQUIRED native rows (record at RC)

| # | Journey | Finding | Gate notes |
|---|---|---|---|
| H1 | DPI matrix 100/125/150/175/200% × 1080p/768p, settings open/closed: main window never exceeds the work area at launch, after DPI change, and after monitor removal | W01 | 768p @ 200% leaves ~683×364 logical — the 420 min height cannot fit; the clamp floors at the OS min and the residual overflow is accepted and recorded |
| H2 | 200% text enlargement: drawer, dashboard commands, and utility bar remain reachable (drawer self-scrolls) | W02 | research gate W02 |
| H3 | Tray keyboard access with main hidden: menu exposes Open / Show-Hide floating / Refresh / Quit; Hide hides (never quits); Refresh does not activate main and respects runtime cooldowns | A10/T01 | research gate T01 |
| H4 | WebView2 keyboard journeys: Tab into drawer, Escape restore to invoker, focus never stranded on a detached node after close | A01/K02 | research checklist K01/K02 |
| H5 | Pointer travel into a visible prediction tooltip keeps it open (real WebView2 hover semantics; jsdom has no layout) | A02/K04 | parallel-safe slice only |
| H6 | Floating bar Escape-with-quick-view still restores its invoker; menu-only dismissal leaves focus on the prior foreground when main is hidden | A09 | native focus verification |
| H7 | Native acceptance harness tray-Hide flow | A10 | `scripts/native-acceptance.ps1` extension is an integration-lane item per plan §6.2; the 18 existing flows must stay green |

## 3. Deferred (Lane A ownership)

- **A02/K04 remaining halves:** click/tap pinning (persistence while reading), Escape dismissal with reopening suppression, and the controlled reveal state that replaces the pure-CSS hover/focus reveal — all require `src/components/dashboard/PredictionBlocks.tsx`, which is single-owner Lane A. Lane C's CSS slice (`pointer-events: auto`) is compatible with that controlled reveal and is pinned by A4.
