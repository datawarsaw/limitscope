# Multi-account OpenAI/Codex monitoring

LimitScope can monitor two independent ChatGPT accounts side by side. Each
account is read from its own local Codex login; the accounts never share a
credential, a quota window, a reset clock or a failure.

## The identity model

The account id IS the provider id. That single choice gives every
provider-keyed subsystem — last-good retention, quota history, cooldowns,
notification state — a separate scope per account for free.

| Account | Account id / provider id | Credential reference |
| --- | --- | --- |
| Primary | `openai-codex` | the default Codex home (`$CODEX_HOME`, then `~/.codex`) |
| Secondary | `openai-codex:secondary` | its own Codex home, named in the settings |

The primary keeps the historical id, so an existing single-account
installation keeps its history, preferences and notifications continuous.
The secondary uses an account-scoped id: everything that only knows the
provider resolves it back to `openai-codex` by dropping the scope suffix
(`baseProviderId` in `src/lib/providerIds.ts`,
`is_codex_provider_id` in `src-tauri/src/runtime.rs`).

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

- New native production-path regressions: 5 PASS. Negative control restoring
  only the old hard-coded primary kind: 4 FAIL with the expected
  `openai-codex` versus `openai-codex:secondary` mismatch; the primary-only
  regression still passed. The fixed source was restored byte-for-byte.
- `cargo test --locked -- --quiet`: 1036 PASS, 18 intentionally ignored
  live probes, 0 FAIL.
- `npm test`: 79 files / 1365 tests PASS, including 4 new account-state
  consumer cases and the existing source/updater security guardrails.
- `npm run build`: PASS (TypeScript and Vite).
- `cargo check --locked --all-targets --features custom-protocol`: PASS;
  three dead-code warnings in unchanged `usage_intelligence`, `zcode_plans`
  and `zcode_reset` code.
- Existing `scripts/secret-scan.mjs` patterns: PASS over 9 files (built
  frontend, Tauri config and all five changed files). The signing-key
  needle call was disabled in memory for this run, so no local signing
  key was read. The tracked scanner itself was not changed.

