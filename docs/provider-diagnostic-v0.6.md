# Provider acceptance diagnostic — v0.6

One operator-facing command reports all five production providers (Codex,
Z.ai, OpenCode Go, Antigravity, Grok) with redacted, normalized output:

```powershell
powershell -NoProfile -File scripts/provider-quota-diagnostic.ps1
```

The script drives the sanctioned read-only Rust probe — the ignored
`five_provider_diagnostic_report` test in `src-tauri/src/diagnostic_report.rs`
— through `cargo test --locked`, so the acceptance output comes from the exact
production fetch paths (same read-only credential resolution, same
normalizers) rather than a re-implementation. It performs no inference, no
deliberate quota spend, no account mutation, and no credential rotation; the
per-provider `live_fetch_returns_windows` tests remain available as
finer-grained probes.

## Safe output schema

Per provider, only these normalized fields are printed:

| Field | Meaning |
| --- | --- |
| `health` | Canonical health (`live`, `stale`, `unknown`, `cooldown`, `error`, `unavailable`) — the same `ProviderHealth` the runtime broadcasts. A source-absent failure is `unavailable`; every other failure is `error`. |
| `checkedAt` | When the probe observed the result (RFC-3339 UTC). |
| `freshness` / `sourceUpdatedAt` | Only where the source provides one (Antigravity's cached-source verdict/stamp today). |
| `credential` | The account/credential label the cards themselves render (e.g. `key ••avmF`, `account ••ff69dd62`, `7a2d5abe… · opencodex`), or the provider's source label when it has no card attribution (Antigravity's plugin-cache selection). |
| `account` | The masked identity (`key:avmF`, `chatgpt:ff69dd62`, `xai:7a2d5abe…`) — the same string the last-good guard compares. |
| windows | `label: used N% reset <timestamp>` — used percent 0–100 and the provider-reported reset, exactly as normalized. |

Failure rows stay useful and stay normalized: `health` (`unavailable` or
`error`), `reason` (the stable failure category — `credential_missing`,
`auth_invalid`, `network`, …), an optional bare `httpStatus`, and the masked
`account` hint of the credential that was attempted when it could be
resolved (`account: <none resolved>` otherwise). Structured error *messages*
and raw provider payloads are deliberately never printed.

Plan type is intentionally not part of the report: it is not a field of the
normalized snapshot DTO, and the diagnostic does not invent provider-specific
extensions (the Codex live test prints it for probe evidence).

## Redaction and scanning

Credentials are read only in-process by the Rust backends; the script never
touches credential stores. The probe output is redacted by construction, and
the captured stdout is scanned anyway — by the script (PowerShell) and, with
the same patterns, by unit test (`secret_scan_catches_credential_shapes` in
`diagnostic_report.rs`) — for:

- `Bearer` tokens and `Authorization` headers
- `sk-`-shaped API keys
- JWT-shaped three-part tokens
- `refresh_token` / `access_token` fields
- PEM private-key blocks

The run fails loudly if any pattern appears. The repo-wide source scanner
(`npm test` → `updaterSecurity.test.ts`, `node scripts/secret-scan.mjs`)
additionally pins that the scanner's own patterns in source never trip it.

Do not paste diagnostic output into tickets: it carries masked account
hints, which are user-specific. The script saves its output to a timestamped
file under `%TEMP%`; it is never written into the repository.

## Known boundaries

- Antigravity has no account identity (accepted WEAK selection attribution
  for v0.6); its row names the credential source without inventing one.
- Codex banked reset credits are intentionally NOT part of this diagnostic:
  they exist through a separate research probe
  (`research/v0.7-reset-budget-discovery`) and are not part of the production
  adapter. That v0.7 TEST boundary stays explicit.
- Providers whose credential cannot be resolved produce `unavailable` rows
  with `account: <none resolved>` — the diagnostic still reports one row per
  provider, never aborts on the first failure.
