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
| Credit expiration | Optional nearest deadline proven from complete, same-request reset-credit details. A deadline, not a scheduled window reset. | Next credit expiry Oct 4, 2026, 12:30 PM (system locale/time zone) |

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
  nextCreditExpiresAt?: "2026-10-04T12:30:00Z", // complete details only
  checkedAt: "2026-09-30T...",  // observation time (freshness budget below)
  source: "codex-wham-usage"    // provenance; only value today
}
```

The field lives beside quota windows, never inside them: window-count
semantics (`limits[].resetAt`) are untouched, and no code path counts
windows, resets, history transitions, or usage drops as credits. There is no
public credit array or per-credit metadata. `nextCreditExpiresAt` is omitted
unless the optional details read proves a nearest deadline.

## Source and boundaries

- Read-only `GET https://chatgpt.com/backend-api/wham/usage` with the
  existing local Codex login (`~/.codex/auth.json`), same credential and
  account scope as the quota fetch. This remains the sole count/applicability
  authority. No inference request, no credential refresh ownership, no mutation, no redemption, no
  automatic use of a credit.
- After successful account-bound usage, a positive balance with an explicit
  matching local account scope and a known full user/workspace identity may
  trigger ONE optional GET of
  `https://chatgpt.com/backend-api/wham/rate-limit-reset-credits`. It uses the
  same already-loaded bearer and `chatgpt-account-id`, the existing Rust client,
  no redirects, no retries, and a 5-second total deadline including the body.
  Body cap: 128 KiB; row cap: 256. Exceeding either cap rejects the whole detail
  list; rows are never truncated to derive a deadline. Balances above the row
  cap skip this optional read. Quota/count success survives all detail failures,
  including 401/403/429, timeout and schema drift.
- **Fragile optional dependency:** this is an internal endpoint used by official
  `openai/codex`, NOT a guaranteed supported external API. Public source evidence
  pinned to `rust-v0.169.0` (inspected unauthenticated on 2026-11-17):
  [HTTP operation and route](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/backend-client/src/client/rate_limit_resets.rs),
  [snake_case detail model](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/backend-client/src/types.rs),
  [protocol warning that details may be capped](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/app-server-protocol/src/protocol/v2/account.rs).
  This candidate does not introduce App Server, a transport/runtime, or a binary.
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
- The shared runtime last-good guard compares full safe user/workspace digests, so a
  credential swap drops retained windows AND credits together; post-credential
  failures carry the attempted identity (same pattern as OpenCode Go,
  MIC-297 follow-up).
- Persisted last-good never stores credits: cold-start hydration always
  re-observes before the capability reads as available.

## Expiry proof (local implementation candidate, 2026-11-17)

- Usage must already bind its explicit served account to the explicit local
  scope. The full safe identity digest must still match after the detail read,
  re-read from the exact same resolved home; unreadable or swapped identity
  withholds expiry. A returned detail `account_id`, if present, must agree too.
  Primary and CoWork reads use their own configured home, bearer and scope.
- `available_count`, when present in details, must be a strict integer equal
  to WHAM's banked count. The number of unique `available` credit rows must
  ALSO equal that count. A capped/missing/null/empty list with a positive
  balance, duplicates, missing IDs, unknown/missing status or reset type,
  or malformed rows withholds the entire global-nearest claim.
- Only `available` + `codex_rate_limits` rows can contribute a deadline.
  Known `redeeming`/`redeemed` rows do not contribute; unknown types anywhere
  reject the proof. IDs are used only to reject duplicates within Rust.
- Dates must be positive RFC3339 timestamps with an offset or integer Unix
  seconds (no strings-as-numbers, floats, milliseconds guessing or relative
  time). Grant must be known and not future; expiry must be future and not
  before grant. Unknown/missing/malformed/elapsed expiry rejects the proof.
  Upstream explicitly models null as nonexpiring, never a numeric deadline;
  this conservative candidate also withholds the global deadline if any
  available row has null expiry, even if other rows expire.
- The optional date is UTC RFC3339 and shares the original usage `checkedAt`:
  it is enriched only during that same request, while still fresh. Each
  successful count-only refresh starts with no expiry, so old detail data
  cannot acquire a newer timestamp. Existing retention preserves the original
  observation; disk hydration never carries credits. Diagnostics retain only
  existing counts/time, omitting expiry and all raw detail metadata.

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
- Existing floating Codex Reset Bank's third line only: `Next credit expiry
  <localized date and time>` for a verified, fresh, future deadline; otherwise
  `Expiry not reported`. Uses the system locale/time zone via `Intl`, like
  current app date formatting. At render time an elapsed deadline reverts to
  unknown, without asserting that a credit has expired. Shell/CSS/compact bar,
  other cards and quota-window reset wording remain unchanged.

Forbidden unless the source explicitly proves the concept: `3 resets left`,
`3 resets available`, or any wording claiming a credit can be used now.
The existing expanded floating card uses these lines; compact-bar rendering
is unchanged.

## Tests

- `src-tauri/src/codex_credit_expiry_tests.rs`: complete vs capped/duplicate/
  oversized lists, strict counts/status/type/dates, null and missing times,
  request eligibility, identity changes, two synthetic configured homes,
  wire redaction, count-only refresh, actual bounded loopback HTTP and body
  timeout. No production endpoint is contacted by these fixtures.
- Existing runtime identity fixtures explicitly disable CoWork for single-account
  scenarios, so no default credential home participates in synthetic checks.
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

## Historical v0.7 validation (not evidence for the expiration candidate)

- `npm test`: 321 passed / 31 files
- `npm run build`: PASS
- `cargo test --locked`: 356 passed / 5 ignored / 0 failed
- Live probe: PASS (banked 3, applicable 0, team plan)
- `node scripts/secret-scan.mjs`: PASS
