# Reliability and data integrity remediation contract

Date: 2026-10-29. Scope: local source candidate on `codex/settings-persistence-hotfix`, based on `452e6e811fc8db4b9e281bf0a36df923bfb798bb`. No installed runtime or public release changed.

## Findings and evidence

| # | Disposition | Contract and regression evidence |
|---|---|---|
| 1 | Implemented | History and Usage Intelligence serialize mutation plus replacement per store. Unique same-directory temporary files use create-new, sync, then rename; Unix also syncs the directory. History clear writes the empty state before clearing memory. Concurrent record/clear, failed clear and write/retry tests cover ordering and disk equality. |
| 2 | Implemented | One collector flight per store; generation invalidation rejects results after clear, disable or rebaseline. `invalidated_scan_cannot_restore_cleared_or_disabled_state` and the blocked-coordinator test cover cancellation and overlap. Repeated triggers during a flight are dropped; the next scheduled refresh retries. |
| 3 | Implemented | IPC rejects failed durable writes. Usage Intelligence rolls events/cursors back to its last committed snapshot. Account/wallet preferences commit memory after disk success. History retains failed observations in memory for retry, explicitly returning `persisted: false`; import IPC rejects that outcome. Partial/invalid frontend migration sources remain intact. |
| 4 | Implemented | Retention frees capacity first. Capacity rejection retains the previous source cursor and fingerprint is cleared, enabling replay/deduplication. `ScanCeiling` reports uncertain coverage. `capacity_retains_backlog_then_retention_releases_it` exercises the 200,000-event boundary and recovery. |
| 5 | Implemented | Malformed supported data is backed up to a unique recovery file before normalization. Semantically invalid events and preference fields also preserve the original. Unknown schemas stay untouched and write-blocked; unknown Usage Intelligence/notification consent is disabled. Tests cover future schemas, malformed data and later saves after semantic repair. |
| 6 | Implemented with operational limits | SQLite Online Backup reads a source connection opened READ_ONLY into memory, then requires quick_check=ok. There are no copy pairs, SQL writes or checkpoint commands against a source. WAL writer/checkpoint/rotation and malformed-source fixtures pass. Backup deadline: 2 seconds; busy wait: 100 ms; snapshot ceiling: 256 MiB. Real provider database compatibility remains a native acceptance check. |
| 7 | Implemented | Rust is authoritative for wallet monitoring and quota notifications. Both WebViews hydrate using `get_operational_settings`; initialization/storage events do not invoke setters. Serial writes, per-wallet partial merges and revisioned events reject delayed snapshots. `useSettings.operational.test.tsx` covers two windows, missing storage, disabled choices, stale replies and retry. |
| 8 | Implemented safe isolation; Qwen capability limited | Z.ai/OpenCode Go/Grok use domain-separated SHA-256 fingerprints of complete credentials. Display suffixes never determine ownership. Credential-dependent Grok and Grok Bot caches reject foreign/unknown bindings. Qwen has no authoritative account identifier: current quota remains available, but new history and prediction attribution are disabled. Existing history is preserved; old weak identities are not relabeled. |
| 9 | Implemented | Quota publication no longer awaits Usage Intelligence scans. One detached flight and bounded database snapshots limit work. Blocking-collector regression proves quota completion is independent. A controlled 60,000-row fixture measured synchronous scan 270.8709 ms versus detached quota completion 1.4818 ms (total background completion 442.9712 ms); this is one fixture run, not a performance SLA. |
| 10 | Implemented, offline verified | All publications share one concurrency group. Installer, signature and feed are staged in a draft, checked against build provenance, downloaded and byte-compared. One activation request changes public visibility/latest status only after verification. Six interruption/channel/order tests run without network. Failed drafts are left inspectable; retries never edit public releases. |

## Security boundaries

- Export IPC requires the exact directory selected by the calling window's native picker. A picker cancel revokes that window's grant; selecting another directory replaces it. New files use create-new. Existing files require native overwrite confirmation even when IPC supplies `confirm_overwrite`. Non-regular, symlink and Windows reparse destinations are rejected. Grants last for the process/window-label lifetime; there is no arbitrary-directory IPC permission.
- Credential-bearing SQLite snapshots exist in memory only and are dropped with their connection. No source database or live credential store was used by the validation fixtures. Normal SQLite read locks/WAL shared-memory coordination are possible; the guarantee is absence of source SQL writes, not absence of all reader coordination.
- Source code and build outputs passed a pattern-based secret scan with the scanner's private-key lookup redirected to an absent fixture profile. Production signing keys were not read. Previous signing-passphrase exposure remains a separate unresolved release-readiness concern.

## Persistence and compatibility

Existing file names and schemas remain. Aggregation DTOs gain `persistenceAvailable` and `lastWriteSucceeded`; older DTO fixtures remain usable by the frontend. The former means the store passed schema/recovery checks, not a promise that the next filesystem write will succeed. The latter describes the last attempted write.

History may remain available in memory after a failed ordinary recording write; import and clear never acknowledge that as durable success. Retrying records writes the complete merged state. Usage Intelligence must roll back both events and cursor on failure to keep replay possible. Unknown-schema files require compatibility/recovery review rather than automatic destructive repair.

Credential rotation creates a new history partition, even for the same real account. Legacy suffix-based partitions remain on disk but do not match new credential fingerprints. Qwen historical records remain available for inspection/export, but are excluded from current prediction input. Codex rollout attribution stays disabled as in the baseline.

## Residual risks and validation limits

- MEDIUM: Qwen account identity is unavailable; history/prediction isolation is implemented by withholding attribution. Restoring that capability requires an authoritative identity source.
- MEDIUM: authenticated providers, native picker/overwrite dialogs, installed runtime behavior, signing and public updater activation were not exercised. Eighteen Rust live tests remain ignored. No replacement installer/signature was generated.
- LOW: clearing history invalidates pending suspicious-drop confirmations before attempting disk clear. A failed clear keeps historical data but discards those transient confirmations; subsequent cycles reclassify them.
- LOW: shutdown does not forcibly abort a SQLite native call. Generation fencing prevents invalid commits; backup work is bounded and temporary databases are not retained. OS power-loss durability beyond file sync/rename depends on filesystem semantics; no Windows power-cut experiment was performed.
- LOW: repository-wide `cargo fmt --check` fails on pre-existing formatting, including unchanged `antigravity.rs`. New standalone Rust modules pass rustfmt. No repository-wide formatting rewrite is included.

## Candidate and lifecycle

The pre-existing `artifacts/release-v0.8.24/` installer and signature are historical evidence and are superseded for this source. They were not overwritten. Version remains 0.8.24; this work is not a release.

An implementation self-review examined the integrated diff and corrected blank identity admission, invalid-event recovery preservation, preference retry short-circuits, and reparse rejection. This is not an independent exact-head review or a trusted review receipt. Independent exact-head review and Human Acceptance precede any installation/publication authorization.

Final validation, exact commits and local log hashes are recorded in `artifacts/reliability-remediation/HANDOFF.md` (ignored local artifact). Reproducible checks: `npm test -- --reporter=dot --maxWorkers=2 --minWorkers=1`, `npm run build`, `cargo test --locked --manifest-path src-tauri/Cargo.toml --bin rate-limits -- --quiet`, `cargo check --locked --all-targets --manifest-path src-tauri/Cargo.toml`, and `git diff --check`.


## Validated implementation revisions

- Runtime/data/settings/security: `02eab88cddc83c6bbcb1a2f5afd0c60eb28bb91d`.
- Draft publication/workflow: `80029088c9ee60797396eedf073fa742ccc6c2e4`.
- Full Rust suite: **1090 passed, 0 failed, 18 ignored live tests**.
- Full frontend suite: **82 files, 1414 tests passed**, including six publication tests.
- Production frontend build and `cargo check --locked --all-targets`: PASS. Cargo check emits eight warnings (unused test-oriented compatibility entry points and existing unused diagnostic fields); there is no compiler error.
- New standalone Rust modules pass `rustfmt --check`; repository-wide formatting does not pass and is explicitly excluded from claims of a clean global format check.
- Pattern secret scan: PASS, six build/configuration files; existing release bundle read-only, real private-key comparison deliberately disabled. `node --check scripts/publish-release.mjs` and `git diff --check`: PASS.

These checks ran against the final source contents before commits. Subsequent changes are documentation only; exact source-tree equivalence is checked during closeout. No independent review receipt is claimed.
