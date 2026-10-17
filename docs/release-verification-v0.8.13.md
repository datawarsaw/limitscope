# LimitScope v0.8.13 — Release Verification Record

Recorded on the v0.8.13 candidate tree that contains this file. The release SHA is that commit.

## Collection method under test

The OpenDesign card is an on-demand, read-only read of Chrome's Windows UI Automation tree. The backend enumerates Chrome top-level windows, confirms the process image is `chrome.exe`, and requires a document element with the OpenDesign dashboard name. Inside that document it locates the named groups `5-hour limit` and `7-day limit`, takes the single `ProgressBar` in each group as the measurement, and keeps the group's other text lines for the reset countdown. Ambiguity is rejected rather than resolved by position: a missing, duplicated, or unlabelled group answers `unknown` and produces no reading.

Acceptance rules exercised by `src-tauri/src/opendesign.rs`:

- Both windows must be present and labelled. A single-window or swapped answer is rejected, so a percentage can never migrate between allowances.
- The percentage is the page's remaining share, passed through unchanged apart from bounds checking. No used-percentage conversion exists on either side.
- Reset text is retained verbatim. No absolute reset time is computed from a relative countdown.
- Only a complete, successful pair replaces the in-memory last-known reading. A failure keeps the previous pair and its original observation stamp.
- The scan runs on a dedicated MTA thread with a 2,500 ms watchdog. There is no network access, no cookie or token read, no browser debug port, no focus change, no reload, and no input injection.

The command is `refresh_opendesign_usage`. It is invoked only by the panel's button; there is no timer, scheduler, or automatic retry.

## Live reads against the authenticated page

Chrome was signed in to the OpenDesign console with the Plan & balance page open. Each row is one explicit Refresh OpenDesign usage press in the running desktop app, read back through the app's own UI Automation tree so the displayed values are the ones a user sees.

| Local time (Europe/Warsaw) | 5-hour remaining | 5-hour reset line | 7-day remaining | 7-day reset line | Plan / period |
| --- | --- | --- | --- | --- | --- |
| 5:51 pm | 68.81% | Resets in 16 min | 89.39% | Resets in 6 days 14 hr | OpenDesign Go · Current period ends Nov 15, 2026 |
| 5:53 pm | 68.34% | Resets in 14 min | 89.24% | Resets in 6 days 14 hr | OpenDesign Go · Current period ends Nov 15, 2026 |
| 5:54 pm (Chrome minimized) | 68.29% | Resets in 13 min | 89.22% | Resets in 6 days 14 hr | OpenDesign Go · Current period ends Nov 15, 2026 |

Earlier probes on the same page recorded 70.99% / 90.12%, 70.93% / 90.10%, and 69.13% / 89.5% for the 5-hour and 7-day windows. Percentages move in the direction the page moves, the 7-day window decreases slowly, and each reset line stays with its own group. The backend scan itself completed in 75.9 ms on a successful probe.

No percentage swap was observed in any read, and the 5-hour countdown decreasing independently of the 7-day countdown is consistent with the page's own two windows.

## Behaviour under the tested conditions

| Condition | Observed result |
| --- | --- |
| Ordinary page reload, then Refresh | Fresh values returned in both rows (5:53 pm row above). No restart or re-login needed. |
| Chrome minimized | Read succeeded with both windows (5:54 pm row above). Visibility of the window on screen was not required. |
| Panel before any read | "No OpenDesign data yet — open Plan & balance in Chrome, then Refresh." No invented values. |
| Failed refresh after a good read | Covered by the typed failure tests: the retained pair and its original stamp survive, and the panel labels them Last known. |
| Chrome not running / dashboard not visible | Covered by typed status tests: distinct messages, no data rows. |
| Malformed `ok` answer | Rejected by `parseRefreshResult`; a success without both windows is not displayed as data. |

A live logout was deliberately not performed against the user's session. Expired-session handling is therefore evidenced by the typed failure and stale-reading tests rather than by a live logout.

## Regression scope

The existing six providers are unchanged in the live app: Codex, Z.ai, OpenCode Go, Antigravity, Grok, and Qwen remained present with their prior states, including the pre-existing Z.ai and OpenCode Go refresh failures. OpenDesign was not added to `production_specs`, so it is not a runtime provider and does not join the automatic scan or the provider rail.

## Automated validation

- `npm test`: 65 files, 1,105 tests passed.
- `cargo test --locked`: 834 passed, 15 ignored, 0 failed.
- `npm run build`: passed.
- `cargo check --locked`: passed with only pre-existing dead-code warnings.
- `rustfmt --check src-tauri/src/opendesign.rs`: clean. Repo-wide `cargo fmt --check` is not a usable gate on this tree: it reports drift in files this change never touched, including files at the unmodified base commit, because the installed rustfmt release reformats existing code. Those files were left alone.
- `npm run scan:secrets`: PASS (no credential-shaped material found).

## Evidence

The screenshot `docs/screenshots/v0.8.13/opendesign-live.png` was captured from the running desktop app with `PrintWindow`; it shows the OpenDesign Go card with both remaining rows, both reset lines, the period line, and the Updated stamp, alongside the unchanged provider rail.

## Limits of this record

The runtime verdict for the feature is CONDITIONAL: the read is reliable only while Chrome runs with an authenticated OpenDesign Plan & balance page open. That is a product condition of the UI approach, not a defect in this delivery, and the panel states it plainly when the condition is not met.
