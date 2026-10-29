# Multi-account OpenAI/Codex monitoring

LimitScope can monitor two independent ChatGPT accounts side by side. Each
account is read from its own local Codex login; the accounts never share a
credential, a quota window, a reset clock or a failure.

## The identity model

The provider ID identifies a configured slot, not a ChatGPT user. Retained
Codex data is also bound to both the authenticated user and the workspace.
Different users in a Team workspace can share the same workspace ID.

| Account | Account id / provider id | Credential reference |
| --- | --- | --- |
| Primary | `openai-codex` | the default Codex home (`$CODEX_HOME`, then `~/.codex`) |
| Secondary | `openai-codex:secondary` | its own Codex home, named in the settings |

The primary keeps the historical provider ID and its preferences. Legacy
Codex history and last-good entries cannot prove their user and are discarded;
new observations build fresh, user-bound history.
The secondary uses an account-scoped id: everything that only knows the
provider resolves it back to `openai-codex` by dropping the scope suffix
(`baseProviderId` in `src/lib/providerIds.ts`,
`is_codex_provider_id` in `src-tauri/src/runtime.rs`).

The local Codex auth file contains the ID and access tokens. The adapter reads
`https://api.openai.com/auth.chatgpt_user_id` (or `user_id`) from both and
requires the user claims to agree. `tokens.account_id` supplies the workspace;
contradictory workspace claims or a contradictory usage response fail closed.
These locally decoded claims are not signature-validated by LimitScope. This
is a local attribution mechanism that trusts the existing Codex-owned auth
file, not an authentication or authorization check. Codex itself parses
these claims: https://github.com/openai/codex/blob/main/codex-rs/login/src/token_data.rs.

The internal `chatgpt-v2:` binding is SHA-256 over domain-separated,
length-prefixed user and workspace identifiers. It uses no token, email,
quota, or alias. It remains stable across token rotation when those IDs stay
the same; a different user or workspace yields a different partition. The
unkeyed digest is pseudonymous and linkable across installations, not a secret
or anonymization against someone who knows a candidate identifier. Provider ID
remains a separate partition. The UI label still uses only a masked workspace
hint. When the current binding is missing or changes, snapshots suppress
retained values, including on pause, cooldown and cold start; cooldowns do not
carry over to another binding. History reads, analytics, export and diagnostics
filter to the current binding. Identity is rechecked at snapshot publication,
not continuously between UI updates. Live two-account validation and Human
Acceptance remain pending.

### The session-generation fence

Each slot carries its own **session generation**: a counter that advances
every time the runtime observes a change of that slot's identity, including
an identity that moves away and back. The generation is the consistency token
for every Codex-owned surface, and it is deliberately **not** the global
snapshot sequence:

| Surface | What decides ownership |
| --- | --- |
| Snapshot projection | the slot's current binding vs. the observation's binding |
| History, analytics, export, diagnostics | the per-slot generation and identity agreed across the bracketing captures |
| Last-good cache | the slot's current binding; a foreign entry is dropped from memory and from disk |
| Notifications | the `(provider, identity, window)` key, so a window belongs to one identity partition |
| Frontend | the per-slot generation carried in `codexIdentityStates` |

The global sequence moves for unrelated reasons — a Grok cycle, a wallet
publication, an OpenDesign manual refresh, an account-configuration change.
Using it for Codex ownership meant that any other provider's activity could
discard valid Codex history, or make a valid Codex identity update look
stale. The per-slot generation removes that coupling: unrelated providers
cannot suppress a Codex slot, and one slot's failure never suppresses
another slot or an unrelated provider.

Two details are deliberately weaker than the generation:

- The notification lane keys its per-window state by the identity
  partition, not by the session generation. Returning to an earlier
  identity therefore returns to the same partition and keeps that
  partition's already-fired flags, so the same account is not notified
  twice for one threshold. It is the same user either way, so no value
  crosses a user boundary.
- The frontend keeps the highest generation it has seen for a slot, and
  never lowers it. Removing a slot also retires that generation: delayed
  snapshots at the same generation cannot restore its reading or identity
  state. The runtime drops the removed registration while retaining its
  monotonic generation allocator, so re-adding even the same user starts a
  newer generation. Normal refreshes within an active generation still land.

### The actual consistency guarantee

The guarantee is **the last session observed by LimitScope**, and nothing
stronger:

- The runtime re-reads the local auth file on every snapshot and on every
  collection, and compares the observed identity with the one the pending
  work was started under. A result whose generation moved is discarded
  before it can reach the cache, the history, the notifications or the
  snapshot.
- An A → B → A move advances the generation twice. Returning to A therefore
  does **not** revive A's earlier reading: the reading belonged to a session
  the runtime has moved past, and it is dropped.
- A slot whose login cannot be proven fails closed **for that slot only**.
  The other slot and every unrelated provider keep their own state.
- The frontend orders every Codex slot by that slot's own generation, so a
  delayed read, a delayed event or an unrelated provider's sequence movement
  can never move a slot to an earlier session. A failed revalidation hides
  Codex until a later read confirms it; events cannot clear that fence.

This is **not** atomic coordination with the externally modified Codex
authentication file. LimitScope does not share a lock with the Codex
process, never writes that file, and never observes it continuously. An
authentication change that happens entirely between two observations cannot
be detected, and a change that lands immediately after an observation is
only visible at the next observation. Every statement above is scoped to the
last observed session.

Codex token-usage collection in Usage Intelligence is disabled while the
source cannot prove event ownership. Existing Codex token events remain in the
store but are excluded from its visible projections; they are not assigned to
an account retroactively. This policy is separate from user-bound quota history.

The alias ("Personal", "Work — WG") is display only. It is never part of an
id, never a credential, and it never appears beside the compact bar's
percentages: the expanded account card is where an account is named.

## Authentication — what is supported and who performs it

The supported mechanism is the official Codex CLI login, scoped to a
directory:

```powershell
New-Item -ItemType Directory -Path C:\codex-home-second
$env:CODEX_HOME = "C:\codex-home-second"
codex login --device-auth
codex login status
```

Each account owns its own `CODEX_HOME` and therefore its own `auth.json`.
LimitScope only ever READS that file: it never writes it, never copies it,
never shares one account's file with another identity, and never scrapes a
browser session. `cli_auth_credentials_store` must be `file` (or the CLI
must be configured to write the file) for the directory-scoped login to
exist.

The account owner performs the login themselves — that step cannot be done
on their behalf, and the app never invents or picks a credential.

## What the endpoint reports

The adapter reads `https://chatgpt.com/backend-api/wham/usage`, the same
ChatGPT usage backend the Codex CLI uses, with the account's own access
token and its own `chatgpt-account-id` header. The `rate_limit` windows it
returns are that account's Codex usage windows (a 5-hour and a weekly pool).
They are NOT ChatGPT product-wide message limits, and the app never states
them as such. The endpoint is not publicly documented, so a schema change
is reported as a deterministic `unexpected_response` error rather than
guessed at.

## Collection and presentation

- The accounts are configured in Settings → Providers & Wallets → OpenAI /
  Codex accounts. The settings lane forwards the configuration to the
  runtime on attach and on every change; the runtime persists its own
  mirror (`codex-accounts-v1.json` in the app-data directory), so the
  scheduler's startup cycle already honors it before any window attaches.
- An account that names no Codex home is not registered at all: it appears
  in no rail, no diagnostics and no export.
- **Pause monitoring** stops collection for that account. Its segment stays
  and keeps its last known reading, dimmed, and the runtime marks the entry
  as frozen so nothing may read it as live.
- **Hide from bar** is the existing provider visibility preference:
  presentation only, the account keeps refreshing and keeps its history.
- The two accounts never share a quota number: no window is averaged,
  summed or merged, and one account's failure, expiry or cooldown can never
  invalidate the other account's data.

## Boundaries

- The app reads a local file the account owner created. It performs no
  login, stores no token, and writes nothing into the Codex home.
- Token refresh is not implemented: an expired session fails until Codex
  itself re-authenticates that account.
- The number of accounts is fixed at two. A third slot is an additive
  change: one provider kind, one account id and one config entry.

## Secondary-result attribution correction (2026-10-28)

The accepted Floating Island D candidate `d56571b08d58caeb725ff7d630ed1c6658a60e84`
registered the secondary account correctly, but `normalize_codex` constructed
every successful DTO with `base_usage(ProviderKind::Codex, ...)`. The result
therefore carried `openai-codex` even when its collector belonged to
`openai-codex:secondary`. Failures without last-good used the correct kind;
retained failures could carry the incorrect id copied from a prior success.

The registration closure now passes its captured `ProviderKind` into
`normalize_codex`. This is the boundary that constructs the provider DTO:
the slot identity comes from registration, independently of optional masked
account hints in the payload. The primary retains its historical id.

The production path is:

1. `runtime::start` builds `RuntimeCore` with `production_specs` and
   `with_codex_accounts`. `codex_specs` registers configured accounts, maps
   each id through `codex_kind_for`, and captures its own Codex home and kind.
2. `get_codex_usage_for` → `fetch_codex_usage` → `fetch_usage_for_auth` reads
   the configured source and requests usage for that account. The existing
   `parse_usage_response` returns `CodexUsage` with windows, plan and masked
   attribution; failures retain the existing attempted-account hint.
3. The registered closure calls `normalize_codex(kind, outcome, now_ms)`.
   `run_provider_job` applies success/failure and cooldown transitions using
   the same registered kind. In-memory last-good is keyed by `kind.id()`;
   the stored DTO now agrees with that key.
4. `run_cycle` joins independent provider jobs and records history from
   normalized entries. `observations_from_usages` uses the full DTO id and
   account hint; failure/cooldown/stale observations are excluded. Paused
   accounts are not collected or recorded and keep their own prior entry.
5. `ProviderLastGoodStore::record_success` persists live readings by
   `kind.id()`. On restart, `hydrate` resolves each registered id separately
   and returns stale readings. Errors and cooldowns do not replace the cache.
6. `snapshot` projects account aliases/monitoring flags by full id.
   `run_chain` publishes that snapshot through `runtime://snapshot`; the
   pull command returns the same DTO. `useProviderUsage` consumes snapshots
   with a sequence guard. `floatingQuotaItems` deduplicates by full id, and
   `FloatingQuotaBar` / `FloatingDockStrip` use that id as their React key.
   Previously the duplicate primary id made this deduplication drop one
   account. `baseProviderId` remains only a provider-family presentation lookup.

Regression coverage in `runtime/codex_attribution_tests.rs` injects raw
`CodexUsage` / `ProviderError` outcomes at the collector boundary of the
production registration factory. It executes the real normalizer, runtime
jobs, snapshot publication, history and disk-cache round trip. It covers
single-account compatibility, missing payload account hints, bare failures,
both concurrent completion orders, retained failure, cooldown, pause/resume,
and stale hydration, in both account directions. The frontend consumer test
passes both account ids through snapshot events and the real floating-bar
projection for retained error, cooldown, stale and paused states.

These are fixture-based regressions, not authenticated multi-account
verification. A live read of both independently configured accounts remains
an operator-owned release check. This correction changes no visuals,
authentication, credentials, installed application, or release/version surface.

Validation on 2026-10-28:

- `cargo test --locked`: 1068 PASS, 18 intentionally ignored live probes,
  0 FAIL (1086 tests in the binary).
- `npm test`: 79 files / 1387 tests PASS, including the new per-slot
  consumer cases and the existing source/updater security guardrails.
- `npm run build`: PASS (TypeScript and Vite).
- `cargo check --locked --all-targets --features custom-protocol`: PASS;
  two dead-code warnings, both in unchanged `zcode_plans` code.
- `npm run scan:secrets`: PASS over 5 files (built frontend, Tauri
  config and the release bundle). The tracked scanner was not changed.

New production-path regressions added for this candidate:

- Rust (`src-tauri/src/runtime/codex_identity_tests.rs`, plus the per-slot
  fence tests in `history.rs`): two users in one Team workspace; an
  unprovable login in one slot; an observed A -> B -> A move; a result
  started under an older generation; a session change immediately before
  the cache, history and notification writes; cache restoration after a
  session change; unrelated provider activity against valid Codex history;
  a single Codex account beside an unrelated provider.
- Frontend (`src/hooks/useProviderUsage.test.ts`,
  `src/lib/usageAnalytics.test.ts`): two slots in different sessions; one
  slot failing closed while the other stays visible; an A -> B -> A move; a
  Codex slot that moved while an unrelated provider moved the global
  sequence; a read that outlives the revalidation interval; and the
  identity-scope fence, including the A -> B -> A case; plus a retired
  slot's earlier session never being reinstated by a late snapshot.

Negative controls, run against the unfixed code and then restored:

- Restoring the global-sequence rule in the Codex fence made
  `unrelated_provider_activity_does_not_discard_valid_codex_history` fail
  with 0 of 2 Codex rows surviving, and the fixed source was restored.
- Restoring the revalidation tick's read-fence poisoning made
  `lands a read that takes longer than the revalidation interval` fail
  with the Codex reading stuck at its earlier value, and the fixed source
  was restored.
- Removing the frontend's per-slot generation high-water fence made
  `never reinstates an earlier session after the accepted entry was
  retired` fail with user A's reading back on screen, and the fixed
  source was restored.

Removed-slot regression (2026-10-28): the real `useProviderUsage` hook
reproduced secondary at generation 5 / seq 1 / user A / 14%, removal at seq 2,
then resurrection from the delayed original event. The added regression
failed before the correction and passes afterward, including same-user
re-addition at generation 6, another delayed original event and a normal
same-generation refresh. A runtime regression separately failed before the
correction because re-addition reused the old generation; it now confirms a
new generation without changing the primary slot's generation.

Validation after the removed-slot correction: 1069 Rust tests passed
(18 live probes ignored), 1388 frontend tests passed across 79 files,
frontend build and `cargo check --locked --all-targets --features
custom-protocol` passed. One initial targeted run of the existing single-slot
history-count test returned 2 rows instead of 1; the isolated rerun, full
identity group and final full Rust suite passed without changing that test.

These are deterministic fixture results. They do not establish live
two-account behaviour, which remains an operator-owned check.

