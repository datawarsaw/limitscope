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

