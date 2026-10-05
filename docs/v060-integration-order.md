# v0.6.0 Release Candidate — Integration Order (proposed before integration)

Written before any lane was merged into `release/v0.6.0-rc`, per the Phase 0
requirement. All SHAs verified against `origin` after `git fetch --prune`
on 2026-09-30.

## Phase 0 — authoritative map

### Verified source heads

| Branch | Head | Relation to base |
|---|---|---|
| `release/v0.5.0-rc` (BASE) | `34bf0e4a` | — |
| `integration/v0.6-core-rehearsal` | `b0978b97` | linear descendant of base (28 commits) |
| `fix/v0.6-settings-persistence` | `79417734` | rehearsal + 4 commits (sibling A) |
| `feature/v0.6-provider-identity-hardening` | `f206da2c` | rehearsal + 3 commits (sibling B) |
| `feature/v0.6-provider-preferences` | `334a429e` | base commit `06f6629` (inside v0.5.0-rc) + 3 commits |
| `feature/v0.6-quota-perspective` | `6886e605` | base head + 4 commits |
| `feature/v0.6-native-acceptance-harness` | `a6c37e94` | base head + 2 commits |

Merge bases vs base: settings×identity = `b0978b97` (they are siblings,
neither contains the other); settings-lane head (03:23) predates
identity-lane head (03:55) chronologically.

### Duplicate commits

Patch-ID comparison proved the rehearsal branch already contains the seven
core lanes; several copies were adapted during rehearsal (diagnostics
canonical-health projection, updater reconciliation, provider-correctness
docs), so the original lane branches are superseded and must NOT be merged
again:

- runtime-status-contract: 3/3 identical
- history-trends: 4/4 identical
- persisted-last-good: 3/3 identical
- notification-recovery: 2/2 identical
- redacted-diagnostics: 0/4 identical (adapted by rehearsal resolutions #1, #2, #7, #10–#12)
- production-updater: 2/3 identical (adapted by resolution #4)
- provider-correctness: 2/3 own commits identical (docs commit adapted)

### Multi-lane file overlap

- settings lane (frontend: `settings.ts`, `useSettings.ts`, floating
  prefs) × identity lane (Rust: `runtime.rs`, `codex.rs`, `zai.rs`,
  `grok.rs`, diagnostics) → **zero file overlap**, clean sibling merge expected.
- provider-preferences × quota-perspective: `App.tsx`, `useSettings.ts`,
  `settings.ts`, `FloatingQuotaWindow.tsx`, `styles.css`, dashboard tests —
  the expected real conflict zone.
- settings lane already contains superset versions of
  `providerPreferences.ts` (+ at-least-one-visible self-heal) and an
  identical `quotaPresentation.ts`; its hardened `settings.ts` already
  carries `providerPreferences` + `quotaPerspective` fields and the safe
  read-modify-write `mergeSettingsWithRaw`.

### Excluded lanes (verified, not invented)

- `feature/v0.6-dashboard-tray-light` — local branch exists at `34bf0e4`
  (identical to base): **zero commits, never completed → excluded.**
- All v0.7 lanes (execution provenance/runs/receipts, codex reset credits,
  usage analytics/UI, harness telemetry, local data controls) → excluded.
- Floating Hide vs Off: shipped in the v0.5.0-rc base
  (`floatingWindowPrefs.ts` with `floatingBarEnabled`, component wiring);
  persistence hardened by the settings lane (`ebc0288`). Included via base + lane 3.

## Proposed integration order

1. `release/v0.5.0-rc` (`34bf0e4a`) — cut branch.
2. merge `integration/v0.6-core-rehearsal` (`b0978b97`) — fast-forward;
   brings runtime-status-contract → history-trends → persisted-last-good →
   notification-recovery → redacted-diagnostics → production-updater →
   provider-correctness plus the 12 documented semantic resolutions
   (provider-correctness lands after production-updater, its git parent).
3. merge `fix/v0.6-settings-persistence` (`79417734`) — fast-forward;
   safe read-modify-write settings persistence, floating state hardening,
   migration fixtures.
4. merge `feature/v0.6-provider-identity-hardening` (`f206da2c`) — merge
   commit; identity stamping + unified five-provider diagnostic.
5. merge `feature/v0.6-provider-preferences` (`334a429e`) — merge commit;
   keep hardened persistence, add preference UI.
6. merge `feature/v0.6-quota-perspective` (`6886e605`) — merge commit;
   Used|Remaining presentation, severity stays usedPercent-based.
7. merge `feature/v0.6-native-acceptance-harness` (`a6c37e94`) — merge commit.
8. version bump 0.6.0 (package.json, package-lock.json, Cargo.toml,
   Cargo.lock, tauri.conf.json), v0.7 leakage scan, full validation.

Rationale: steps 2–4 reproduce the validated rehearsal lineage exactly
(its tree was the validated state: 311/311 npm, 340 cargo tests) with the
two accepted remediation lanes applied on top; steps 5–7 add the accepted
product lanes, whose conflicts are resolved against the already-canonical
runtime/settings contracts.
