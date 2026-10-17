# OpenDesign provider

OpenDesign uses the standard provider identity `opendesign`, appended after
`qwen-token-plan` in the runtime registry. The dashboard rail, quota details,
visibility/order preferences and floating bar consume the shared runtime
snapshot. The old supplementary dashboard card is no longer mounted.

## Source and refresh boundary

`src-tauri/src/opendesign.rs` remains the authoritative collector. Only an
explicit `refresh_opendesign_usage` action scans Chrome's accessibility tree.
The reader never navigates, focuses, reloads, types into or starts Chrome and
does not read credentials or call an OpenDesign API. The existing watchdog
and single-flight protection remain in place.

Automatic refresh was evaluated and remains unsupported: the collector depends
on the user keeping the authenticated Plan & balance page available, and a
runtime timer must not drive browser accessibility work. Ordinary provider
cycles only project the last manual result held in process memory. Both main
and floating details offer a manual Refresh and explain this condition.

## Independent quota windows

| Source window | Standard label | Percentage normalization | Reset presentation |
| --- | --- | --- | --- |
| `5-hour limit` | `5-hour` | `usedPercent = 100 - remainingPercent` | Source `resetText` |
| `7-day limit` | `7-day` | `usedPercent = 100 - remainingPercent` | Source `resetText` |

The complement adapts the collector to the existing shared quota contract;
the collector's own remaining values are unchanged. OpenDesign views display
remaining allowance. No values are averaged, combined or substituted. The
floating bar explicitly designates `7-day` as its long-term quota. The main
dashboard continues to feature the most depleted of the two windows.

`LimitWindow.resetText` is an additive optional presentation field. OpenDesign
never sets `resetAt`. Text such as `Resets in 39 min` is shown verbatim with
an `At observation:` prefix, so a cached countdown cannot look like a running
clock. Other providers retain their existing absolute-reset behavior.

## Missing, failed and cached observations

The collector accepts only a complete, unambiguous pair. Missing or duplicate
groups/bars, unreadable accessibility trees and timeouts never become 0%.
Chrome absent and dashboard absent remain typed outcomes; a logged-out or
unavailable Plan & balance page cannot produce a quota reading.

The last complete observation stays in memory with its original timestamp.
`checkedAt` and `sourceUpdatedAt` describe that observation, not later runtime
cycles. Failed manual reads preserve the pair, mark it stale/error, and keep
the failure visible. A new process requires a new manual read. Manual readings
are excluded from persisted provider cache, history/prediction and automatic
quota notifications; repeated projection is not new usage evidence.
