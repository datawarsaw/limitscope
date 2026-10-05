# Usage Analytics Backend Contract (v0.7)

This document defines the deterministic backend/data contract for the future
LimitScope Usage view. This branch does not build the Usage UI, charts,
navigation, or dashboard changes.

The source remains the existing bounded `quota-history-v1.json` observation
log. The analytics layer adds no database, telemetry, stored aggregates, or
synthetic activity points.

## Query surface

One IPC command answers the complete request:

```ts
get_usage_analytics(query: {
  range: "24h" | "7d";
  providerId?: string;
  account?: string | null;
  windowLabel?: string;
  exactAccount?: boolean;
}): Promise<UsageAnalytics>
```

`account: null` matches every account by default. With
`exactAccount: true`, it selects only unattributed observations. A non-null
account always selects that exact account. Provider, account, and window
filters compose and preserve the history store's
`(providerId, account, windowLabel)` isolation.

The response contains:

- `summary`: the five supported summary metrics;
- `heatmap`: one explicit observed/not-observed cell per local calendar day;
- `trends`: true observation points grouped by logical window;
- `coverage` within each trend series: gap thresholds and not-observed spans;
- `gapSemantics: "notObservedNeverZeroFilled"`;
- `availabilityInference: "none"`.

The TypeScript wire types are in `src/lib/usageAnalytics.ts`; the Rust wire
types and builder are in `src-tauri/src/usage_analytics.rs`.

## Summary strip

Only the five research-proven metrics are exposed.

### Peak observed usage

The maximum stored `usedPercent` in the selected range, including its
provider, optional account, window, and true observation timestamp.

- `24h` is `exact` relative to stored detailed observations.
- `7d` is `lowerBound`, because 30-minute compaction can discard an
  unrepresentative intermediate peak.

The DTO carries `exactness: "exact" | "lowerBound"`. A 7-day peak is never
silently labeled exact.

### Most constrained window

The identity-bearing result of the same deterministic maximum selection:

1. highest `usedPercent`;
2. earliest `observedAt` on ties;
3. lexicographically smaller `(providerId, account, windowLabel)` on remaining
   ties, with an absent account sorting before a present account.

The result returns `providerId`, optional `account`, `windowLabel`,
`usedPercent`, `observedAt`, and the same peak exactness qualifier.

### Observed reset cycles

Count of detected reset-boundary transitions in the selected range. The
implementation calls the same `is_reset_boundary` predicate used by history
compaction:

- `usedPercent` drops by more than 5 percentage points; or
- `resetAt` moves forward by more than 60 seconds, or changes presence/value.

There is no second reset algorithm and no stored reset-event table. The count
and trend annotations are query-time projections. They are a lower bound:
silent resets, resets within gaps, and resets with less than a 5-point visible
drop and no `resetAt` change are unobservable.

Use the wording **Observed reset cycles** or **Observed resets**. This contract
does not expose or imply provider-granted resets remaining.

### Observed days

Count of distinct local calendar days containing at least one selected
observation. It measures observation coverage, not user activity. Missing days
are not counted and are not represented as zero-usage days.

### Time near limit

This metric is always `estimated: true` and is omitted when no comparable
interval exists (empty history or isolated points).

Thresholds are `usedPercent >= 80` and `usedPercent >= 95`. The deterministic
method is piecewise-linear interpolation between adjacent observations only
when:

1. both points belong to the same reset cycle; and
2. their interval is at or below that series' gap threshold.

For an interval of duration `d` from `v0` to `v1`, the estimated duration
above threshold `T` is:

- `d` when both endpoints are at or above `T`;
- `0` when both endpoints are below `T`;
- otherwise `d` multiplied by the post/pre-crossing linear fraction.

The reported share is threshold duration divided by the sum of comparable
interval durations across selected series. No point is invented, no line is
smoothed through a reset, and long gaps are never interpolated. The DTO
returns both thresholds, estimated milliseconds, estimated share, comparable
span, and method/policy strings.

This is a discrete-observation estimate, not continuous monitoring.

## Heatmap

The sole semantic is daily peak observed `usedPercent` after active filters.
Observation count is never a heatmap value.

Absolute bands use fixed physical boundaries:

| Band | Peak used percent |
|---|---|
| `1` | `0.00-25.00` |
| `2` | `>25.00-50.00` |
| `3` | `>50.00-75.00` |
| `4` | `>75.00-100.00` |

Each day explicitly returns `observed: true | false`. An unobserved day has
no peak, no identity, and no band. It is never coerced to zero.

Cells return the winning timestamp and window identity for deterministic
tooltips. Equal daily peaks use the summary tie-break. Peak exactness follows
the same range rule as the summary peak.

## Trend and reset annotation

Each trend series is one logical window:
`(providerId, account?, windowLabel)`.

Points contain true observation timestamps and canonical used-orientation
`usedPercent`. They do not contain a remaining-perspective duplicate. A UI
may derive remaining capacity as `100 - usedPercent`.

Each point contains:

- `cycleId`: deterministic opaque ID derived from logical-window identity and
  the first retained timestamp of that cycle;
- `cycleStart`: true on the first retained point of the cycle;
- `resetBoundary`: true on the point that opens a detected reset transition;
- `resolution: "detailed" | "compacted"`;
- optional original `resetAt`.

Cycles are annotated over the full retained stream before the 24-hour slice is
applied, so the same point has the same `cycleId` in 24-hour and 7-day
queries. The ID is query-stable for retained data; retention expiry can
legitimately change the first retained timestamp of a cycle.

The UI contract is to break the line at differing `cycleId` values and use
`resetBoundary` for a marker. It must not smooth or connect across resets.

## Gaps and missing data

The per-series gap threshold is:

```text
max(45 minutes, 2 * median adjacent same-cycle observation interval)
```

The median is the deterministic integer midpoint of sorted positive intervals.
Leading, internal, and trailing spans longer than that threshold are returned
as `NotObservedGap` entries with exact `from`, `to`, `durationMs`, and
`kind: "notObserved"`.

Gaps mean the state is unknown. They never mean zero usage, provider downtime,
or provider availability. The response therefore includes
`availabilityInference: "none"`; missing history alone cannot prove
availability.

## Storage and performance

The analytics query reads only the existing bounded store:

- 500 detailed points per logical window in the recent 24 hours;
- 350 compacted points per logical window from 24 hours to 7 days;
- 850 points maximum per logical window;
- 8,500 points maximum for a representative 5-provider, 2-window workload.

No database is introduced.

The representative benchmark constructs 8,500 retained points across 10
logical windows and measures the four requested phases. On the validation
machine:

| Phase | Debug build |
|---|---:|
| Summary | 265.7494 ms |
| Heatmap | 9.1676 ms |
| 24h full query | 21.1449 ms |
| 7d full query | 318.0296 ms |

The benchmark asserts an upper bound of 1,000 ms for summary, 500 ms for
heatmap, and 2 seconds for each full query. Each phase is measured as the
fastest of five timed passes after one untimed warm-up pass. The summary
budget was raised from 500 ms in September 2026 after CI runs 36769066810 and
36776194193 failed with single-shot samples of 506.0/529.5 ms on
byte-identical code: one wall-clock sample on the shared Windows runner
(inflated by preemption and sibling test-thread contention) sits at that
runner's noise floor, while the stable debug floor is ~265 ms and the release
floor ~82 ms. The fastest-of-five aggregate is immune to inflated individual
samples, and a real slowdown raises every sample including the fastest one,
so the gate still fails any change that roughly triples the summary cost of
the representative workload. The full 7-day builder includes cycle annotation,
summary, heatmap, trend, and gap projections.

## Contract tests

Rust tests pin:

- empty history and explicit unobserved days;
- single-window behavior without invented estimates;
- multiple providers and multiple windows;
- reset preservation inside one compacted bucket;
- gap-day semantics;
- 24-hour exact peak;
- 7-day lower-bound peak;
- 80% and 95% estimated intervals;
- account and unattributed isolation;
- window isolation;
- absolute heatmap bands;
- reset annotation stability across ranges;
- canonical `usedPercent` preservation;
- deterministic equal-peak tie-breaking;
- representative 7-day query performance.

TypeScript tests pin the single `get_usage_analytics` IPC call and prohibit a
fabricated browser fallback.

## Intentionally unavailable

This contract does not expose tokens, sessions, turns, models, tools, cache,
latency, task cost, provider availability history, resets remaining, or
30-day/lifetime aggregates. None can be derived honestly from quota
observations.
