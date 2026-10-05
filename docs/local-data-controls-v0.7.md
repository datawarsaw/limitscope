# LimitScope v0.7 Local Data Controls

Status: **ACTIVE CONTRACT** (authoritative for the v0.7 local data lane)
Branch: `feature/v0.7-local-data-controls`
Provenance: reconciled from `feature/v0.7-local-data-controls` and
`codex/v0.7-local-data-controls`; this document supersedes both earlier lane
drafts.

LimitScope persists several unrelated classes of local state. This document
records what the app owns, what it deliberately does not, and what each
product control removes. It is the single authority for the v0.7 local data
surface; where any other document disagrees, this one wins.

## 1. Ownership inventory

One row per data class. "App-managed" means LimitScope creates, reads, and
deletes the store itself; "Clearable" means a product control removes it.

| Data class | Owner | Category | Storage | Retention | App-managed | Clearable | Clear operation | Preserved by other clears |
|---|---|---|---|---|---|---|---|---|
| Quota history | LimitScope (Rust) | Usage history | `<app-data>/quota-history-v1.json` | Bounded (24 h detailed tier, 7 d total, 500 samples per window cap) | Yes | Yes | **Usage history** [Clear] → `clear_history` → `QuotaHistoryStore::clear()` (memory + file, revision bump) | Settings, floating prefs, provider cache, execution runs, credentials |
| Legacy quota history blob | LimitScope (WebView) | Usage history | `rate-limits.quota-history.v1` (localStorage, pre-v0.5 leftover removed by the one-time import) | Ephemeral | Yes | Yes | Same **Usage history** clear (`removeLegacyHistory()`) | Everything else in the WebView profile |
| Provider cache (last-good) | LimitScope (Rust) | Provider cache | `<app-data>/provider-last-good-v1.json` (+ `.json.tmp` mid-write) | Bounded (7 d max age, dropped at hydration) | Yes | Yes | **Provider cache** [Clear] → `clear_provider_cache` → `ProviderLastGoodStore::clear()` (persisted storage only) | History, settings, floating prefs, execution runs, credentials |
| Core preferences | LimitScope (WebView) | Preferences | `rate-limits.settings.v1` (localStorage) | Persistent until reset | Yes | Yes | **Preferences** [Reset] → `resetSettings()` through the read-modify-write merge | Unknown/foreign fields in the same object; history; cache; execution runs; credentials |
| Floating bar preferences | LimitScope (WebView) | Preferences | `rate-limits.floating-quota.v1` (localStorage) | Persistent until reset | Yes | Yes | **Preferences** [Reset] → `resetFloatingQuotaPrefs()` through the same merge | Unknown/foreign fields; history; cache; execution runs; credentials |
| Execution active run | LimitScope (WebView) | Execution runs | `limitscope.execution-runs.v1` (`activeRun`) | Life of the run bracket | Yes | Yes, behind deliberate confirmation | **Execution runs** [Clear] → `clearExecutionRunsState({ deliberateActiveConfirmation: true })` | History, settings, floating prefs, provider cache, credentials |
| Execution completed runs | LimitScope (WebView) | Execution runs | `limitscope.execution-runs.v1` (`recentRuns`, bounded to 20) | Bounded | Yes | Yes | Same **Execution runs** [Clear]; completed records clear normally even while an active run is preserved | History, settings, floating prefs, provider cache, credentials |
| Manual provenance context file | User (CLI) | CLI-managed state | `.limitscope-provenance-run.json`, written wherever `scripts/provenance.mjs` runs | CLI session | **No** | **No** | none — see §9 | n/a |
| Quota notification state | LimitScope (Rust) | Runtime bookkeeping | `<app-data>/quota-notifications-v1.json` | Bounded (64 windows, 3 notifications per cycle) | Yes | Partly | The `enabled` flag mirrors **Preferences** reset (`set_quota_notifications_enabled`); the bounded per-window dedup latch stays | n/a (not a user-facing category) |
| Autostart entry | LimitScope (OS integration) | OS integration | HKCU `Run` value via the autostart plugin | Persistent | Yes, own entry only | Yes, own entry only | **Preferences** [Reset] calls `disable()` | Every other `Run` entry (never read, modified, or removed) |
| Diagnostic exports | User | Exported file | User-chosen path via the native save dialog | User-managed | No | **No** | none — LimitScope never searches for or deletes exports | n/a |
| Provider credentials | External providers/CLI tools | Credentials | `~/.codex/auth.json`, OpenCode `auth.json`, OpenCodex/Z.ai/Grok/xAI stores, Google/Antigravity material, browser cookies, OS credential vaults | Provider-owned | **No** | **No** | none, by design | n/a |
| Harness telemetry prototype | LimitScope (research) | Prototype | `prototype/v0.7-harness-telemetry` branch only | Non-production | No | No | none — no production telemetry store exists | n/a |

Classification summary (owner / category / retention / clearable):

- Usage history (Rust store + legacy blob) — LimitScope / Usage history /
  bounded / yes.
- Provider cache — LimitScope / Provider cache / bounded / yes (persisted
  storage only; see §5).
- Core + floating preferences — LimitScope / Preferences / persistent / yes.
- Execution active + completed runs — LimitScope / Execution runs /
  run-scoped + bounded / yes (active run behind deliberate confirmation).
- Notification state — LimitScope / Runtime bookkeeping / bounded /
  enabled flag only.
- Autostart entry — LimitScope / OS integration / persistent / yes, own entry.
- Manual provenance context file — user (CLI) / CLI-managed state /
  CLI-session / **never from the app**.
- Provider credentials, diagnostic exports, other applications' files —
  external / Credentials & user files / external / **never**.

Account isolation: a clear covers the entire category across every provider
and every account partition of the owned store, so no stale account-specific
history or cache fragment survives it. Provider-side account data is never
touched — LimitScope has no path to it.

No data summary is shown. A "7 days observed" / "stored locally" readout
would require walking storage the app otherwise never inspects, so the
section stays a set of deliberate actions.

## 2. Product surface

The Settings drawer carries one compact **Local data** section:

```
Local data
  Usage history   Quota observations kept for 24 hours on this device.   [Clear]
  Provider cache  Last known quota retained for the next start.          [Clear]
  Execution runs  Local execution run records on this device.            [Clear]
  Preferences     Theme, refresh interval, notifications, providers,     [Reset]
                  floating bar.
```

There is deliberately no "delete everything" control: the categories have
different owners, retention, and consequences, so each stays explicit.

This section replaces the earlier standalone `Clear history` row from
v0.3-v0.6, which cleared the same store without a confirmation step. The
operation and its preserved state are unchanged; it is now confirmed and
named "Clear usage history".

## 3. Confirmation, success, and failure UX

Each control asks first, in place (never a modal), and states exactly what is
removed and what is not:

- "Clear usage history? This removes locally stored quota observations. Your
  settings and provider credentials are not affected."
- "Clear provider cache? This removes retained last-known quota data. Your
  settings and provider credentials are not affected; LimitScope rebuilds the
  cache on the next successful refresh."
- "Clear execution runs? This removes locally stored execution run records.
  Your quota history, settings, and provider credentials are not affected."
  While an execution run is in flight the confirmation names it explicitly:
  "Discard the active execution run and clear? This discards the active
  execution run and removes all stored run records. …" and the confirming
  button becomes "Discard active run and clear".
- "Reset preferences? This restores LimitScope preferences to their defaults.
  Your quota history and provider credentials are not affected."

The confirming button carries the full action in words ("Clear usage
history"), so the destructive choice is identified by text and not by color
alone. After a successful operation a restrained inline status line appears
("Usage history cleared."); a failure shows a scoped `role="alert"` line
("Could not clear usage history.") and never claims the data was removed.
The preference reset reports a partial result rather than a clean one when
the OS autostart entry cannot be updated. Cancel always leaves every store
exactly as it was, including an active run.

## 4. The credential boundary

Clearing local data can never reach external authentication material:
`~/.codex/auth.json`, OpenCode `auth.json`, the OpenCodex/Z.ai/Grok credential
stores, Google/Antigravity credential material, browser cookies, Windows
credential stores, provider tokens, or another application's files. The UI
offers no "clear provider credentials" action.

Two structural properties enforce this, and both are pinned by tests
(`src/lib/localDataBoundary.test.ts`):

1. **No path crosses IPC.** Every Rust clear is a fixed, application-owned
   target. `clear_provider_cache` takes no arguments at all; the frontend
   names a constant command (`LOCAL_DATA_COMMANDS`), never a path. No generic
   `delete_file(path)`-shaped command exists in the handler surface, and the
   pinned IPC allowlist fails if one is registered.
2. **The clear service names no file.** `src-tauri/src/local_data.rs` contains
   no file-name literal and no credential-location string in its code (its
   prose documents the boundary it enforces). The provider cache clear only
   ever touches the store the runtime already opened.

## 5. Idempotency, corrupt data, and the persisted-only cache clear

Every clear is idempotent, and "nothing to remove" is success, never an error:

- an already-empty history clears successfully;
- a missing `provider-last-good-v1.json` clears successfully (`removed: false`);
- a corrupt `provider-last-good-v1.json` is still removable — corruption never
  blocks clearing data whose ownership is known;
- a corrupt preference object resets to canonical defaults instead of
  preserving garbage;
- corrupt or missing execution-run stores clear the same way;
- repeated clears are no-ops after the first.

**The provider cache clear is persisted-storage-only, on purpose.** It
removes exactly what a cold start would hydrate: the persisted
`provider-last-good-v1.json`, an interrupted write's `.json.tmp` leftover,
and the store's own in-memory mirror of the persisted file. It does **not**
reset the running runtime's in-session state: the currently displayed
snapshot and the runtime's retained last-good map stay as they are until the
next normal refresh cycle naturally replaces them, and the next successful
live refresh is the first writer to the cleared store. The distinction
matters because the two things have different owners and lifecycles:

- *persisted recovery cache* — written by the runtime, read at cold start to
  hydrate retained quota. Owned by the "Provider cache" control.
- *current runtime snapshot* — what the window is showing this session.
  Owned by the refresh cycle; no settings control rewrites it.

Conflating them would make a cache clear visibly blank out live data the
user is looking at, which is a runtime action pretending to be a storage
action. A cold start after the clear hydrates nothing; the next refresh may
replace whatever is on screen; that is the entire contract. (This is the
reconciliation of the two v0.7 lane drafts: the variant that also wiped the
runtime's in-memory last-good state was not adopted.)

## 6. Unknown future preference fields

Both preference stores write through their read-modify-write merge, so a
reset clears the canonical fields of the schema that owns them and leaves
every other key untouched. Unknown fields written by another lane or a future
version survive an explicit reset; LimitScope only clears what it knows it
owns (see the v0.6 contract, `docs/settings-persistence-contract-v0.6.md` §3,
and the `fixtures/settings-migration/future-unknown-field.json` fixture).

Concretely, a stored object like:

```json
{
  "theme": "oled",
  "futurePluginSettings": { "foo": 1 }
}
```

after **Reset preferences** becomes a canonical-default object that still
carries `futurePluginSettings` unchanged (`theme` returns to `graphite`).
The same rule holds for unknown keys in `rate-limits.floating-quota.v1`.

## 7. Preferences reset semantics

Canonical defaults restored: `launchAtStartup` (false),
`refreshIntervalMinutes` (5), `theme` (graphite), `quotaNotifications` (false),
`providerPreferences` (empty order, nothing hidden), `quotaPerspective` (used),
and the floating bar's `visible` (true), `floatingBarEnabled` (true),
`alwaysOnTop` (true), and geometry (system placed).

The refresh interval and the notification enabled flag are mirrored into the
runtime, which persists them itself. The notification lane's bounded
per-window dedup latch is kept: it is runtime bookkeeping, and it only matters
while notifications are enabled again.

Quota history, the provider cache, execution runs, and every credential store
are untouched by the reset.

## 8. Autostart

Launch-at-startup is OS integration rather than ordinary local data. Because
`launchAtStartup` is a canonical preference whose default is off, the
preferences reset disables LimitScope's own autostart entry through the
autostart plugin. No other `Run` value is read, modified, or removed — the
plugin addresses only the entry the app itself registered.

## 9. Execution data and the CLI provenance boundary

The desktop app's owned execution state is the integrated WebView-local
`limitscope.execution-runs.v1` store: `activeRun` is the run bracket in
flight and `recentRuns` is the bounded record of finished runs.
**Execution runs** [Clear] removes both — but an *active* run is never
silently discarded: the confirmation names the active run and the confirming
action is its own worded choice ("Discard active run and clear"). The store
API enforces the same rule independently of the UI: without
`deliberateActiveConfirmation: true` an active run is preserved and only the
completed records clear.

`.limitscope-provenance-run.json` is a different thing entirely: the manual
provenance CLI's context file (`scripts/provenance.mjs`), written relative to
wherever the CLI was invoked. That location is workspace-dependent and not
deterministically app-owned, so LimitScope does not scan for it — not the
process working directory, not any workspace. It is CLI-managed state outside
the desktop app's owned-data clear surface; the CLI that creates it owns
removing it. No app command touches it.

Exported execution receipts chosen by the user (`--out` files) are
user-owned external artifacts, exactly like diagnostic exports: never
searched for, never deleted.

## 10. Diagnostic exports and harness telemetry

Diagnostic bundles are user-created exported artifacts once saved: LimitScope
does not search the filesystem for them and does not delete them from these
controls. Harness telemetry belongs to `prototype/v0.7-harness-telemetry` and
is not production-managed, so no telemetry deletion UI ships here (future
telemetry store: not currently production-managed).

## 11. Floating window

Floating behaviour is a preference class, so the preferences reset restores
its canonical defaults (shown, enabled, pinned, system-placed). The floating
window is a separate webview and applies the restored values on its next
attach. **Clear usage history**, **Clear provider cache**, and
**Execution runs** never touch floating state.

## 12. Implementation map

| Layer | File | Responsibility |
|---|---|---|
| Owned-data service (TS) | `src/lib/localData.ts` | Fixed command table, category copy, history/cache/execution/preference operations |
| Owned-data service (Rust) | `src-tauri/src/local_data.rs` | `clear_provider_cache`, ownership docs, credential boundary |
| Cache store | `src-tauri/src/last_good.rs` | `ProviderLastGoodStore::clear()` (persisted storage only, idempotent) |
| History store | `src-tauri/src/history.rs` | Existing canonical `clear_history` / `QuotaHistoryStore::clear()` |
| Preference stores | `src/lib/settings.ts`, `src/lib/floatingWindowPrefs.ts` | `resetSettings()`, `resetFloatingQuotaPrefs()` (merge-based, unknown fields survive) |
| Execution store | `src/lib/executionRuns.ts` | Active-run guard, bounded completed-run record, `clearExecutionRunsState` |
| Orchestration | `src/hooks/useSettings.ts`, `src/hooks/useQuotaPredictions.ts` | OS integration + runtime mirrors; history clear with immediate UI drop |
| UI | `src/components/LocalDataSection.tsx` | Rows, in-place confirmations, scoped outcomes, focus handling |

Settings components never see a filesystem path: the section calls the owned
application actions it is handed.

## 13. Test coverage map

| # | Assertion | Test |
|---|---|---|
| 1-3 | History clear: populated, empty, corrupt | `src/lib/localData.test.ts`, `src/hooks/quotaHistoryOwnership.test.tsx`, `src-tauri/src/history.rs` |
| 4-6 | History clear preserves settings, provider cache, execution runs | `src/lib/localData.test.ts` (cross-category preservation) |
| 7-9 | Cache clear: populated, missing, corrupt | `src-tauri/src/local_data.rs`, `src-tauri/src/last_good.rs` |
| 10 | Cache clear preserves the current in-session runtime snapshot | `src-tauri/src/local_data.rs` (persisted-only contract) |
| 11 | Cache clear prevents cold-start hydration | `src-tauri/src/local_data.rs` |
| 12-13 | Cache clear preserves history and execution runs | `src/lib/localData.test.ts` |
| 14 | Reset restores canonical settings | `src/lib/settings.test.ts`, `src/hooks/useSettings.test.tsx` |
| 15 | Reset preserves unknown settings fields | `src/lib/settings.test.ts`, `src/lib/localData.test.ts`, `src/hooks/useSettings.test.tsx` |
| 16-18 | Reset preserves history, cache, execution runs | `src/lib/localData.test.ts`, `src/hooks/useSettings.test.tsx` |
| 19 | Floating prefs reset correctly, unknown fields survive | `src/lib/floatingWindowPrefs.test.ts`, `src/lib/localData.test.ts` |
| 20 | Autostart reset touches only LimitScope's entry | `src/hooks/useSettings.test.tsx` |
| 21-23 | Completed runs clear; active run requires deliberate confirmation; cancel preserves | `src/lib/executionRuns.test.ts`, `src/components/LocalDataSection.test.tsx`, `src/App.productIntegration.test.tsx` |
| 24 | Confirmed clear removes the active run | `src/lib/executionRuns.test.ts`, `src/App.productIntegration.test.tsx` |
| 25 | Execution clear preserves history/settings/cache | `src/lib/localData.test.ts` |
| 26 | No credential mutation | `src/lib/localDataBoundary.test.ts`, `src-tauri/src/local_data.rs` |
| 27 | No arbitrary delete-path IPC; pinned command surface | `src/lib/localDataBoundary.test.ts`, `src/lib/runtimeTriggerGuardrail.test.ts` |
| 28-29 | All clears idempotent; corrupt stores removable | across the files above |
| 30 | Scoped error reporting; success never lies | `src/components/LocalDataSection.test.tsx`, `src/lib/localData.test.ts` |

## 14. Dormant trigger contract: bounded local-log scanning

Status: **DORMANT — not built, not scheduled, not accepted.** Nothing in this
section describes shipped behavior, and no part of §1–§13 depends on it. It is
recorded now so that if a future provider or evidence lane ever needs to read
potentially large or growing local files — JSONL activity logs, session
transcripts, rolling usage logs, local execution evidence — the safety
properties are fixed before any implementation exists, not negotiated around
one.

**Current state, stated plainly:** no local-log scanner exists today; no
cursor or scan-state schema is needed today; no scanning worker, service, or
background task should be built speculatively; and every provider adapter
(`codex.rs`, `zai.rs`, `opencode_go.rs`, `grok.rs`, `antigravity.rs`)
continues to do exactly what it does now — plain read-only `read_to_string`
of its small, named credential/config/cache files. This section adds no owned
row to the §1 inventory, no command to any surface, and no setting.

### 14.1 Activation condition

The contract activates only when an adopted provider/evidence lane requires
reading potentially large or growing foreign-owned local files. Until such a
lane is adopted, this section is inert prose. Adopting the lane means
adopting every rule in §14.2; none may be relaxed per-lane.

### 14.2 The rules

1. **Incremental cursor.** Track file identity (path plus a stable identity
   check such as size/identity metadata) and a byte offset; read only newly
   appended data. A changed file identity means "new file", never "re-read
   the world".
2. **Bounded reads.** An explicit maximum bytes per refresh, and explicit
   file-count limits where relevant. No code path reads an entire large
   transcript by default.
3. **Fail-closed ceiling.** If unread data exceeds the configured safe
   ceiling, that refresh is rejected or skipped as a whole and the failure is
   visible. The cursor is never silently advanced past unread data.
4. **Backlog skip on first observation.** Large historical backlogs are not
   retroactively attributed: first-seen historical content is outside normal
   live attribution (the attribution ladder,
   `docs/execution-provenance-contract-v0.7.md` §1) unless the lane
   explicitly adopts it later.
5. **Read-only ownership.** Foreign-owned files are never mutated,
   truncated, rotated, renamed, or deleted — the same boundary §4 and §9
   already enforce for credentials and provenance context.
6. **Bounded retention.** Parsed evidence, if it is retained at all, gets
   explicit retention and text-size limits and an owned row in §1 with its
   own clear operation, like every other app-managed store.
7. **Opt-in gating.** Sensitive local scanning is explicitly enabled by the
   user; while disabled, the scan call is not even constructed — no probe
   read, no existence check, no stat of the source files.
8. **Testable read-only proof.** The implementation ships a test that proves
   the source files are byte-identical after a full scan cycle.

### 14.3 What this contract is not

It is not an implementation plan. No cursor format, storage location,
scheduler, or module boundary is chosen here, and building any of them ahead
of an adopted lane would violate this section. QuotaBar's scanning constants
may be cited as inspiration where the repository's reference-project
convention already cites it (`docs/provider-discovery.md` §4), but its exact
numbers are not frozen as LimitScope policy: limits are chosen when a lane is
adopted, with recorded rationale.
