# Quota Display Perspective Contract

## v0.7 Final Presentation Contract

The final quota-perspective refinement is presentation-only. The canonical
metric is always `usedPercent`; the selected perspective changes only the
display value, meter geometry, and accessible meter value at the final
presentation boundary.

| Canonical input | Used display / fill | Remaining display / fill |
| --- | --- | --- |
| 0% used | `0% used` / 0% | `100% remaining` / 100% |
| 20% used | `20% used` / 20% | `80% remaining` / 80% |
| 80% used | `80% used` / 80% | `20% remaining` / 20% |
| 95% used | `95% used` / 95% | `5% remaining` / 5% |
| 100% used | `100% used` / 100% | `0% remaining` / 0% |

The fill always runs left to right. Remaining mode represents available
capacity; it never reverses the physical bar direction. Severity is derived
from canonical `usedPercent` and remains explicit in copy: `Critical · 5%
remaining` is correct for a 95%-used quota. Near-limit and critical threshold
calculation remains `>=80% used` and `>=95% used`; any remaining equivalents
are presentation labels only.

The shared helper clamps finite presentation inputs safely to `0..100` while
preserving the finite source value separately. Nonnumeric or non-finite input
renders `Usage unavailable`, with no synthetic numeric meter value. Every
visible quota meter uses the helper's `meterPercent` and exposes an accessible
value that names the selected perspective, including the floating quick view.
Main provider cards, per-window rows, provider rail meters, floating compact
items, and floating hover/quick details share this contract. Attention is
text-only, tray has no quota presentation surface, and Usage has no quota
progress/fill element.

Heatmap bands, severity bands, threshold calculations, trend geometry, and all
stored history remain canonical on `usedPercent`. The heatmap may change its
explanatory labels in Remaining mode, but its colors and fixed used bands never
invert. Charts that do not explicitly present the selected perspective remain
unchanged; any display transformation is applied only at the final presentation
boundary.

## Scope

The v0.6 quota display preference selects one of two presentation modes:

- `Used` (default): show and fill the meter with canonical used capacity.
- `Remaining`: derive `100 - usedPercent` for display text and meter fill.

This is a presentation-only contract. Provider fetching and normalization,
runtime snapshots, persisted history, prediction calculations, notification
thresholds, reset timestamps, and cooldown behavior remain unchanged.

## Canonical Metric

`usedPercent` remains the sole canonical quota metric. It is the value stored
in history, fed to the prediction engine, and evaluated by severity and
notification policy. Remaining mode never persists an inverted percentage.

`src/lib/quotaPresentation.ts` is the single inversion boundary. Its primary
`quotaPresentation(usedPercent, perspective)` helper returns safe display text,
accessible labels, meter fill, and the displayed `aria-valuenow`. Main,
floating, tray, history/trend, and prediction adapters delegate to the same
derivation so later dashboard/tray lanes can consume the contract without
reimplementing it.

## Display And Severity

For canonical `usedPercent = 20`:

- Used: `20% used`, meter fill 20%.
- Remaining: `80% remaining`, meter fill 80%.

Severity is always evaluated from canonical `usedPercent`. A canonical value
of 95% remains critical in Remaining mode while its factual copy reads `5%
remaining`. The meter, accessible value, and text use the same displayed
metric; severity is conveyed separately through tone and explicit attention
wording.

Malformed or non-finite values render as `Usage unavailable`, omit a numeric
`aria-valuenow`, and never produce a synthetic quota number. Finite
out-of-range defensive inputs clamp only the presentation value to `0..100`;
the source value remains separate and is never fed back into product logic.

## Reset, Prediction, And History

Reset timestamps and countdowns do not depend on quota perspective. Copy stays
factual (`Resets in 2d`); it never claims capacity is available for the full
reset duration.

Prediction math remains canonical. When a projection exists, Used mode shows
`Projected at reset: 74% used`; Remaining mode may exactly derive `Projected
remaining at reset: 26%`. Burn rate remains canonical because inverting a rate
of consumption would be misleading.

History and trend data remain stored and computed as `usedPercent`. A
Remaining-mode chart may plot `100 - usedPercent` on a 0-100% axis as a display
transformation, but observations are never rewritten or mutated.
