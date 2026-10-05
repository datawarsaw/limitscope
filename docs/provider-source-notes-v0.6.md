# Provider Source Notes — OpenCode Go & Antigravity (v0.6 correctness)

Written for the `feature/v0.6-provider-correctness` investigation (2026-09-29/30).
Evidence tags: `[LIVE]` verified by executing a real fetch on this machine,
`[LOCAL]` verified from local file structure (values redacted), `[CODE]` pinned
by tests in this repository.

No credential values are in this document. The key/account tails quoted are
the same masked hints the app itself renders.

---

## OpenCode Go

- **Credential source** — the single `opencode-go` API key in OpenCode's
  `~/.local/share/opencode/auth.json`, re-read every cycle (stateless; nothing
  account-specific is cached anywhere) `[CODE]`.
- **Quota source** — `GET https://opencode.ai/zen/go/v1/usage` with that key
  as bearer token `[CODE]`.
- **Account-selection rule** — there is nothing to select: OpenCode stores one
  `opencode-go` key, so the card always describes exactly that credential. The
  `account.identity` (`key:<last-4>`) is the same tail the OpenCode console
  masks with `[CODE]`.
- **Window semantics** — `usage.rolling → "5-hour"`,
  `usage.weekly → "Weekly"`, `usage.monthly → "30-day"`. The upstream
  `percent` field is **used** percent (0–100), never remaining: verified live
  on 2026-09-29, where the endpoint reports `status: "rate-limited"`
  together with `percent: 100` on the same window `[LIVE]`. No
  remaining→used conversion is performed because none applies.

### The 30-day "100%" report — root cause

LimitScope showed 0% / 0% / 100% (limit reached) while the reference monitor
showed the remaining OpenCode account at 0% / 5% / 54%. Both numbers are true
— for **different accounts**:

| Source | Key tail | 5h | Weekly | 30-day | resetsAt (30d) |
| --- | --- | --- | --- | --- | --- |
| OpenCode `auth.json` (what LimitScope reads) | `avmF` `[LIVE]` | 0% | 0% | **100%** (`rate-limited`) | 2026-10-03T20:48:17Z |
| Reference monitor's stored account | `yZw0` `[LIVE]` | 0% | 5% | **54%** | 2026-10-15T13:12:08Z |

The removed OpenCode account survives in **OpenCode's own `auth.json`** (it is
rewritten only by `opencode auth login`), not in any LimitScope state.
Normalization, field direction, reset parsing, and attribution are all
correct; no code change can make LimitScope display an account whose
credential it does not hold. Pointing LimitScope at the remaining account
requires re-authenticating OpenCode Go (`opencode auth login`) so
`auth.json` holds that key.

### Fix shipped in this branch

The one real defect found: on a failed refresh, the runtime retained the
last-good snapshot of **whatever account succeeded last**. After a credential
change, the old account's data could therefore survive (marked errored but
with its windows and attribution). The OpenCode backend now stamps every
post-credential failure with the attempted account's masked identity, and the
runtime drops last-good retention when that identity differs from the
retained snapshot's attribution — a bare error is shown instead `[CODE]`.

---

## Antigravity

- **Credential source** — the active account's OAuth `refreshToken` +
  `projectId` in the OpenCode Antigravity plugin cache
  (`~/.config/opencode/antigravity-accounts.json`), the same file and account
  selection as before; the token never leaves the Rust process `[CODE]`.
- **Quota source (primary, live)** — Google's own Cloud Code Assist
  accounting endpoint, the same source the Antigravity IDE and the reference
  monitor use: `POST
  https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary`
  with `{"project": <projectId>}`, after a `refresh_token` grant against the
  Google OAuth token endpoint using the public Antigravity desktop client
  identifiers (configuration, not user secrets). A 403 retries once with the
  plain `antigravity/1.0` client UA `[LIVE]`.
- **Quota source (fallback)** — the plugin's `cachedQuota`, with the exact
  v0.5 semantics (24-hour freshness threshold, stale verdict, windows shown
  regardless, resets never re-invented) `[CODE]`.
- **Account-selection rule** — the plugin's `activeIndex` (the account the
  plugin last used), falling back to the first stored account; the same
  selection feeds both the live fetch and the cache fallback `[CODE]`.
- **Window semantics** — the response's `groups[].buckets[]` carry one bucket
  per family per window (`5h`, `weekly`), each with `remainingFraction`
  (0–1 **remaining**) and `resetTime`. Groups are classified by their
  display name/description: Gemini → the `gemini` family; the "Claude and
  GPT models" group (Claude Opus/Sonnet + GPT-OSS) → the `non-gemini` family
  — the same bucket the cache called `non-gemini`. Used percent is the
  explicit conversion `(1 − remainingFraction) × 100`, clamped `[LIVE]` +
  `[CODE]`. The reference monitor labels these buckets `Gem`/`Cla`;
  LimitScope's `Gemini`/`Claude/Grok` labels denote the same Google buckets,
  so the mismatch was never a bucket-mapping error and no rename was made.
- **Reset semantics** — `resetTime` is passed through exactly as Google sent
  it (ISO-8601 validated); a live snapshot stamps `sourceUpdatedAt` with the
  fetch time and reads `fresh`, so a future reset renders as a countdown. The
  stale cache fallback keeps the old verdict rules and never fabricates a
  next reset `[CODE]`.

### Root cause of the "Reset time passed" mismatch

LimitScope was a passive reader of the plugin cache — but the cache's writer
(the plugin inside OpenCode) had stopped running; its last write on this
machine was **2026-09-18**. Every number LimitScope showed (Gemini 10%,
Gemini Weekly 5%, Claude/Grok 14%, Claude/Grok Weekly 72%) was that Sep-18
snapshot, and every one of its reset times had already passed, so the card
read "Reset time passed" and `stale`. The reference monitor fetches Google
live and therefore showed current-cycle values with future resets. LimitScope
now shares that live source; the cache remains only as an offline fallback.

### Live comparison (2026-09-29 23:13Z, same machine) `[LIVE]`

| Window | Reference monitor (user-visible) | Raw Google source (this fetch) | LimitScope normalized | Match |
| --- | --- | --- | --- | --- |
| Gem / Gemini | 14% used | `gemini-5h` remaining 0.991144 → 0.89% used, reset 2026-09-30T03:49:11Z | Gemini 0.89%, reset future | PARTIAL¹ |
| Gem (Weekly) / Gemini Weekly | 11% used | `gemini-weekly` remaining 0.89034826 → 10.97% used, reset 2026-10-04T21:32:12Z | Gemini Weekly 10.97%, reset future | YES² |
| Cla / Claude/Grok | 0% used | `3p-5h` remaining 1 → 0% used, reset 2026-09-30T03:52:14Z | Claude/Grok 0%, reset future | YES |
| Cla (Weekly) / Claude/Grok Weekly | 0% used | `3p-weekly` remaining 1 → 0% used, reset 2026-10-06T22:52:14Z | Claude/Grok Weekly 0%, reset future | YES |

¹ The 5-hour window churns fastest; the reference screenshot predates this
fetch by several hours of the same 5-hour cycle. Both apps now read the same
endpoint, so transient disagreement within a window is timing, not semantics.
² The reference value was captured earlier in the same weekly window; the
normalized conversion (11% vs 10.97%) agrees within reading time.

### Stale/last-good verification `[CODE]`

Live success replaces the retained snapshot each cycle; a cache-read failure
retains last-good marked errored with its original `checkedAt`; the
Antigravity live path degrades to the cache fallback (stale verdict by
snapshot age) rather than surfacing an error, and a malformed live payload
(empty parse) is treated as no-live-data, never as truth.

---

## Cross-provider identity hardening (v0.6 R1, 2026-09-30)

The provider source matrix (`research/v0.6-provider-source-matrix`, section 11)
rated the missing attempted-account stamping on the Codex / Z.ai / Grok failure
paths as risk **R1 (MEDIUM)**: a failure landing after a credential swap could
retain the previous account's last-good windows (always error-labeled, never
live, but still the wrong account). This section records the remediation,
which ports the OpenCode Go `with_identity_hint` pattern to the remaining
backends. OpenCode Go's existing account guard and Antigravity's accepted
WEAK attribution are unchanged.

### The last-good rule (runtime contract, all five providers)

After credential resolution, the runtime's retention guard
(`apply_failure_at` in `runtime.rs`) compares the failure's stamped identity
with the retained snapshot's `account.identity`:

- **same attempted identity** — retained last-good stays eligible per the
  normal error policy (health `error`, original `checkedAt`).
- **different attempted identity** — the old last-good is dropped; a bare
  error entry (or `unavailable`, per the failure family) is shown instead.
- **unknown identity** (the credential could not be resolved at all) — the
  existing conservative behavior: retention unchanged.

Identities are never invented from provider labels; each backend derives
them from the credential it actually attempted, in the same masked shape the
success path renders on the card, so the two paths compare like-for-like.

### Identity strength by provider (after hardening)

| Provider | Identity shape | Strength | Derivation |
| --- | --- | --- | --- |
| OpenAI Codex | `chatgpt:<last 8 of tokens.account_id>` | **STRONG** (new) | `~/.codex/auth.json` `tokens.account_id` — the same identifier the request was already scoped with; no JWT decoded beyond the existing expiry check, no email, no token material. Guarded: ids shorter than 16 chars or with non-identifier tails get no hint. |
| Z.ai | `key:<last 4 of the key>` | **STRONG** (new) | The credential candidate ACTUALLY attempted: a refusal is stamped with the refused key's hint and advances; a non-credential failure is stamped with the in-flight key's hint; a success carries the winning key's hint (closing the matrix's R2 winning-key invisibility). Same mask convention as OpenCode Go; keys shorter than 12 chars get no hint. |
| OpenCode Go | `key:<last 4 of the key>` | **STRONG** (unchanged) | Reference implementation; guard pinned by the `opencode_last_good_*` runtime tests. |
| Antigravity | none (selection-indexed source label only) | **WEAK** (accepted for v0.6, unchanged) | Failures stay unattributed by design; retention keeps the conservative path. No Google identity is invented from extra personal data. |
| Grok | `xai:<first 8 of account id + …>` | **STRONG** on failures (new; success was already STRONG) | The resolved credential's store-agnostic masked account id (`credential.accountId` / `user_id` / JWT `sub` fallback — same derivation as the success card, so a store change with the same account is the same identity). The expired-credential verdict is stamped too: the expired credential was the one selected for the attempt. `credential_missing`/`credential_ambiguous` stay unattributed (nothing was attempted). |

### Failure attribution behavior

- Every failure **after** credential resolution carries the attempted
  identity (`network`, HTTP failures, schema errors, auth rejections). Every
  failure **before** resolution (`not_logged_in`, `credential_missing`,
  `*_not_installed`, …) carries none — nothing was attempted, so the
  conservative retention path applies.
- Z.ai's HTTP-200 envelope refusals (`auth_invalid`/`not_entitled`) advance
  to the next candidate **with the refused candidate's identity retained on
  the surfaced error**, so a final failure after several rejections names the
  last candidate actually tried — never an earlier one.
- The identities are masked and stable: tails/prefixes of account ids and
  keys only, length-guarded against leaking most of the material, never
  tokens, JWTs, emails, or authorization headers. They persist through the
  last-good store's existing `account.identity` plumbing (no schema change)
  and render through the same `account` field the cards already use.

### Known NONE/WEAK cases (documented, deliberate)

- Codex without a parseable/long-enough `tokens.account_id` → no identity,
  conservative retention.
- Z.ai keys too short to mask safely → no identity, conservative retention.
- Grok stores without any account id and an unparseable JWT → no identity.
- Antigravity → WEAK selection attribution only (accepted for v0.6).

### Verification `[CODE]`

The guard is pinned per provider by runtime tests (`codex_*`, `zai_*`,
`grok_last_good_*`, `antigravity_failures_stay_unattributed_*`,
`opencode_last_good_*` in `runtime.rs`), by backend tests for the identity
derivations and candidate attribution (`codex.rs`, `zai.rs`, `grok.rs`), and
by identity-format parity tests that would catch a drift between a backend's
stamped failure identity and the runtime attribution.
