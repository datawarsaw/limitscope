# LimitScope v0.8.24 — Release Notes

Local Windows release candidate. Not installed or published. Human Acceptance
of the persistence hotfix remains pending.

## Changes

- Retains the Settings Redesign baseline: General, Providers & Wallets,
  and Data & Privacy sections.
- Preserves Rust-owned OpenAI/Codex account aliases, homes and monitoring
  flags when a WebView attaches or Settings reopens. Missing, stale or
  corrupt WebView cache data no longer writes defaults into Rust.
- Loads the persisted Usage Intelligence opt-in from Rust without changing
  it on startup or storage events. Explicit account edits and opt-in
  changes continue to persist through the existing commands.
- Serializes account edits within one hook and prevents late initialization
  replies from replacing explicit edits or an explicit preference reset.

## Compatibility and acceptance

Storage formats, persistence keys, account/session isolation and updater
trust configuration are unchanged. This release does not restore values
already overwritten by an older version. Concurrent explicit edits from
separate windows remain outside the hotfix concurrency guarantee. Failed
IPC reads can temporarily leave cached values visible without writing
those values to Rust.

The product source is the Settings Redesign release candidate
`10e7325b1f4636fc82ce1e8859053bdcf4739dfd` plus persistence hotfix
`1d6c96a03d7559ff48740633c68a416e260541f3`. Release preparation changes only
version metadata and release documentation. No Disable / Archive or Codex
Reset Bank is added.

Native UI acceptance of 0.8.24, installation and publication require a
separately authorized lifecycle run. No native UI verification of this
candidate is claimed by these notes.
