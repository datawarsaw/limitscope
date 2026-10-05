# Quota history store — design

Bounded, durable history of quota observations: the local memory that lets the
prediction engine (`src/lib/prediction`) reason
about burn rates and exhaustion across app restarts. Implementation:
`src/lib/quotaHistory.ts`. The v0.3 release candidate wires this module into
the app through `src/hooks/useQuotaPredictions.ts`; the *Write policy*,
*Corruption recovery*, and *Integration contract* sections note where the
shipped wiring supersedes the original unwired contract.

## Storage choice

| | A. `localStorage` | B. Tauri app-data JSON file |
|---|---|---|
| Already in the app | Yes — `settings.ts` persists under `rate-limits.settings.v1` | No |
| Dependencies | None | `@tauri-apps/plugin-fs` (new plugin + capability config) **or** a custom Rust command |
| Sync/async | Synchronous read-modify-write | Async IPC; the caller must handle in-flight writes |
| Corruption handling | Parse + salvage, self-heal rewrite (below) | Same logic, plus file-lock and partial-write concerns |
| Persistence location | WebView2 user-data dir (per OS user), like settings | App-data dir (per OS user) |

**Chosen: `localStorage`** (key `rate-limits.quota-history.v1`). It is the
simplest reliable mechanism this app already trusts for settings: no new
plugin, no Rust change, no async plumbing, and history is written in a single
document per refresh. The tradeoffs are accepted knowingly:

- A WebView profile wipe (OS-level "clear browser data", profile deletion)
  loses the history. History is an expendable cache — it rebuilds as refreshes
  accumulate — and settings already accept the same exposure.
- The ~5 MB WebView storage quota is shared with settings; the bounding rules
  keep this store roughly two orders of magnitude below it (see *Expected
  storage size*), and a quota failure is handled as an ordinary
  non-fatal write failure.

The storage backend is injectable (`QuotaHistoryOptions.storage`) so tests run
without a DOM and a future caller can redirect it without touching the module.
With no backend available, every function degrades to "empty history, not
persisted" rather than throwing.

## Data model and identity

One observation is a flat record identical in shape to the engine's
`QuotaObservation`, plus one optional store-level field:

```ts
{ providerId, windowLabel, usedPercent, observedAt, resetAt?, account? }
```

- Timestamps are canonicalized to ISO-8601 UTC on entry, so byte-identical
  entries correspond to identical instants.
- A **logical quota window** is the `(providerId, windowLabel)` pair, joined
  with a `\u0000` separator for grouping — the same scheme the engine uses.
  Labels ("5h", "weekly") repeat across providers and are never treated as
  globally unique.
- `account` (store-level, engine-invisible) is the provider-proven account
  identity token (`AccountAttribution.identity`, e.g. `key:3456`). It
  partitions a provider's windows per account — see *Account partitioning*
  below. Absent means unattributed.
- Percentages are clamped to 0–100 and an unparseable `resetAt` degrades to
  absent — both mirror the engine's own tolerance, so the store never rejects
  a sample the engine would have accepted, and never keeps one the engine
  would reject (non-finite percentage, blank identifier, unparseable
  `observedAt` are rejected outright). A present-but-invalid `account`
  (blank, oversized, or containing the separator) rejects the observation:
  data whose attribution cannot be trusted must not silently degrade into
  the unattributed partition.

The persisted document is one JSON blob: `{ "version": 1, "observations": [...] }`,
sorted by provider, account, window label, then time (diff-stable, single
write per refresh). A blob with an unrecognized `version` is discarded
whole — blending an unknown layout into the prediction input is worse than
losing a cache.

**The `account` field does not bump the blob version.** It is an additive,
optional per-observation field: a v0.3.0 build reading a newer blob strips
the unknown field on its next rewrite (a safe, lossy downgrade), and this
build reading a v0.3.0 blob sees every entry as unattributed. Bumping the
version would instead discard the whole blob — including the history of
providers that never carry an account — which is exactly the loss the
migration policy below is designed to avoid.

## Write policy

`recordObservations` is the only entry point and accepts raw, untrusted
records; every entry is validated individually and counted
(`{ accepted, rejected, persisted }`), so a batch with one bad row cannot
poison the rest. Callers gate on their side as well; the shipped wiring
(`historyObservationsFromUsages` in `src/lib/v03Integration.ts`) samples only
providers whose refresh succeeded (`status === "ok"`), excluding simulated
providers and stale snapshots:

- Record **only from successful provider data** — never from error refreshes
  (a `ProviderUsage` with `status === "error"`).
- Skip unknown/placeholder windows; labels come from the provider's own
  window list.
- **Stale snapshots are stored, but duplicates cannot grow storage.** When a
  provider re-serves an unchanged cached snapshot, the caller should pass the
  snapshot's own time (`sourceUpdatedAt`) as `observedAt`. Every re-observation
  of the same snapshot then collapses onto the stored entry: dedup keys on the
  exact observation instant per window, newest input winning. Ten identical
  stale refreshes add zero entries.
- Observations older than the retention window, or stamped more than five
  minutes into the future (clock skew, same tolerance as the engine), are
  rejected at write time — storing them would only be pruned on the next pass.

## Retention and bounding

Three rules, applied on every write and every load (and on demand via
`pruneHistory`):

1. **24-hour retention** — entries older than 24h relative to the reference
   time are pruned. An entry exactly 24h old is retained, matching the
   engine's `maxSampleAgeMs` comparison.
2. **500 newest per logical window** — the count cap is per
   `(providerId, account, windowLabel)` triple and always keeps the newest
   samples. At the default 5-minute refresh interval a window accumulates
   ~288 samples in 24h, so the cap only engages at faster intervals.
3. **Duplicate collapse** — same window, same account, same instant = one
   entry. Different accounts of one provider never dedup against each other.

### Account partitioning (MIC-297 follow-up)

History identity is `(providerId, account, windowLabel)`. The `account`
token comes from the provider itself: `AccountAttribution.identity`, set
only when the provider can prove which account the windows belong to.
OpenCode Go uses `key:<last-four-of-stored-key>` — the same console-style
fingerprint the card already displays (MIC-297), never the credential
itself. A future multi-account provider opts in by setting its own stable
token; providers that never prove an account keep a single unattributed
partition, exactly as before.

**Why the key tail is stable enough.** The tail is re-derived from the
stored credential on every refresh: it changes if and only if the stored
credential changes, which is precisely the swap signal the isolation needs.
Its cost is symmetric and bounded: rotating a credential on the *same*
account (new key, new tail) starts fresh history, and two distinct
credentials colliding on a four-character tail would merge their partitions
— both decay within the 24-hour retention horizon, and neither can leak
data outward. Prediction loss from a swap is equally bounded: the engine
rebuilds a fit from a few hours of samples.

**Read-side isolation.** The store keeps every account's entries; callers
narrow them. `historyForCurrentAccounts` (`src/lib/v03Integration.ts`) —
wired into `useQuotaPredictions` before the engine runs — admits, per
provider: only observations carrying exactly the currently proven identity
when the current usage proves one; otherwise only unattributed
observations. Accounts are never aggregated, and a provider with no
current usage contributes no predictions.

### Legacy migration policy (conservative, no wipe)

v0.3.0 history has no account field. On upgrade:

- Legacy entries stay in the blob under the unattributed partition and age
  out through the normal 24-hour retention — no rewrite, no destructive
  step, no version bump.
- They are **never attached to the current account**: once OpenCode proves
  an identity (`key:…`), attributed reads and predictions exclude
  unattributed entries outright, so the pre-upgrade account's observations
  cannot feed the new account's burn rate, cycle segmentation, or
  projections.
- Unrelated providers' history is untouched by the upgrade.

The alternative — dropping or re-attributing legacy OpenCode entries at
load time — was rejected: dropping early buys nothing over retention
(≤ 24 h), and re-attribution would guess an identity the old build never
proved.

### Reset cycles

The store deliberately performs **no reset detection and no merging**:

- Samples from consecutive quota cycles coexist as distinct entries, each
  carrying the `resetAt` its provider reported at observation time, so the
  engine can segment cycles itself (percentage drop > 5 points, or `resetAt`
  moving forward).
- Old cycles cannot grow unbounded: the 24h rule and the newest-500 cap
  evict them, and because the cap keeps the *newest* samples, a fresh cycle
  is never starved by the cycle it replaced. Within a window's 500-entry
  budget, cycle history simply rolls off as the new cycle accumulates.

## Corruption recovery

Degradation is handled at the granularity it occurs, and every degraded load
writes the sanitized state back so it self-heals:

| Persisted state | Behavior |
|---|---|
| Unparseable JSON, wrong version, wrong shape | Empty history; blob rewritten to a fresh v1 document |
| Array with invalid entries | Valid entries salvaged, invalid discarded individually, result rewritten |
| Healthy but over-retained (aged/duplicate/over-cap) | Pruned view returned, pruned state rewritten |
| Storage read fails | Empty history; next write rebuilds from incoming data |
| Storage write fails (`recordObservations`) | Counts still returned with `persisted: false`; in-memory refresh data unaffected |

Hard rules: the module never throws on data or storage failures (only an
invalid *explicitly injected* `now` throws — a programming error), and no code
path can break a provider refresh. The shipped call site is fire-and-forget:
`useQuotaPredictions` records observations in an effect after the refresh
result is applied, and a persistence failure only surfaces as the Settings
note `Prediction history is unavailable on this device.`

## Expected storage size

~170–200 bytes per observation (two RFC-3339 timestamps dominate; ~130 without
`resetAt`), plus ~35 bytes of envelope:

| Scenario | Per-window samples (24h) | Typical windows | Size |
|---|---|---|---|
| Default 5-min refresh, 3 providers × 2 windows | ~288 | 6 | ~0.3 MB |
| 1-min refresh (count cap engaged) | 500 | 6 | ~0.5 MB |
| Worst case modeled: 10 windows × 500 cap | 500 | 10 | ~1 MB |

Hard ceiling: `windows × 500 × ~200 B`. Against the ~5 MB WebView quota the
modeled worst case keeps an order of magnitude of headroom even counting
settings and WebView overhead. Reads and writes are one document, so a
worst-case load is a ~1 MB string parse per app start — negligible next to
rendering.

## Privacy and security implications

- Stays on-device in the WebView2 profile (per OS user); never synced, never
  sent anywhere, no telemetry.
- Contains only usage percentages, window labels, provider identifiers,
  timestamps, and (for providers that prove an account) the provider's
  display-safe identity token — e.g. OpenCode Go's four-character key tail,
  the same fingerprint the card already shows. No tokens, credentials, raw
  secrets, or message content: the store persists only what
  `AccountAttribution.identity` explicitly declares safe, and rejects
  anything else.
- Readable by anything the user's OS profile can read (same trust boundary as
  `settings.ts`); no secrets are at risk by construction.
- Cleared via the Settings `Clear history` action, which calls
  `clearHistory()`, or by removing the WebView profile; the store also forgets
  everything older than 24h on its own.

## Integration contract with the prediction engine

- `loadHistory()` returns `QuotaObservation[]` — structurally the engine's
  input type. `predictWindows({ observations: loadHistory(), now })` requires
  no mapping. `getWindowHistory(providerId, label)` returns one window's
  series ascending by time, ready for `predictWindow`; pass
  `account: <identity>` for account-aware providers (or omit it for the
  unattributed partition only).
- Verified against the real engine (integrated on this branch at
  `src/lib/prediction`): a recorded
  session including a mid-series reset produces two segments with `high`
  confidence, the expected burn rate and exhaustion projection; windows
  without `resetAt` still yield burn rates; shared labels across providers
  stay separate; TypeScript accepts `loadHistory()` output as
  `QuotaObservation[]` without casts.
- Shipped wiring (v0.3 release candidate): `useQuotaPredictions` records
  observations after each refresh result is applied, feeds
  `predictWindows({ observations: history, now })`, and maps predictions back
  to provider windows through `v03Integration`. Recording and prediction stay
  best-effort: a storage or prediction failure never blocks provider refresh,
  tray actions, or last-good rendering.
- Account-aware wiring (MIC-297 follow-up): the hook records observations
  carrying `usage.account.identity` and runs predictions over
  `historyForCurrentAccounts(history, usages)`, so each account's
  predictions are computed from that account's samples only.
