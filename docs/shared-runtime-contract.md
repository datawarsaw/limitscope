# Shared Rust provider runtime (v0.5, "More Rust Behind the UI")

Status: implementation candidate on `feature/v0.5-shared-runtime` (step 1)
plus the phase 2 history migration (`feature/v0.5-rust-history`), the
resilience layer (`feature/v0.5-runtime-resilience`, below), and the
threshold notification lane (`feature/v0.5-threshold-notifications`,
below) — Human Acceptance pending. Steps so far: (1) the shared provider
runtime; (2) Rust-owned quota history (below); (3) resilience — server
cooldowns and wake/reconnect refresh triggers; (4) threshold
notifications (below). Prediction calculation and account-aware filtering
stay in TS.

## Source of truth

`src-tauri/src/runtime.rs` is the single owner of provider refresh:

- cycle orchestration and fetch invocation — the existing `get_*_usage`
  backends are called in-process, unchanged (no parser rewrite, same
  credential lookup, quota normalization, account attribution, and
  Antigravity freshness verdicts; the former TS adapters' normalization
  moved into Rust verbatim, pinned by tests);
- per-cycle coalescing — at most one cycle runs at a time; a request landing
  mid-cycle sets one pending follow-up flag, so exactly one follow-up cycle
  runs afterwards and no parallel cycles or queues can build up;
- one bounded transient retry — 750 ms + 0–250 ms jitter, only when the
  structured error carries a transient verdict (or the legacy `network`
  code / 429–5xx status); Antigravity never retries; no provider burns the
  fast retry when the server sent a `Retry-After` hint (the hint becomes a
  cooldown instead — see Resilience below);
- last-good retention — a failed refresh keeps the provider's last good
  usage, marked `status: "error"` with `error: "Refresh failed: <message>"`,
  at the original `checkedAt`; without last-good data a bare error entry
  stands; no provider ever disappears because one cycle failed;
- Grok's 15-minute cadence (cached results within the window; local
  credential failures are never cached);
- the refresh interval timer — one immediate cycle at startup, then one per
  interval; hidden or closed windows do not affect the timer.

## IPC contract

Commands (both windows call them; custom commands are not ACL-gated):

- `get_runtime_snapshot` → full `RuntimeSnapshot`
- `request_refresh` → starts or coalesces a cycle
- `request_refresh_on_reconnect` → the webview `online` hook; a trigger
  only, with per-kind burst dedupe (see Resilience below)
- `set_refresh_interval` (minutes) → updates the runtime scheduler; the
  persisted TS settings value is pushed on attach and on every change

Events:

- `runtime://snapshot` — full snapshot after every completed cycle (no diff
  protocol). `seq` is a monotonic counter; consumers ignore snapshots older
  than the newest they applied, which makes the pull-vs-event race harmless.
- `runtime://cycle-started` — once per chain start; drives the loading
  projection (a chain's follow-up cycles do not re-fire it, matching the
  old coordinator's `onStart` semantics).

The snapshot additionally carries `historyRevision` (phase 2): a monotonic
counter over the Rust-owned quota history, bumped only when the history
actually changed (new observation, prune, import, clear). Consumers re-pull
`get_history` only when it differs from the revision they last pulled, so
the history file is never transferred on unchanged cycles.

Snapshot entries serialize to the exact `ProviderUsage` wire shape the
frontend already had (`camelCase`, optional fields omitted), so rendering,
presentation wording, and staleness display are unchanged TS code.

## TS view role

`useProviderUsage` keeps its public API and becomes a pure consumer: pull
once on attach (subscribe first, then pull), apply seq-guarded snapshot
events, forward the interval, and wrap `request_refresh` with an optimistic
loading span. It holds no provider timer, no retry, no last-good, no
adapter calls. Staleness presentation (interval × 2 threshold, "min ago")
stays in TS, anchored to the runtime's `lastUpdatedAt`.

The floating window's Refresh (`requestGlobalRefresh`) and the tray's
Refresh both call the runtime directly — there is exactly one refresh
pathway, and one snapshot fan-out updates every window.

Prediction calculation, themes, layout, interaction, and countdown formatting
remain TS-owned. The provider adapters (`src/providers/*`), registry,
`RefreshCoordinator`, and `transientRetry` were deleted with the migration;
their behavioral coverage lives in `runtime.rs` tests.

## Quota history (v0.5 phase 2: Rust-owned)

`src-tauri/src/history.rs` is the single owner of quota history: recording,
persistence, retention, deduplication, validation/self-healing,
account-aware partitioning, and the clear mutation. The runtime records one
observation batch per completed cycle from the same normalized snapshot it
broadcasts, so any number of open windows produce exactly one history
stream — the former main-window-only localStorage recorder is gone, along
with the entire TS storage layer (`recordObservations`, `loadHistory`,
pruning, self-heal rewrites, `clearHistory`).

**Ownership split.** Rust owns history; TS owns prediction calculation. The
frontend reads history (`get_history`), clears it on user request
(`clear_history`), and ran a one-time legacy import
(`import_legacy_history`); the prediction engine, confidence logic,
MIC-298 protections, and the visibility gate are unchanged TS
(`predictWindows`, `historyForCurrentAccounts`).

**Storage.** One bounded JSON file in the Tauri app-data directory:
`<app-data>/quota-history-v1.json`, shape
`{ "version": 1, "observations": [...] }` — byte-compatible with the
retired localStorage blob. Written temp-then-rename. A missing file is an
empty history; a corrupt or foreign-version blob degrades to the
salvageable subset (or empty) and the sanitized state is written back —
startup never crashes on bad history. No SQLite, no persistence framework,
no compression, no unbounded append log.

**Schema.** Each observation carries exactly the six safe fields
(`providerId`, `windowLabel`, `usedPercent`, `observedAt`, optional
`resetAt`, optional `account`) — pinned by tests; no token/credential/secret
-shaped field exists in the schema, and account values are the masked
display-safe identity tokens (`key:XXXX`, `xai:<id>`), never credentials.

**Rules preserved from the TS store, test-for-test.** 24-hour retention
relative to the runtime clock (an observation exactly 24 hours old is
retained); newest-500-per-window bound over `(providerId, account,
windowLabel)`; same-timestamp dedup with the newest input winning (quota
cycles never merge); 5-minute clock-skew rejection; clamped percentages;
`resetAt` degrading to absent when unparseable; present-but-invalid account
rejecting the observation; per-cycle eligibility (only usable `ok`
snapshots, source-snapshot timestamp preference, malformed sibling windows
dropped without losing the rest; simulated providers cannot occur — the
production registry carries none).

**Account isolation.** History partitions on the proven account identity
exactly as in v0.3.1: an attributed account never inherits another
account's observations, and legacy unattributed entries stay unattributed —
import never attaches them to a proven account. No credential material is
ever persisted.

**Legacy migration (one-time).** On main-window attach, the frontend reads
the legacy localStorage blob (`rate-limits.quota-history.v1`), sends it to
`import_legacy_history`, and removes the key only after a successful
import. The Rust side validates every entry with the store rules and
merges by dedup, so the import is idempotent — a retry or two windows
importing concurrently cannot duplicate samples. A failed or malformed
import leaves the key in place for the next launch and never blocks
startup. After migration, future writes go only to Rust; there is no
dual-write.

**Clear history.** The settings button clears through `clear_history`
(in-memory state immediately, persisted file replaced with an empty
envelope), removes any legacy key so cleared history cannot resurrect, and
the prediction UI updates immediately. Main and floating stay consistent:
both re-read on the next `historyRevision` change.

**IPC.** `get_history` → full observation list; `clear_history` → unit;
`import_legacy_history` (observations) → `{ accepted, rejected }`. History
reaches the UI via pull-on-revision, not broadcast: no history event
framework, no full-file push per cycle.

## Resilience (v0.5 step 3: cooldowns, wake, reconnect)

**Server cooldown ownership.** A 429/5xx response carrying a standard
`Retry-After` header (integer delta-seconds or HTTP-date — one shared
parser in `provider_error.rs`, malformed values ignored safely) puts the
provider on an ephemeral cooldown of `now + min(hint, 24 h)`. The cooldown
lives in the runtime only (`ProviderCooldown` in `runtime.rs`): it is
never persisted, never spans an app restart, and is cleared by the next
successful refresh. While it lasts the provider is not fetched at all:
the skip projects exactly the failed refresh's shape (last-good retained
and marked errored, or a bare error entry), so history — which samples
only structurally successful refreshes — can never record a fake quota
observation for a skipped fetch. No countdown UI and no cooldown field on
the wire: the existing errored entry is already the honest state.

**Cadence vs cooldown.** They are separate gates. Provider cadence (e.g.
Grok's 15-minute minimum poll interval) and the server cooldown are
checked independently; the next eligible fetch is the **later** of the
two. A cooldown expiring inside a still-open cadence window keeps serving
the cached result.

**Manual refresh rule.** Every refresh source respects a server-directed
cooldown — scheduled ticks, wake/reconnect triggers, and manual refreshes
alike. Manual refresh may not bypass cadence either (unchanged behavior);
a server's explicit `Retry-After` is stronger than any local cadence, and
there is deliberately no "spam retry" escape hatch.

**Wake/resume trigger.** The scheduler sleeps in 30-second chunks and
compares the wall clock against each completed chunk. A suspend freezes
the monotonic timer but not the wall clock; wall-clock overshoot beyond a
2-second tolerance means the machine slept, and exactly one wake refresh
runs (it replaces the pending interval tick, which restarts from now).
This is the smallest reliable Windows mechanism available to the stack —
Tauri exposes no native resume event and a power-broadcast hook would need
a new Win32 dependency. On platforms whose monotonic clock runs through
sleep, the interval deadline simply fires on wake as a normal tick, so
behavior is safe either way; a burst of wake events coalesces through the
per-kind dedupe window.

**Network reconnect trigger.** Both webviews listen for the browser
`online` event and invoke the shared `request_refresh_on_reconnect`
command — JS is only a trigger, never a fetch owner. The runtime coalesces
reconnect bursts (interface flapping, two windows firing) per trigger
kind within a 5-second dedupe window; a trigger landing mid-cycle joins
the single pending follow-up.

**All sources converge.** Scheduler ticks, the main window's Refresh, the
floating bar's Refresh, the tray Refresh, wake, and reconnect all end in
the same entry points (`try_begin_cycle` / `try_begin_auto_cycle`) on the
one runtime: one cycle at a time, one pending follow-up, provider cadence
and cooldowns always honored. Manual refreshes are never deduped; the
automatic triggers are, per kind, independently. A structural guardrail
test (`src/lib/runtimeTriggerGuardrail.test.ts`) pins that TS can only
invoke the shared runtime/history/window commands — no provider fetch
command and no cooldown logic exists on the TS side.

## Threshold notifications (v0.5 step 4: one bounded lane)

`src-tauri/src/notifications.rs` is the single owner of quota threshold
notifications. The lane is evaluated exactly once per completed runtime
cycle, next to the history recording, so the main and floating webviews can
never produce duplicate notifications — TS holds no notification logic at
all (pinned by the structural guardrail scan). Delivery goes through the
Tauri notification plugin from Rust, best-effort: a failed toast never
fails a cycle.

**Thresholds.** Exactly two threshold kinds, fixed for v0.5: NEAR LIMIT at
80% used and CRITICAL LIMIT at 95% used, plus one v0.6 recovery kind
(below). No error, stale, prediction, or digest notifications exist.

**Crossing semantics.** A notification fires only when the window's
previous actual observed usage is below the threshold and the new actual
observation is at or above it. A first observation of a window — app
start, restart, new cycle — is always baseline and never notifies, so
startup cannot spam. While usage stays above a fired threshold, no further
notification fires for it (79→81 fires once; 82, 84, 99 stay silent).

**Dedup key.** State is keyed by the logical quota window
`(providerId, account identity, windowLabel)` plus the observed `resetAt`
cycle and the threshold kind. Account values are the provider-generated
display-safe identity tokens (`key:XXXX`, `xai:<id>`); no credential or
token material is ever stored or rendered.

**Reset-cycle re-arm.** A fired threshold re-arms only when the window
reports a new, valid, different `resetAt` — a new cycle clears the fired
marks and its first observation becomes the baseline. A window without a
usable `resetAt` is undatable: it never re-arms, and there is deliberately
no time-of-day guessing.

**Account isolation.** Accounts of one provider never share notification
state, and unattributed (legacy) state never attaches to a proven account
— the identity slot of the key keeps them apart.

**Stale/error suppression.** Notifications require a usable current
observation, using exactly the history store's eligibility rules
(`observations_from_usages`): failed, stale, errored last-good,
cooldown-retained, unknown, and malformed windows never notify and never
move the crossing baseline. Simulated providers cannot occur — the Rust
production registry carries none.

**Burst guard.** At most 3 native notifications per cycle, ordered
deterministically: most severe first (critical, then near-limit, then
recovery), then highest usage, then identity order. Windows whose
notification did not fit the cap are not marked fired, so a later genuine
crossing can still notify them. One window emits at most one notification
per cycle even when it jumps past both thresholds (70→97 is a critical,
not near + critical). Recovery and threshold crossings are mutually
exclusive for a single observation (< 80 vs >= 80).

## Quota recovery (v0.6: quota available again)

One new notification kind, RECOVERY, reusing the same Rust lane, identity
model, eligibility rules, burst cap, toggle, and JSON file — no framework,
no frontend changes.

**Reset evidence requirement.** Recovery fires ONLY on the
reset-transition observation itself: previous valid `resetAt`, new valid
`resetAt`, new different and later (epoch-ms compare). Usage drops
without a `resetAt` transition never notify (96% -> same resetAt 4% is
silent). Backward resets (new earlier than previous, clock skew) re-arm
thresholds but never recover.

**Previous/next thresholds.** Previous cycle peak >= 80% (max eligible
usage in the just-ended cycle, falling back to the last baseline for
pre-v0.6 state); current usage < 80%. Examples: 84% -> new cycle 3% YES;
97% -> new cycle 12% YES; 79% -> new cycle 4% NO; 96% -> new resetAt 87%
NO (still constrained). A new cycle that starts high does NOT recover
later in the same cycle when usage drops — that would be a usage-drop
heuristic, explicitly out of scope for v0.6.

**Dedup key.** `(providerId, account, windowLabel, new resetAt, recovery)`.
One recovery maximum per new reset cycle. Restart-safe via the persisted
`fired` mark (fired-before-deliver); Main + floating never duplicate
(single Rust evaluation point); wake/reconnect bursts coalesce upstream.

**Account/window isolation.** Same slots as thresholds: account A never
recovers for account B; unattributed never attaches to proven; each
windowLabel independent (Weekly may recover while 5-hour does not).

**Undatable windows.** Missing/invalid `resetAt` never recovers. No
time-of-day guessing, no usage-drop heuristic.

**Burst ordering.** Critical, then near-limit, then recovery; then highest
usage, then identity. Cap stays 3 per runtime cycle (not increased).
Capped-out recoveries stay unfired and wait for the next genuine reset.

**Persistence compatibility.** Same file
(`quota-notifications-v1.json`, version 1, 64-window cap, temp-then-rename,
self-healing). Three additive optional fields per window
(`cyclePeak`, `prevPeak`, `prevResetAt`); v0.5 files load safely via
serde defaults, with previous-peak fallback to `lastPercent`. No
migration screen.

**Copy.** Title: `{provider} quota available again`. Body:
`{window} reset · {used}% used`. Factual, compact, no recommendations.

**Lifecycle pinned by test.** Cycle A 79->82 NEAR, 82->96 CRITICAL, 96
stays silent; Cycle B 96->4 with new resetAt RECOVERY; 4->30 silence;
30->81 NEAR again.

**Persistence.** The toggle and the dedup/re-arm state live in one bounded
JSON file, `<app-data>/quota-notifications-v1.json`, written
temp-then-rename. A corrupt or foreign-version file degrades to empty and
is rewritten sanitized; the map is bounded to the 64 most recently updated
windows. Because fired state is persisted before delivery, a restart can
never re-notify the current cycle, and the scheduler's immediate startup
cycle respects the persisted toggle even before any window attaches.

**Setting.** One drawer toggle, "Quota notifications", default off
(opt-in, like launchAtStartup — interruptive behavior is never default-on
in this app). The persisted TS setting is forwarded to
`set_quota_notifications_enabled` on attach and on every change, exactly
like the refresh interval; thresholds are fixed (80/95), with no
per-provider rules, sounds, quiet hours, or rule builders.

**Notification click.** Focusing the main window on toast activation is
deferred (`NOTIFICATION_CLICK_DEFERRED`): the Windows backend of the
notification plugin exposes no activation callback, and a custom
framework is out of scope for this step.


## Persisted normalized last-good provider state (v0.6)

`src-tauri/src/last_good.rs` is the single owner of persisted last-good
quota state. The purpose is to allow the UI to display the last known
provider quota state immediately after a cold app start, before the first
live provider refresh completes.

### Trust rules & stale semantics
- Hydrated state is explicitly marked `status: "stale"` and `dataFreshness: "stale"`.
- It is NEVER treated as live data or a new observation.
- The UI renders it with the amber Stale dot and explicit source note
  ("Showing data from <time>").
- Hydration creates zero history observations: `observations_from_usages`
  skips any usage where `status != "ok"` or `data_freshness == Some("stale")`.
- Hydration never increments the history revision.
- Hydration never triggers notifications: the notification lane relies on
  actual current observations from `observations_from_usages`.
- Predictions are suppressed: `visiblePrediction` explicitly checks
  `status === "stale"` and `dataFreshness === "stale"` and returns undefined.

### Storage & bounded schema
- Stored in one bounded JSON file in the Tauri app-data directory:
  `<app-data>/provider-last-good-v1.json`, envelope shape:
  `{ "version": 1, "providers": [ { "providerId": "...", "providerName": "...", "account": { ... }, "limits": [ ... ], "lastSuccessfulAt": "...", "sourceUpdatedAt": "..." } ] }`.
- Written temp-then-rename (`MoveFileExW` atomic replacement on Windows) to
  guarantee crash safety.
- Self-healing: a corrupt or foreign-version file degrades safely to empty
  and rewrites sanitized on next save; a malformed window is dropped
  without dropping sibling windows; one bad provider never invalidates
  valid providers.
- Secret audit: strictly no tokens, cookies, authorization headers, raw
  payloads, raw error objects, or secret material exist in the schema or
  persisted blob. Account values are masked display-safe identities
  (`key:XXXX`, `xai:<id>`), never credentials.

### Cache age & expiry
- Max cache age: 7 days (`MAX_CACHE_AGE_SECS = 604_800`). Any entry older
  than 7 days relative to the runtime clock is discarded at hydration time.
- Implausible future stamps (> 5 minutes clock skew) are also discarded.

### Reset safety
- If a cached quota window's `resetAt` is in the past at hydration time,
  that window is omitted.
- If all windows of a provider have reset in the past, the provider is
  omitted entirely rather than shown with an empty/misleading shell.

### Account isolation
- Account A's persisted state never attaches to or blends with Account B.
- When Account B's live refresh arrives, it replaces Provider A's state
  cleanly.
- Unattributed cached state never attaches to a newly proven account.

### Write policy
- Only genuine successful provider refreshes (`status == "ok"`, non-empty
  limits, not a stale projection) update the cache.
- Provider errors, cooldown skips, stale projections, and unavailable
  states NEVER overwrite a good cached entry.
- Provider successes update independently; failure of Provider A does not
  erase Provider B's or Provider A's last good state.


## Runtime panic containment (v0.8.3)

One provider panic or one cycle-body panic can never silently remove a
provider from a cycle, and can never permanently stop automatic
refreshes. Containment reuses the existing structured failure path — no
parallel error model, no new vocabulary:

- **Fetch boundary.** Every provider fetch call (the closure call itself
  and every poll of its future) runs behind a `catch_unwind` boundary
  (`FetchPanicBoundary` in `runtime.rs`). A panic becomes the ordinary
  structured failure — code `unexpected` (the backends' established
  catch-all), `transient: false` — and flows through the normal failure
  path: last-good retention, error metadata, diagnostics. A panic is a
  local defect, so it never burns the fast retry and never creates a
  cooldown; the next cycle simply fetches again. Panic payloads are
  app-internal strings (never credential material); the detail rides the
  error message for diagnosability.
- **Cycle containment.** A job task that still dies without a result (a
  panic outside the fetch boundary) is mapped back to its spec by index
  and projected through `apply_failure` — a provider can never silently
  disappear from a cycle. With several jobs lost in one cycle the panic
  payloads cannot be attributed, so those providers get the generic
  projection.
- **Chain/scheduler guard.** Each cycle in `run_chain` runs in its own
  task. A panicked cycle cannot wedge the in-flight flag (`finish_cycle`
  still closes it), the snapshot still closes the frontend loading span,
  and the next tick starts a fresh cycle. The panicked cycle's own
  results are discarded mid-cycle, so the emitted snapshot shows the last
  completed cycle's state until the next refresh lands.
- **Release profile.** `panic = "unwind"` replaces `panic = "abort"` in
  the release profile — containment is unreachable under abort. The
  binary pays the unwinding tables for the guarantee.

Pinned by tests in `runtime.rs`: a panicking fetch stays in the cycle as
a structured error while siblings still succeed in the same cycle
(`panicking_fetch_becomes_a_structured_failure_and_stays_in_the_cycle`);
last-good retention applies and the provider recovers next cycle with no
retry burn and no cooldown
(`panicking_fetch_keeps_last_good_and_recovers_without_retry_or_cooldown`);
a panic inside the cycle body cannot wedge the flag or kill the chain and
the scheduler keeps refreshing
(`chain_survives_a_panicking_cycle_body_and_keeps_refreshing`).


