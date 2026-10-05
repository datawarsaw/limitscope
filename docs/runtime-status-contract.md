# Runtime status contract (v0.6)

Status: implemented on `feature/v0.6-runtime-status-contract` (base:
`release/v0.5.0-rc`). No provider fetcher, quota normalization, provider
correctness value, history identity, prediction calculation, or UI layout
changed — this contract only names what the runtime already projects, adds
the two missing distinctions (cooldown, unavailable), and moves the state
decision fully into Rust.

## Source of truth

`src-tauri/src/runtime.rs` computes one explicit `health` value per provider
entry (the `ProviderHealth` enum) and ships it on every snapshot wire entry.
The frontend reads `usage.health` and derives visual labels only — it never
reconstructs health from `status`/`error`/`dataFreshness` fragments. The
legacy `status` field (`"ok" | "stale" | "error" | "unknown"`) stays on the
wire unchanged but is **derived** from `health`
(`ProviderHealth::legacy_status`) so the two fields can never disagree:

| health        | status (legacy) | frontend label  |
| ------------- | --------------- | --------------- |
| `live`        | `"ok"`          | Live / Cached   |
| `stale`       | `"stale"`       | Stale           |
| `unknown`     | `"unknown"`     | Unknown         |
| `cooldown`    | `"error"`       | Cooldown        |
| `error`       | `"error"`       | Refresh failed  |
| `unavailable` | `"error"`       | Unavailable     |

(The legacy vocabulary has no finer word than `"error"` for the three
failure families; the health field is the distinction.)

## Health states

### Live

The **current provider fetch succeeded**: data came from the current
account, passed provider normalization, and is within freshness policy.
Live never means "we have some last-known data" — a retained value is
never live, under any circumstance.

### Stale

Previously valid data is being **retained for display**, and the freshness
policy says it is no longer current. Today this is Antigravity's own
source-snapshot verdict (`dataFreshness: "stale"` on a successful read).
Tomorrow it is also where a **persisted last-good** snapshot maps: a
hydrated entry is `health = "stale"`, `freshness = "stale"` — never live,
never error (nothing has failed yet; the data is simply not current).

### Unknown

The provider **answered successfully but reported no usable quota windows**
(empty limits, or Grok's zero-window success with retained data shown).
This is the legacy `status: "unknown"` success shape, kept as an explicit
health so "the source said nothing usable" stays distinct from "the source
could not be reached" (error) and from "nothing to show" (unavailable).
Unknown entries never enter history and never fire notifications (they
carry no windows), and they never read as a confirmed live value.

### Cooldown

A server-directed `Retry-After` cooldown is active, so the provider is
**intentionally not fetched** — by any refresh source. Distinct from error
(nothing failed *now*; the next attempt is deferred by the server), from
stale (no freshness verdict is implied), and from unavailable (the provider
can absolutely be evaluated once the window passes). If last-good data
exists it is shown as retained data, **but the health stays cooldown**.
Cooldowns are ephemeral runtime state: never persisted, never recorded as
history, capped at 24 hours. While the cooldown holds, the entry keeps the
failure metadata of the refresh that seeded it. Expiry plus a successful
fetch returns the provider to live.

### Error

The **current refresh attempt failed**. With retained last-good data, the
data stays on display with its original `checkedAt` and
`error: "Refresh failed: <message>"` — and the health is still `error`.
Error-with-last-good is never projected as live and never as ordinary
stale. Without retained data the entry is a bare error entry.

### Unavailable

**No usable current or retained provider state exists, and the provider
cannot currently be evaluated because the required local source/auth state
is absent.** This is decided by the failure's stable code
(`failure_is_source_absent` in `runtime.rs`): missing/expired/unreadable
credentials, rejected auth, missing login state, missing local cache
sources (`cache_missing`, `no_account`, `quota_missing`), and the
`*_not_installed` family. Ordinary transient failures (transport, HTTP,
schema, entitlement verdicts) are **never** unavailable — without retained
data they are plain `error`. A source-absent failure *with* retained
last-good stays `error` (the retained data is still shown; see the
transition table).

## Freshness (independent, small)

`dataFreshness` remains a small, independent signal — the freshness of a
provider's **cached source snapshot** — and never duplicates health:

- `"fresh"` / `"stale"` — Antigravity's own verdict about the snapshot it
  read (a fresh verdict without a parseable stamp degrades to stale).
- Absent — the provider has no source-snapshot concept (this is freshness
  *unknown*, not stale; absence is never fabricated into a value).

Meaningful pairings: `health = live` + `freshness = fresh` (a live fetch
from a fresh cache — the "Cached" label); `health = error` +
`freshness = stale` or `fresh` (a retained entry keeps the freshness of the
snapshot it retains); `health = cooldown` + `freshness = stale`. The
frontend may combine them only for labels ("Cached"), never to re-derive
health.

## Retained last-good semantics

A failed refresh retains the provider's last good usage:

- values, windows, and account attribution are kept **verbatim** — the
  retained entry keeps the account it was fetched for (a switch to another
  account never re-attributes it), and the next successful fetch replaces
  the retained state wholesale with the new account's data;
- `checkedAt` stays at the original fetch time — on a retained entry it
  *is* the last-success time, so no separate `lastSuccessfulAt` field
  exists;
- the entry is explicitly in the failure family: `health = error`
  (or `cooldown` while a server cooldown defers the next attempt);
- without last-good data, the failure surfaces bare — `error` or
  `unavailable` per the code classification above.

## Error metadata

Failure entries carry normalized, display-safe metadata alongside the
existing `error` message:

- `errorCategory` — the structured error's stable `code` (the
  `provider_error.rs` vocabulary; never raw provider response text);
- `errorHttpStatus` — the offending HTTP status when the failure came from
  a non-success response (a bare number, no headers or body).

Both are omitted on non-failure entries and stay compatible with the
diagnostics security contract (no provider response text is exposed
anywhere on the wire).

## History eligibility

Recorded per completed cycle by `history.rs` (`observations_from_usages`,
the single gate shared with notifications):

| health        | freshness        | eligible |
| ------------- | ---------------- | -------- |
| `live`        | fresh / absent   | **yes**  |
| `live`        | `stale`          | no       |
| `stale`       | any              | no       |
| `unknown`     | any              | no       |
| `cooldown`    | any              | no       |
| `error`       | any              | no       |
| `unavailable` | any              | no       |

Only genuine successful current observations are recorded. History identity
is unchanged: observations carry the provider's proven account identity, or
record unattributed.

## Notification eligibility

Threshold and recovery notifications evaluate the **same gate** — the same
`observations_from_usages` result — so they fire only on live/current
observations. No threshold crossing is detected from stale, unknown,
cooldown, error, or unavailable entries, and a failure cycle can never
re-arm or fire a threshold.

## Transition table

| From ↓ / Event → | fetch succeeds | fetch fails | Retry-After set | skip while cooling | source/auth absent |
| ---------------- | -------------- | ----------- | --------------- | ------------------ | ------------------ |
| (start)          | live / stale¹  | error       | cooldown²       | —                  | unavailable        |
| live             | live / stale¹  | error³      | cooldown²       | cooldown²          | unavailable        |
| stale            | live / stale¹  | error³      | cooldown²       | cooldown²          | unavailable        |
| unknown          | live / stale¹  | error³      | cooldown²       | cooldown²          | unavailable        |
| cooldown         | live / stale¹  | error³      | cooldown²       | cooldown²          | unavailable        |
| error            | live / stale¹  | error³      | cooldown²       | cooldown²          | unavailable        |
| unavailable      | live / stale¹  | error       | cooldown²       | cooldown²          | unavailable        |

¹ Antigravity's source verdict decides live vs stale; a success with no
usable windows is `unknown`. ² Only a 429/5xx failure carrying a parseable
`Retry-After` applies a cooldown; the health of every subsequent skip is
`cooldown` regardless of retained data. ³ With retained last-good the entry
shows the retained values and stays in the failure family; without
last-good a source-absent code projects `unavailable`, anything else
`error`.

## Frontend contract

- `src/types.ts` — `ProviderHealth` union; `ProviderUsage.health` is
  required; `status` is legacy (derived in Rust); `errorCategory` /
  `errorHttpStatus` appear only on failure entries.
- `src/lib/v03Integration.ts` — `providerStatusPresentation` reads
  `health` and picks label + class only; `visiblePrediction`,
  `refreshCycleSucceeded`, and the attention rail read `health` directly.
- `src/lib/floatingQuota.ts` — reads `health` for the stale marker; the
  failure states ride the existing `error` visual class.
- No TS file implements cooldown policy: the structural guardrail
  (`runtimeTriggerGuardrail.test.ts`) bans Retry-After handling and any
  cooldown logic outside the wire vocabulary.
- A structural guardrail also pins the production registry order
  (`openai-codex`, `zai`, `opencode-go`, `antigravity`, `grok`) — unchanged.

## Deterministic tests

Rust (`src-tauri/src/runtime.rs`, plus `history.rs` and `notifications.rs`
eligibility tests) pins: fresh success → live; stale verdict → stale (never
live, on the wire too); failure with last-good → error + retained values +
metadata; failure without last-good → error vs unavailable by code;
cooldown bare and with last-good; cooldown expiry + success → live;
persisted-style stale snapshots never read live; retained last-good keeps
its account and is replaced wholesale on account switch; recovery to live
from error and from cooldown; freshness independent from health; the wire
key set (`health`, `errorCategory`, `errorHttpStatus`, no snake_case leak);
registry order. TypeScript pins the presentation mapping (labels, classes,
notes), the attention rail lines, the floating states, and the prediction
gate's failure-state behavior.
