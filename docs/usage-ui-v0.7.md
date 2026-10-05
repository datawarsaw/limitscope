# Usage UI (v0.7)

The v0.7 Usage view is LimitScope's first historical analytics surface. It
consumes the accepted backend contract unchanged
([usage-analytics-backend-v0.7.md](usage-analytics-backend-v0.7.md), branch
`feature/v0.7-usage-analytics-backend` @ `17f1984`) and adds no analytics
logic in React: every number on screen comes from one
`get_usage_analytics` IPC call, and presentation code only shapes values
into text, tones, and chart geometry.

Branch: `feature/v0.7-usage-ui` (worktree `C:\AI\Token_Monitor_v07_usage_ui`).

## Navigation

- The main window header carries the primary product navigation:
  **Overview | Usage** (a two-button segmented group, `aria-pressed` state,
  keyboard operable like any button).
- **Overview** is the unchanged v0.5 three-zone dashboard (provider rail ·
  primary overview + quota windows · needs-attention rail).
- **Usage** replaces the main column with the analytics surface. The
  needs-attention rail is Overview-only; the provider rail stays and becomes
  the Usage **provider scope** (see Filters).
- Providers and History are not top-level destinations. Provider selection
  remains the rail's job; Settings remains the utility-bar drawer.

## Layout

One coherent analytics surface, top to bottom:

1. Toolbar: `All providers` scope toggle, `24h | 7d` range group, `Window`
   select, and an `Updating…` status while a refetch is in flight.
2. Summary strip: the five research-proven metrics in one strip that wraps
   instead of exploding into cards.
3. Usage trend: small multiples, one chart per logical window
   `(providerId, account, windowLabel)`, bounded to the first six series in
   deterministic backend order with an honest "…and N more windows" line —
   never a spaghetti chart of every series.
4. Daily peak heatmap (see below) with a fixed-band legend.
5. A compact coverage line: observed days, not-observed span count, and the
   reminder that gaps are missing observations, never zero usage.

## Summary semantics

| Metric | Source | Presentation |
|---|---|---|
| Peak observed | `summary.peakObservedUsage` | `84%` (exact) or `84%+` with a visible `lower bound` badge |
| Most constrained | `summary.mostConstrainedWindow` | `Codex · Weekly credits`, `84% used`, derived `% left`, severity badge |
| Observed resets | `summary.observedResetCycles.count` | Labelled **Observed resets** (never "resets remaining/available"); the count is a lower bound and carries the badge |
| Observed days | `summary.observedDays.count` | `N of the last M days` (M = heatmap length) |
| Near limit | `summary.timeNearLimit` | `~1h 24m` + `est.` badge; breakdown lines `≥80% · 1h 24m`, `≥95% · 18m` |

Nothing is recomputed from trend arrays. A metric the backend omits (e.g.
near-limit time with no comparable intervals) renders as `—` with an honest
sub-line, never a zero.

## Exact vs lower-bound rendering

- `24h` peaks are **exact** relative to stored detailed observations; they
  show no qualifier.
- `7d` peaks are **lower bounds** (30-minute compaction can discard an
  unrepresentative peak); they render `84%+` plus the text `lower bound`.
- The same rule applies per heatmap day (`peakExactness`) and to the reset
  count (`exactness` field). A lower-bound value is never displayed as
  exact.

## Estimated near-limit time

The backend interpolates time spent at or above 80% / 95% only between
adjacent same-cycle observations whose interval is within the series' gap
threshold; it is always an estimate. The UI keeps the word **est.** beside
the value, shows the threshold breakdown, and never implies continuous
monitoring.

## Trend charts

- Native SVG, no chart dependency. Fixed 0–100 percent y-scale (a half-full
  chart always means 50% used) and the full query range as the x-domain, so
  small multiples are comparable.
- Points connect only while they share a `cycleId` and the interval is at or
  below the series' `gapThresholdMs`. Reset boundaries and gaps therefore
  break the line mechanically (`trendShape` in
  `src/lib/usagePresentation.ts`); nothing is smoothed through a reset.
- `resetBoundary` points render hollow diamond markers (shape + tooltip +
  counted in the summary — not color-only) using the backend's own cycle
  annotations.
- Not-observed gaps render as hatched floor spans labelled "Not observed"
  and are never zero-filled; `availabilityInference: "none"` is respected.
- A dashed guide marks the 80% near-limit threshold; explained in the chart
  legend, and edge time labels drop out at narrow widths
  (`data-layout="narrow"`).

## Heatmap

- One cell per local calendar day in the queried range; the only semantic is
  **daily peak observed `usedPercent`** after filters.
- Bands are the backend's fixed absolute ranges
  (`0–25% / 25–50% / 50–75% / 75–100%`) — presentation colors by the `band`
  field and never normalizes to the user's own maximum.
- An unobserved day is hatched with a dash glyph and labelled
  "not observed" (visible in tooltip, text in the accessible name); it is
  never drawn as a 0% day.

## Filters

- **Range**: `24h | 7d` (backend query parameter).
- **Provider**: the provider rail doubles as the scope — `All providers` by
  default, or a specific provider by selecting a rail entry. The scope is a
  backend query parameter, so summary, trends, and heatmap all respect it.
- **Window**: optional select fed from the runtime's own quota window labels
  for the scoped providers, passed through to the backend.
- No complex filter panel; the toolbar is one row that wraps.

## Loading, errors, empty states

- The analytics query runs only while the Usage view is open, keyed on the
  normalized query plus the runtime history revision — never per render.
  The provider quota runtime stays independent.
- A first load shows a quiet `Loading usage…` line; a refetch keeps the
  previous payload on screen behind an `Updating…` status.
- A failed analytics query shows a retryable error card inside the Usage
  view only. Live provider cards, the rail, and refresh are unaffected.
- Empty history: "No usage history yet. LimitScope will build this view as
  quota observations are recorded." — naming the provider when scoped.
  A provider/window with no observations in range says so explicitly; no
  fake chart points are ever rendered.

## Used / remaining

Canonical data stays `usedPercent` in used orientation everywhere. The
v0.6 quota-perspective contract is not part of this lineage, so there is no
perspective toggle; remaining capacity appears only as the derived text
`100 − used` (e.g. `84% used · 16% left`) and never mutates an analytics
value.

## Severity

Severity remains canonical on the **used** percentage: `95% used` in
remaining terms (`5% left`) is still **Critical**. Words (`High` ≥ 80%,
`Critical` ≥ 95%) accompany tone colors so severity is never color-only.

## Accessibility

- All filter controls are native buttons/selects: keyboard operable, with
  visible focus rings shared with the dashboard.
- The provider rail keeps its roving-tabindex tab semantics; under the
  Usage view's "All providers" scope the first entry stays a tab stop so
  the keyboard can always enter the rail.
- Every chart is `role="img"` with a spoken one-line summary (title, scale,
  peak, reset count, not-observed spans) mirrored by visible caption text.
- Heatmap cells carry accessible labels: date, peak percent, band range,
  lower-bound qualifier, or "not observed".
- Reset markers (diamond shape), severity (word badges), and exactness /
  estimated qualifiers (text) never rely on color alone.

## Dev-only visual fixtures

`src/dev/usageFixtures.ts` is a development-only harness: with `npm run
dev`, `?fixture=default|lower-bound|reset-boundary|gaps|empty` and
`?theme=graphite|glass|oled` render representative states for visual
acceptance. The module is dead-code-eliminated from production builds and
ships no analytics logic.

## Tests

`src/lib/usagePresentation.test.ts` pins the pure semantics (exactness,
durations, canonical severity, fixed bands, reset/gap segmentation, fixed
0–100 geometry, bounded small multiples). `src/components/usage/`
`UsageView.test.tsx` and `src/App.usageView.test.tsx` cover navigation,
queries, filters, labels, states, and failure isolation.
