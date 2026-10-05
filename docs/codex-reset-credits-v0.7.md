# Codex Banked Reset Credits — v0.7 Recovery-Credit Contract

Provider-specific capability: the Codex backend explicitly reports a banked
reset-credit balance. No other provider source in this repository exposes a
reset count, and none is inferred for them.

Evidence: `docs/research/reset-replenishment-discovery.md`
(`research/v0.7-reset-budget-discovery`, RESEARCH_COMPLETE 2026-09-30).
Live re-verification on this branch: 2026-09-30, team plan — WHAM usage
returned one Weekly window (43%, reset 2026-10-03T07:15:21Z) with
`rate_limit_reset_credits.available_count = 3` and a separate
applicability field of `0`.

## Terminology

| Term | Meaning | Example |
|---|---|---|
| Window reset | The next boundary timestamp attached to one quota window (FIXED_RESET). A known future time, not a count. | Weekly window resets 2026-10-03T07:15:21Z |
| Rolling recovery | Capacity that regenerates continuously inside a rolling window (e.g. OpenCode Go `rolling`). No boundary restores everything at once. | 5-hour rolling percent declining without a reset event |
| Banked credit | An explicitly reported balance of granted reset credits held for the account (REPLENISHMENT). Usable only through a provider-side redemption, which this app never performs. | 3 banked reset credits |
| Currently applicable credit | The separate backend field stating how many banked credits apply to the current plan state right now. Unknown until reported; missing means unknown, never zero. | 0 currently applicable (observed) vs unknown (not reported) |
| Credit expiration | A per-credit grant deadline from the read-only reset-credit details endpoint. A deadline, not a scheduled reset event. Not consumed by this capability. | Credit expires 2026-10-04 (details endpoint only) |

The discovery's core finding, preserved end to end: **"3 banked reset
credits" is supported; "3 replenishments available now" is NOT supported**
(banked 3, applicable 0 on the observed account).

## Normalized contract

Rust `CodexResetCredits` (`src-tauri/src/codex.rs`) → runtime
`ProviderUsageDto.reset_credits` → TS `CodexResetCredits`
(`src/types.ts`). Wire (camelCase, optional fields omitted):

```text
resetCredits: {
  bankedCredits: 3,             // explicit balance only; never derived
  currentlyApplicable?: 0,      // separate field; absent = unknown
  checkedAt: "2026-09-30T...",  // observation time (freshness budget below)
  source: "codex-wham-usage"    // provenance; only value today
}
```

The field lives beside quota windows, never inside them: window-count
semantics (`limits[].resetAt`) are untouched, and no code path counts
windows, resets, history transitions, or usage drops as credits. There is no
`expiresAt`: per-credit expirations exist only in the reset-credit details
endpoint, which this capability deliberately does not call — smallest
surface, usage payload alone.

## Source and boundaries

- Read-only `GET https://chatgpt.com/backend-api/wham/usage` with the
  existing local Codex login (`~/.codex/auth.json`), same credential and
  account scope as the quota fetch. No second endpoint, no inference
  request, no credential refresh ownership, no mutation, no redemption, no
  automatic use of a credit.
- Failure independence: credit fields parse best-effort. Windows available
  with credit data missing or malformed → quota Live, capability
  unavailable. A quota parse failure is still a quota failure (no
  observation exists at all).
- Diagnostics carry only `bankedCredits`, `currentlyApplicable`,
  `checkedAt` (`src-tauri/src/diagnostics.rs`). No raw payload, no
  identifiers.

## Account isolation

Credits bind to the same masked account identity as the Codex quota windows:
the served response `account_id`, falling back to the stored credential's
account scope. Only the masked hint (`chatgpt:<masked 8-char hint>`, e.g.
`ChatGPT ••ccta`) crosses to the UI — raw ids never leave Rust.

- Served account contradicts the stored scope → credits unavailable (windows
  stay bound to what was actually served).
- No observed account → credits unavailable (unbound counts never read as
  the current account's balance).
- The shared runtime last-good guard compares masked identities, so a
  credential swap drops retained windows AND credits together; post-credential
  failures carry the attempted identity (same pattern as OpenCode Go,
  MIC-297 follow-up).
- Persisted last-good never stores credits: cold-start hydration always
  re-observes before the capability reads as available.

## Freshness (TTL)

`RESET_CREDITS_TTL_SECS = 900` (15 minutes), enforced in Rust
(`reset_credits_fresh`, retention trim) and TypeScript
(`isResetCreditsFresh`, `RESET_CREDITS_TTL_MS`).

Rationale: the usage payload is re-queried live every refresh cycle
(default 5 minutes), and a banked balance can be consumed at any time by a
redemption outside this app — visible only on the next fetch. Carried-forward
caching without field-level freshness (e.g. the OpenCodex `resetCredits`
cache field) is explicitly untrusted by the discovery. A count older than
roughly three default cycles must therefore stop reading as current rather
than risk presenting a redeemed balance as banked. Retained snapshots keep
their original `checkedAt` so age is always computable; 5-minute
future tolerance covers clock skew (same tolerance as the last-good store).

## Display contract

Implemented in `src/lib/resetCredits.ts` (`formatResetCredits`, Codex
provider only — other providers can never render a credit line):

- `3 banked reset credits` / `1 banked reset credit` (explicit zero stays
  explicit: `0 banked reset credits`)
- `1 credit currently applicable` / `0 credits currently applicable`
- `No credit applicability information` (missing applicability only)
- Unavailable (absent, stale, invalid, non-Codex): render nothing

Forbidden unless the source explicitly proves the concept: `3 resets left`,
`3 resets available`, or any wording claiming a credit can be used now.
Card rendering beyond this helper is future work; the helper pins the exact
strings.

## Tests

- Rust `src-tauri/src/codex.rs`: 0/1/multiple banked, applicability
  0/1/missing, missing banked, malformed counts (string/float/negative/bool/
  overflow), non-count sources (windows, balances, earned totals, detail
  rows), account mismatch, unbound identities, stale/future stamps, wire
  redaction, quota-ok/credits-missing independence.
- Rust `src-tauri/src/runtime.rs`: normalization propagation, Live without
  credits, retention trim (fresh kept, stale dropped, non-Codex untouched).
- Rust `src-tauri/src/last_good.rs`: hydration never carries credits.
- Rust `src-tauri/src/diagnostics.rs`: projection carries counts and time
  only.
- TS `src/lib/resetCredits.test.ts`: wording, pluralization, unknown vs
  zero applicability, non-Codex refusal, malformed/stale handling, and a
  wording guard against "resets left/available" phrasing.
- Live (ignored, needs local login): `codex::tests::live_fetch_returns_windows`
  prints counts only; verified 2026-09-30 (banked 3, applicable 0).

## Validation on this branch

- `npm test`: 321 passed / 31 files
- `npm run build`: PASS
- `cargo test --locked`: 356 passed / 5 ignored / 0 failed
- Live probe: PASS (banked 3, applicable 0, team plan)
- `node scripts/secret-scan.mjs`: PASS
