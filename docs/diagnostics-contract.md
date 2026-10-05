# Redacted diagnostics contract

Schema version: `1`

## Purpose

`Export diagnostics` writes one small JSON support bundle for provider refresh,
stale data, cooldown, runtime timing, history availability, notification state,
and build identification. The filename is the neutral internal form
`runtime-diagnostics-<timestamp>.json` because the open v0.5/v0.6 lanes still
carry both LimitScope and Rate Limits branding.

## Included fields

- `schemaVersion`, `generatedAt`
- `app`: product name, version, platform, architecture, app identifier
- `environment`: OS family, app-data health, runtime provider count
- `runtime`: snapshot sequence, cycle timestamps, last usable-data timestamp,
  refresh interval, scheduler state, follow-up state
- `providers`: canonical normalized health (`live`/`stale`/`unknown`/`cooldown`/
  `error`/`unavailable`, per the runtime status contract) with its derived
  legacy status, freshness, masked account label, quota windows,
  cooldown state, last success, and normalized last error
- `history`: revision, total/oldest/newest observation metadata, and per-account
  window counts only
- `notifications`: enabled state and bounded dedup/current-cycle counts only
- `settings`: theme, refresh interval, launch-at-startup, notification enabled

## Explicitly excluded

Access tokens, refresh tokens, API keys, JWTs, cookies, Authorization headers,
raw provider response bodies, raw exception dumps, provider credential stores,
full quota history, notification payload history, machine username, home
directory, arbitrary absolute paths, and hardware inventory.

## Redaction guarantees

`src-tauri/src/diagnostics.rs` is the only serialization boundary. It builds a
dedicated safe DTO from the runtime's normalized provider DTOs and structured
failures. Raw provider response types cannot enter that API. Provider error text
is discarded and replaced with a fixed local message; category, code, status,
and timestamp remain separate normalized fields. Every remaining free-form
string passes a bounded sanitizer that removes credential labels, common token
forms, JWTs, long credential-shaped values, session identifiers, and absolute
paths. Invalid optional timestamps and malformed windows are omitted or
normalized safely.

The webview sends only a typed, whitelisted settings record. Rust owns the save
dialog and output path, writes atomically through a temporary file, bounds the
bundle at 2 MiB, and returns only `saved`/`cancelled` plus the selected filename.

## Support workflow

1. Reproduce the issue without deliberately consuming provider quota.
2. Open `Settings` and choose `Export diagnostics`.
3. Save the JSON bundle through the system dialog.
4. Attach that single file to the support request. It needs no archive.
5. Treat the file as internal support data, while recognizing that the schema
   excludes credentials and raw provider payloads by construction.

