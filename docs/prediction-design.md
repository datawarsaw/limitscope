# Quota Prediction Engine — Design Notes (Prototype)

Whether useful projections can be derived from historical percentage snapshots,
and what the smallest honest algorithm for that looks like.

- Date: 2026-09-27
- Branch: `feature/quota-prediction-engine`
- Created: `src/lib/prediction/{types.ts,engine.ts,engine.test.ts}` and this file
- Engine boundary: no UI, provider adapters, Rust code, or storage live in the engine module.
- Verify with: `npx vitest run src/lib/prediction` (32 tests)

The v0.3 release-candidate integration keeps that pure boundary and connects the
engine to the app through `src/hooks/useQuotaPredictions.ts`, using the bounded
local store in `src/lib/quotaHistory.ts`. The notes below record the prototype
experiment; they do not describe the integrated lifecycle as still unwired.

The question this answers is narrow: given a series of `{usedPercent, observedAt,
resetAt}` samples for one quota window, can we say where the window is heading
without lying about how sure we are? The answer is yes for short windows with a
known reset, and the interesting part is the uncertainty handling, not the math.

---

## 1. Scope and non-goals

In scope: a pure function from snapshots to an estimate, deterministic
arithmetic, explicit reset handling, explicit confidence rules, unit tests.

Not in scope, on purpose: persistence, app wiring, provider changes, machine
learning, and any statistical claim beyond "a straight line fits these samples".
The engine never reads the wall clock — `now` is a required argument — so every
result is reproducible from its inputs alone.

## 2. Data model

An observation is deliberately flat, because it may come from a `LimitWindow`, a
cached snapshot, or a test fixture:

```ts
type QuotaObservation = {
  providerId: string;
  windowLabel: string;
  usedPercent: number;   // clamped to 0–100 on ingest
  observedAt: string;    // RFC-3339
  resetAt?: string;      // RFC-3339, optional
};
```

`predictWindow` returns the requested shape plus two diagnostics:

```ts
type QuotaPrediction = {
  providerId: string;
  windowLabel: string;
  burnRatePerHour?: number;          // percent points per hour, never negative
  projectedPercentAtReset?: number;  // clamped to 0–100
  estimatedExhaustionAt?: string;
  willExhaustBeforeReset?: boolean;
  confidence: "insufficient" | "low" | "medium" | "high";
  risk?: "low" | "medium" | "high";
  basis: PredictionBasis;            // which samples produced this
};
```

`basis` carries the segment id, sample counts, fit span, mean gap, staleness,
and reset bookkeeping. It exists so a caller — or a test — can explain a
prediction instead of trusting it, and so the UI could show "from 12 samples
over 2h" rather than an unexplained number.

Percent points per hour is the natural unit here: it does not require knowing how
long the window is, so the same code works for a 5-hour window and a weekly one.

## 3. Algorithm

1. **Filter and normalize.** Keep only observations matching the requested
   provider and window label. Drop non-finite percentages, unparseable
   `observedAt`, blank labels, samples older than `maxSampleAgeMs` (24h), and
   samples stamped more than `clockSkewToleranceMs` (5 min) in the future.
   Clamp percentages to 0–100. Sort by time; for equal timestamps keep the last.
2. **Segment into reset cycles.** A new segment starts when either the
   percentage dropped by more than `resetDropPoints` (5 points) or `resetAt`
   moved forward by more than `resetAtChangeToleranceMs` (60s). A backward
   `resetAt` with no drop is treated as a provider correction, not a reset: the
   newest value wins. Only the newest segment is ever used from here on, so
   samples from two quota cycles are never blended.
3. **Fit the burn rate.** Take the samples of that segment inside
   `rateWindowMs` (6h) of the newest one; if that leaves fewer than two samples,
   widen to the whole segment. Fit an ordinary least-squares line, x in hours
   from the first fitted sample:

   ```
   slope = (n·Σxy − Σx·Σy) / (n·Σxx − Σ(x)²)
   ```

   and clamp at zero — inside one segment usage does not un-burn, so a small
   negative slope can only mean idle, and a larger negative one would have been
   caught as a reset by step 2.
4. **Project.** With `burn` in percent points per hour and `hoursToReset` from
   the newest usable `resetAt`:

   ```
   projectedPercentAtReset = clamp(used + burn × hoursToReset, 0, 100)
   estimatedExhaustionAt   = lastObservedAt + (100 − used) / burn     // burn > 0 only
   willExhaustBeforeReset  = estimatedExhaustionAt < resetAt
   ```

   Exhaustion is anchored on the newest *observation*, not on `now`, so a stale
   source does not get a free head start on its projection.
5. **Rate the confidence** (section 5), then **derive risk** (section 6).

Why least squares rather than "last minus first": two noisy endpoints should not
decide the answer, and irregular sampling needs no special case. Why not a
weighted fit: see the next section.

## 4. Which history window?

Measured on one dataset — 20 hours of history at 1%/h (20% → 36%), then a
two-hour burst at 10%/h (36% → 66%), reset 10 hours after `now`:

| `rateWindowMs` | samples | span | mean gap | burn %/h | projected at reset | risk | confidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1h | 3 | 60 min | 30 min | 10.0 | 100 (clamped) | high | medium |
| 6h (default) | 13 | 360 min | 30 min | 5.5 | 100 (clamped) | high | high |
| 24h | 39 | 1140 min | 30 min | 1.66 | 82.6 | medium | high |

The 1-hour window is the honest read on "right now" but it is thin: three
samples, and a single interrupted burst dominates it. The 24-hour window is the
most stable and the least useful — it averages a burst into a flat day and, in
this dataset, downgrades the risk band from high to medium for a window that is
genuinely accelerating. Six hours is the default because it covers a whole
5-hour quota window while still reacting within the hour.

Exponentially weighted recent samples would blend the two behaviours with a
halflife parameter. Not implemented: the halflife cannot be calibrated without
real history, and adding a knob whose correct value is unknown is worse than
shipping one documented window with a visible `basis.fitSpanMinutes`. The
`rateWindowMs` option is the escape hatch if a caller wants a different tradeoff.

## 5. Confidence rules

Deterministic, in this order (first match wins):

| Level | Condition |
| --- | --- |
| `insufficient` | No measurable burn rate: fewer than two usable samples in the newest segment, a zero-length fit span, or a degenerate slope (identical timestamps). No numeric fields are returned. |
| `low` | A burn rate exists, but the source is stale (newest sample older than 30 min), **or** there is no usable future `resetAt`, **or** the fit span is under 1 hour. |
| `medium` | Fresh, reset bound present, span ≥ 1h, and at least one of: span < 3h, fewer than 5 fit samples, or mean gap > 30 min. |
| `high` | Fresh, usable reset bound, span ≥ 3h, ≥ 5 fit samples, mean gap ≤ 30 min. |

Two deliberate choices. Confidence describes the **projected outcome**, so a
missing reset bound caps it at `low` — the burn rate is still reported, but the
thing the field is about cannot be computed. And density matters as much as
span: two samples four hours apart are `medium`, not `high`, because span alone
would flatter a sparse series.

These thresholds are module constants, not options. A tuning surface with no
data behind it would be false precision; the rules are tabulated here precisely
so they can be changed when history justifies it.

## 6. Risk rules

| Risk | Condition |
| --- | --- |
| `high` | `usedPercent` is already 100, or projected exhaustion lands before `resetAt`. |
| `medium` | `projectedPercentAtReset` ≥ 80. |
| `low` | Anything else, including a zero burn rate. |
| `undefined` | No projection exists: no burn rate, or no usable reset bound, and the window is not already at 100%. |

Risk inherits the projection's uncertainty and does not encode confidence
itself, so a `low`-confidence `high` risk is possible (a burst measured from two
samples). A UI must show the two together; showing the band alone would imply
certainty the engine does not have.

## 7. Edge cases

| Case | Behaviour |
| --- | --- |
| First sample / only one sample | `insufficient`; `fitSampleCount` 1, no numeric fields, `risk` undefined. |
| Two identical samples | Slope 0 → burn 0, projection equals the current percent, no exhaustion time, `willExhaustBeforeReset` false, risk low. |
| Two samples with the same timestamp | Deduplicated, last one wins, so no zero-span fit is attempted. |
| Quota dropped because a reset happened | New segment; only post-reset samples feed the fit. `basis.segmentCount` exposes the split. |
| `resetAt` moved forward | New segment, even if the percentage had not visibly dropped yet. |
| `resetAt` moved backward, with no drop | Provider correction, not a reset: one segment, newest `resetAt` wins. If the corrected bound is in the past, see the next row. |
| `resetAt` at or before `now` | `basis.resetExpired` true; nothing is projected against a bound that has already passed; confidence capped at `low`. The snapshot is inconsistent with its own reset time, and pretending otherwise would invent a number. |
| Percentage jumped backward by ≤ 5 points | Absorbed as noise into the fit (which is clamped at 0, never negative). |
| Percentage jumped backward by > 5 points | Treated as a reset and segmented. |
| Stale source | Confidence `low`; the numbers are still returned so the caller can decide whether to show them (`basis.latestAgeMinutes`). |
| Missing `resetAt` | Burn rate and `estimatedExhaustionAt` still reported; `projectedPercentAtReset`, `willExhaustBeforeReset` and `risk` undefined; confidence `low`. |
| Provider reports 100% | Treated as an observation, not a projection: projected 100, `willExhaustBeforeReset` true, `estimatedExhaustionAt` = the observation time, risk high — reported even at `insufficient` confidence. |
| Sparse observations / long gaps | Fit widens from the 6h window to the whole segment (`basis.usedWholeSegment`); confidence reflects the density, so 2 samples over 4h is `medium`. |
| Clock skew | Samples more than 5 min in the future are dropped. A future stamp inside the tolerance is kept and its age reads as 0, never negative. |
| Ancient samples | Older than 24h relative to `now`: dropped. |
| Malformed input | NaN percentages, unparseable `observedAt`, blank labels: dropped. Percentages outside 0–100: clamped, not dropped. |
| Unparseable `now` | Throws `TypeError`: that is a programming error, not noisy data. |

## 8. Example projections

Actual engine output for a 5-hour window sampled every 5 minutes over 5 hours
(61 samples), `now` = 11:00Z, `resetAt` = 13:00Z:

| Scenario | burn %/h | projected at reset | exhaustion | before reset? | confidence | risk |
| --- | --- | --- | --- | --- | --- | --- |
| 10% → 30%, 4%/h | 4.00 | 38.0 | 2026-09-28T04:30Z | no | high | low |
| 20% → 80%, 12%/h | 12.00 | 100 (clamped from 104) | 2026-09-27T12:40Z | yes, 20 min before reset | high | high |
| flat at 42% | 0.00 | 42.0 | — | no | high | low |
| one sample, fresh reset | — | — | — | — | insufficient | — |
| 2 samples 4h apart, 20% → 40% | 5.00 | 50.0 | 2026-09-27T23:00Z | no | medium | low |

The second row is the case the feature exists for: 80% used with two hours left
is not alarming on its own, but 12%/h means the window runs out twenty minutes
before it resets. The first row is the opposite: 30% used looks harmless and the
projection confirms it.

## 9. Is this good enough for v0.3?

The math is good enough for v0.3. The remaining limitations are model shape and
presentation discipline.

Two caveats the numbers do not fix. First, the burn rate is a straight line
through one segment, so a step change in usage is averaged, not anticipated: the
6h default understates a burst that began 30 minutes ago, and the 1h window
overstates it. The confidence field does not communicate that choice — only
`basis.fitSpanMinutes` does. Second, accuracy depends on the bounded 24-hour
history supplied by v0.3 integration. The engine intentionally owns no storage;
if local history is unavailable or empty, confidence degrades and the UI hides
the projection.

Surface it under three conditions. Keep the bounded 24-hour observation history,
because without it every launch shows `insufficient` and the feature looks
broken. Show the projection only at `medium` or `high` confidence, with the burn
rate or projected percentage beside it; a bare risk badge derived from a
low-confidence fit would be misleading. Keep the 6h window: weekly windows move
too slowly for the projection to change what a user does, which is fine - the
5-hour windows are where this pays for itself.
