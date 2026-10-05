# Provider Contract Audit — Rate Limits

Read-only audit of every provider against a single contract checklist.
**No application code was read with the intent to change it and nothing outside
`docs/provider-contract-audit.md` was created or modified.**

- Date: 2026-09-27
- Tree state: branch `fix/codex-unexpected-response` working tree, i.e. including the
  isolated Claude provider (`src-tauri/src/claude.rs`, `src/providers/claudeProvider.ts`)
  and its discovery doc.
- Method: full read of the contract surface (`src/types.ts`, `src/providers/*`,
  `src/lib/transientRetry.ts`, `src/lib/refreshCoordinator.ts`, `src/hooks/useProviderUsage.ts`,
  `src-tauri/src/{main,codex,zai,opencode_go,antigravity,claude}.rs`) and both discovery docs,
  plus the frontend provider tests. Claims below cite code; "live-verified" claims are
  quoted from the in-repo docs, not re-executed.

**Provider inventory at audit time:**

| Provider | Registry entry | Backend | Status |
| --- | --- | --- | --- |
| OpenAI / Codex | `CodexProvider` | `codex::get_codex_usage` | Production |
| Z.ai | `ZaiProvider` | `zai::get_zai_usage` | Production |
| OpenCode Go | `OpenCodeGoProvider` | `opencode_go::get_opencode_go_usage` | Production |
| Google Antigravity | `AntigravityProvider` | `antigravity::get_antigravity_usage` | Production |
| Claude Code | `MockProvider({ id: "claude" })` | `claude.rs` compiled but **not** in the invoke handler | Isolated |
| xAI Grok | `MockProvider({ id: "grok" })` | none | Mock only |

Grok has **no isolated implementation** (no `grok.rs`, no `grokProvider.ts` — discovery only,
`docs/provider-discovery.md` §3.3), so per the task definition it is **excluded** from the
detailed audit. It appears only in the readiness table.

---

## 1. Provider identity

| Provider | `id` (wire + registry) | `name` (display) | Command |
| --- | --- | --- | --- |
| OpenAI / Codex | `openai-codex` | `OpenAI / Codex` | `get_codex_usage` |
| Z.ai | `zai` | `Z.ai` | `get_zai_usage` |
| OpenCode Go | `opencode-go` | `OpenCode Go` | `get_opencode_go_usage` |
| Google Antigravity | `antigravity` | `Google Antigravity` | `get_antigravity_usage` |
| Claude Code | `claude` | `Claude Code` | `get_claude_usage` (unregistered) |
| xAI Grok (mock) | `grok` | `xAI Grok` | — |

Ids are stable and unique; the isolated `ClaudeProvider.id` (`"claude"`, claudeProvider.ts:50)
matches the mock id it will replace, so the swap in registry.ts is identity-preserving.
The mock's display name ("Claude") differs from the real adapter's ("Claude Code") — cosmetic.

## 2. Source type

| Provider | Source | Live/Cache |
| --- | --- | --- |
| Codex | Live API `GET chatgpt.com/backend-api/wham/usage` + local `~/.codex/auth.json` | Live |
| Z.ai | Live API `GET api.z.ai/api/monitor/usage/quota/limit` + local ZCode stores | Live |
| OpenCode Go | Live API `GET opencode.ai/zen/go/v1/usage` + local OpenCode `auth.json` | Live |
| Antigravity | Passive read of `~/.config/opencode/antigravity-accounts.json` (plugin-written cache). **No network at all.** | Cache |
| Claude | Live API `GET api.anthropic.com/api/oauth/usage`, fallback `POST /v1/messages` header probe + local `.claude/.credentials.json` | Live |

Cached-vs-live is explicit in the contract: `sourceUpdatedAt`/`dataFreshness` are defined as
present only for cached snapshots (types.ts:7–13), and only Antigravity sets them. That matches
reality exactly — no live provider pretends to have a snapshot time, and the one cached provider
cannot read as live.

## 3. Authentication

| | Codex | Z.ai | OpenCode Go | Antigravity | Claude (isolated) |
| --- | --- | --- | --- | --- | --- |
| Source | `~/.codex/auth.json` → `tokens.access_token` (+`account_id` header) | `ZAI_API_KEY` env → enabled `api.z.ai` providers in `~/.zcode/v2/config.json` → `enc:v1` entries in `credentials.json` (decrypted in-process) | `~/.local/share/opencode/auth.json` → `opencode-go.key` | none | `~/.claude/.credentials.json` → `claudeAiOauth.accessToken`; scope-based routing (`user:profile` → usage endpoint, inference-only → probe) |
| Backend-only? | Yes — token never crosses IPC | Yes | Yes | n/a (no secret is even extracted) | Yes |
| Read-only? | Yes | Yes (decrypt only, no writes) | Yes | Yes (no writes, no OAuth refresh) | Yes — refresh deliberately not implemented; expired token → `auth_expired` pointing at the CLI |
| Refresh behavior | None. Local JWT `exp` pre-check, server 401/403 as backstop; user re-logs via Codex CLI | None; on `auth_invalid`/`not_entitled` the Rust side falls through to the next candidate key | None | n/a | None. Local `expiresAt` pre-check; 403 on the usage endpoint falls back to the header probe |

All auth lives in Rust; every TS adapter receives only normalized windows (stated in each
adapter's doc comment and enforced by the wire-format tests, §10). Z.ai is the most complex:
a three-tier candidate list with per-key fallback (`zai.rs:628–643`), and host-pinning so a key
is never sent to a host it was not minted for (`provider_base_url_is_zai`, https required,
`zai.rs:435–455`).

## 4. Quota representation

- **`usedPercent` semantics:** always "share of the window consumed", 0–100, for every
  provider. Antigravity inverts upstream `remainingFraction` (0–1 *remaining*) into used
  percent (`antigravity.rs:168–170`); the Claude header path multiplies a 0–1 fraction by 100
  while its endpoint path is already 0–100 (`claude.rs:292–309`, `352–379`). No provider emits
  "remaining" disguised as "used".
- **Clamping:** every Rust backend clamps to `[0, 100]` at normalization, and every TS adapter
  re-clamps defensively at the boundary (`clampPercent` in each adapter). Antigravity also
  skips non-finite fractions rather than rendering them.
- **Window labels:** shared vocabulary for the common windows — `5-hour`, `Weekly` — with
  per-provider derivations: Codex maps window seconds (`Daily`/`Weekly`/`30-day`/humanized
  fallback), Z.ai maps upstream `type`/`unit`/`number` (`5-hour`, `Tokens`, `Monthly`, …),
  OpenCode Go pins `5-hour`/`Weekly`/`30-day`, Antigravity uses family labels
  (`Gemini`, `Claude/Grok`, `Gemini Weekly`). Drift is display-level only (see findings).
- **Reset timestamps:** all RFC-3339 strings. Codex derives whole-second UTC (`reset_at` unix
  seconds, falling back to `now + reset_after_seconds`); Z.ai converts epoch-ms with
  plausibility bounds (post-2100 / seconds-like magnitudes rejected, `zai.rs:210–227`);
  OpenCode Go, Antigravity, and Claude pass upstream ISO-8601 strings through *verbatim*
  (fractional seconds and offsets survive); Claude's probe renders epoch seconds to UTC.
  A malformed/missing stamp never blocks the window — it just drops the reset line.
- **Missing reset behavior:** uniform. A window with no usable reset is still emitted with
  `resetAt` absent; no provider invents a date. Verified in tests in all five backends.

## 5. Freshness

- `sourceUpdatedAt` / `dataFreshness` exist only on Antigravity: exact `cachedQuotaUpdatedAt`
  preserved to the millisecond, classified against a 24-hour threshold
  (`antigravity.rs:205–222`). An indeterminate stamp (missing/malformed) degrades to `stale`,
  never fresh; a stale snapshot keeps every window visible but surfaces as status `stale`
  instead of `ok` (antigravityProvider.ts:70–75).
- Live providers set neither field and carry no snapshot concept — correct, since their data
  is per-request.
- Separately, UI-level staleness (`useProviderUsage.ts:83–89`: no successful refresh for
  2× the interval) applies uniformly to all cards; that is a fetch-freshness readout, not a
  source-freshness verdict, and the two do not conflict.
- On a failed refresh, Antigravity's retained last-good keeps its `sourceUpdatedAt` and
  `dataFreshness` with it (providers.test.ts:354–371).

**Freshness default (historical correction).** The TS adapter coerces an absent/unknown
verdict to `stale` (`data.dataFreshness === "fresh" ? "fresh" : "stale"`,
antigravityProvider.ts) — which already satisfied the documented "unknown age must never read
as fresh" rule (types.ts) at the v0.3.0 RC. The unsafe mapping (absent verdict → **fresh**)
existed only in the v0.2-era code and was corrected before the v0.3.0 RC. The v0.3.1 hardening
commit (84d1ac6) then closed the residual paths that could still turn an undatable snapshot
into a fresh verdict: a `fresh` verdict is trusted only when it arrives with a parseable
`sourceUpdatedAt`, and the Rust half degrades a source stamp more than five minutes ahead
of the machine clock to `stale` instead of reading it fresh indefinitely. Undatable data is
also excluded from prediction.

## 6. Failure taxonomy

Every error code currently emitted by each backend (`{ code, message }` structured payloads;
messages are documented and tested to be display-safe):

| Meaning | Codex | Z.ai | OpenCode Go | Antigravity | Claude |
| --- | --- | --- | --- | --- | --- |
| Source app/store missing | `codex_not_installed` | `zcode_not_installed` | `opencode_not_installed` | `cache_missing` (file absent) | `claude_not_installed` |
| Credential file missing | *(folded into `not_logged_in`)* | — | `auth_file_missing` | — | *(folded into `credential_missing`)* |
| No usable credential present | `not_logged_in` | `credential_missing` | `credential_missing` | `no_account` / `quota_missing` | `credential_missing` |
| Store unreadable / unparsable | `auth_unreadable` | `auth_unreadable` (incl. undecryptable) | `auth_unreadable` | `cache_unreadable` + `cache_invalid` | `auth_unreadable` |
| Credential rejected by server | `auth_expired` (401 **and** 403) | `auth_invalid` (401) | `auth_invalid` (401) | — | `auth_invalid` (401) |
| Entitlement refusal | — (folded into `auth_expired`) | `not_entitled` (403) | `not_entitled` (403) | — | `not_entitled` (403; triggers probe fallback) |
| Expired locally (pre-flight) | `auth_expired` (JWT `exp`) | — | — | — | `auth_expired` (`expiresAt`) |
| Schema change (source format no longer understood) | `unexpected_response` | `unexpected_response` | `unexpected_response` | **`schema_changed`** | `unexpected_response` |
| HTTP 429 / 5xx / other non-2xx | `unexpected_response` ("… returned HTTP {status}.") | `unexpected_response` (HTTP wording **or** HTTP-200 envelope "(code {code}): {msg}", `msg` display-sanitized before render) | `unexpected_response` ("… returned HTTP {status}.") | — | `unexpected_response` ("… returned HTTP {status}.") |
| Transport failure (connect/DNS/timeout/truncated) | `network` | `network` | `network` | — | `network` |
| Local init failure (HTTP client) | `unexpected` | `unexpected` | `unexpected` | — | `unexpected` |
| Home directory unresolvable | *(inside `codex_not_installed`)* | *(inside `zcode_not_installed` / `auth_unreadable`)* | *(inside `opencode_not_installed`)* | **`home_unresolved`** | *(inside `claude_not_installed`)* |

21 distinct codes across five backends. Analysis:

- **Duplicate meanings under different names:**
  - `schema_changed` (Antigravity) ≡ `unexpected_response` (all four others): the same
    "source format is no longer understood" verdict, named differently because Antigravity's
    source is a file, not a response.
  - `not_logged_in` (Codex) ≡ `credential_missing` (Z.ai, OpenCode Go, Claude): "the store has
    no usable credential".
  - `home_unresolved` (Antigravity) duplicates a condition every other provider folds into its
    `*_not_installed` code.
  - Z.ai also overloads `auth_unreadable` to cover *undecryptable* (not merely unreadable)
    credentials — a mild semantic stretch within one provider.
- **Provider-specific codes that should stay provider-specific:** the `*_not_installed` family
  (they name the app to install/open and are directly actionable), the Antigravity domain codes
  (`cache_missing`, `no_account`, `quota_missing` — there is no credential or server in that
  flow), and `auth_file_missing` if OpenCode's file-vs-credential distinction is kept.
- **Codes that could become generic (but are not yet):** `credential_missing` (already shared by
  three providers), `auth_invalid`/`not_entitled` (shared by three), `auth_expired` (two), and
  the already-generic `network` / `unexpected` / `unexpected_response`. Nothing *forces*
  unification yet — see §Final.
- **Special mechanism worth naming:** the frontend retry layer classifies 429/5xx by regex on
  the error *message text* (`/returned HTTP (429|5\d\d)\b/`, `/\(code (429|5\d\d)\)/`,
  transientRetry.ts:27–29), because the structured payload carries only `{ code, message }`.
  This is an implicit cross-boundary string contract. It is double-guarded — codex.rs
  `schema_error_messages_are_not_transient` (codex.rs:530–549) asserts schema errors never emit
  the magic wording, and transientRetry has its own tests — but a innocent wording change on
  either side silently breaks 429/5xx retries.

## 7. Retry semantics

`withTransientRetry` (transientRetry.ts): at most one retry after 750 ms + 0–250 ms jitter;
only `network` and `unexpected_response`-with-429/5xx are transient. Everything else
(auth, entitlement, credentials, schema, local init) is deterministic and never retried.

| Failure class | Codex | Z.ai | OpenCode Go | Antigravity | Claude (isolated) |
| --- | --- | --- | --- | --- | --- |
| network | retry ×1 | retry ×1 | retry ×1 | n/a (no network) | retry ×1 |
| 429 | retry ×1 (message match) | retry ×1 (HTTP message or envelope) | retry ×1 | n/a | retry ×1 |
| 5xx | retry ×1 | retry ×1 | retry ×1 | n/a | retry ×1 |
| auth | never | never | never | n/a | never |
| entitlement | never | never *(Rust falls through key candidates instead)* | never | n/a | never *(Rust falls back to the probe route instead)* |
| schema | never | never | never | never | never |

Consistency verdict: **uniform**. All four live-fetch adapters share the identical retry
wrapper; Antigravity omits it by design (a local file read has no transient failure class).
Two provider-specific *fallbacks* live below the retry layer and are orthogonal to it:
Z.ai's candidate-key loop (only on `auth_invalid`/`not_entitled`, zai.rs:85–88) and Claude's
route fallback (only on `not_entitled`, claude.rs:503–513). Both are correctly *not* retried
by the frontend.

One provider-specific tension, documented but worth restating: the Claude usage endpoint is
verified to tolerate ≥180 s per-token cadence and to 429 unknown/aggressive clients; the app's
retry-once after ~750 ms will usually burn a second 429. Harmless (last-good holds), but the
real guard is cadence — see §11.

## 8. Last-good semantics

Verified identical across all five adapters (four wired + isolated Claude):

| Aspect | Codex | Z.ai | OpenCode Go | Antigravity | Claude |
| --- | --- | --- | --- | --- | --- |
| Windows retained on refresh failure | yes | yes | yes | yes | yes |
| `checkedAt` preserved from the *original* fetch | yes | yes | yes | yes | yes |
| `sourceUpdatedAt`/`dataFreshness` retained | n/a | n/a | n/a | **yes** | n/a |
| Status becomes `error` | yes | yes | yes | yes | yes |
| Error reason surfaced | `Refresh failed: {message} ({code})` | same | same | same | same |
| First-fetch failure | error card, no windows | same | same | same | same |

Tested per provider (providers.test.ts:62–76, 223–236, 260–272, 354–371; claudeProvider.test.ts:142).
The retention logic is duplicated, not shared — which is exactly why it *is* consistent today:
each adapter was copied from the previous one. `fetchAllUsage` additionally guarantees a
contract-violating adapter rejection degrades only its own card (registry.ts:53–68), and the
RefreshCoordinator serializes cycles so retained data is never clobbered mid-flight.

Cosmetic: the `fetchAllUsage` rejection fallback sets `name: provider.id` rather than the
adapter's display name (registry.ts:61) — unreachable with well-behaved adapters.

## 9. Empty-success semantics

The dangerous case — an HTTP-200-ish success carrying zero meaningful windows, which would
overwrite last-good with an empty "unknown" card — is **closed at the backend for every
provider**:

| Provider | Guard |
| --- | --- |
| Codex | 200 without a usable *primary* window → `unexpected_response` (`parse_usage_response`, codex.rs:266–300; added in commit 0cbd3fe) |
| Z.ai | envelope-auth failures → `auth_invalid`/`not_entitled`; empty `limits[]` / no payload → `unexpected_response` (zai.rs:277–283) |
| OpenCode Go | neither current nor legacy shape yields a window → `unexpected_response` (opencode_go.rs:213–220) |
| Antigravity | no usable family windows → `schema_changed` (antigravity.rs:332–334) |
| Claude | no usable window on either route → `unexpected_response` (claude.rs:333–340, 372–377) |

Consequently the adapters' `limits.length > 0 ? "ok" : "unknown"` branch is defensive-only and
cannot fire from a real backend. Note the residual hazard honestly: the adapters *do* assign
`this.lastGood = usage` before checking window count, so if a future backend ever returned an
empty success, last-good would be overwritten with the empty state. Today that is unreachable
on all five paths; the guard lives at the right layer (the source of the data), and the finding
is recorded as CONSISTENT with the observation that the frontend layer alone would not prevent
it.

## 10. Credential boundary

| | Codex | Z.ai | OpenCode Go | Antigravity | Claude |
| --- | --- | --- | --- | --- | --- |
| Secrets read | access token (+account id) | plan API keys (plaintext config mirror + decrypted `enc:v1` store) | API key | **none** — quota fields only are extracted; refreshToken/sessionToken/email/fingerprint never parsed into any struct | OAuth access token (+expiry, scopes) |
| Secret storage | third-party files, read-only | third-party stores, read-only (incl. in-process AES-256-GCM decryption of the ZCode store) | third-party file, read-only | n/a | third-party file, read-only |
| `Debug` redacted | `CodexAuth` manual impl, token `<redacted>` (codex.rs:164–171) | `ZaiKey` redacting newtype (zai.rs:366–372) | `OpenCodeGoKey` redacting newtype (opencode_go.rs:224–231) | no secret struct exists | `ClaudeAccessToken` redacting newtype; struct `Debug` composes it (claude.rs:139–156) |
| Wire-format test (Rust) | yes — asserts token/account/email/user absent from serialized usage (codex.rs:552–573) | yes — camelCase + key absent (zai.rs:1126–1138) + `Debug` leak test | yes — camelCase + key absent (opencode_go.rs:558–573) | yes — camelCase + refreshToken/session token/email/fingerprint absent (antigravity.rs:736–758) | yes — camelCase + token absent (claude.rs:843–856) |
| Boundary test (TS) | yes — extra payload fields (`accessToken`, `email`) never reach the card (providers.test.ts:150–163) | yes | yes | yes | yes (claudeProvider.test.ts:131) |

Error messages never embed tokens or file contents (per-backend doc comments; messages embed
only `io::Error` strings for unreadable files, which carry paths, not contents). The one
structural caveat: Z.ai's backend **re-implements the ZCode app's private `enc:v1` scheme**
(SHA-256 of a derived secret → AES-256-GCM, zai.rs:286–361). It is tested against sealed
fixtures, but it is a standing coupling to another application's internal format — the most
maintenance-exposed code in the auth surface. The plaintext-mirror-first strategy is the
mitigation and is documented.

## 11. Integration readiness

**Claude Code** — implementation complete and tested (19 Rust tests + 19 TS tests per the
discovery doc, both suites green), but deliberately unreachable: `mod claude` carries
`#[allow(dead_code)]` and is absent from the invoke handler (main.rs:8–9, 44–49), and the
registry still serves the mock. The exact missing prerequisites, in order:

1. `src-tauri/src/main.rs`: add `claude::get_claude_usage` to `tauri::generate_handler![]` and
   drop the `#[allow(dead_code)]` above `mod claude;`.
2. `src/providers/registry.ts`: replace `MockProvider({ id: "claude", … })` with
   `new ClaudeProvider()` (add the import). Ids already match; no UI change needed.
3. **Live verification, which has never happened**: the provider is NOT live-verified (no
   Claude Code installation or credential existed on the dev machine). On a machine with a
   logged-in Claude Code, run `cargo test -- --ignored live_fetch` once, then observe a real
   refresh cycle. This is the only prerequisite that cannot be done offline, and the endpoint
   is undocumented — schema drift is a live risk until this passes.
4. **Cadence decision**: the refresh-interval setting offers 1 minute (settings.ts:1); the
   usage endpoint is verified to destabilize below ~180 s per token and to 429 aggressively.
   Integration should either accept 429 churn on the 1-minute setting (last-good + single
   retry contain it) or gate Claude's polling to ≥3 min. Decide before flipping the registry.

**Grok** — no isolated implementation exists (mock only; discovery doc rates it Medium/High
difficulty because the viable source is an internal, undocumented billing plane requiring a
full OAuth refresh loop). Excluded from this audit per task definition.

---

## Output table

| Provider | Source | Live/Cache | Auth | Windows | Freshness | Retry | Last-good | Security | Readiness |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OpenAI / Codex | chatgpt.com usage API + `~/.codex/auth.json` | Live | Bearer; backend-only; read-only; no refresh | primary + optional secondary; clamped; reset derived or passthrough | `checkedAt` only (live) | network/429/5xx ×1 | full retention, tested | Debug-redacted; wire + TS boundary tests | Production |
| Z.ai | api.z.ai monitor API + ZCode stores | Live | Bearer; 3-tier key candidates; backend-only; read-only; no refresh | upstream `limits[]` mapped by type/unit; clamped; epoch-ms reset with bounds | `checkedAt` only (live) | network/429/5xx ×1 + Rust key fallback | full retention, tested | Debug-redacted; wire + TS tests; re-implements ZCode `enc:v1` | Production |
| OpenCode Go | opencode.ai usage API + OpenCode `auth.json` | Live | Bearer; backend-only; read-only; no refresh | rolling/weekly/monthly, dual-shape feature-detect; clamped; verbatim reset | `checkedAt` only (live) | network/429/5xx ×1 | full retention, tested | Debug-redacted; wire + TS tests | Production |
| Google Antigravity | plugin quota cache (local file) | Cache | none (passive) | family windows; remaining→used inverted; clamped; verbatim reset | `sourceUpdatedAt` + 24 h verdict; indeterminate → stale | none (by design) | full retention **incl. freshness metadata**, tested | no secret ever extracted; wire test | Production |
| Claude Code | api.anthropic.com usage endpoint (+ probe fallback) | Live | OAuth Bearer; scope-routed; backend-only; read-only; CLI owns refresh | 5-hour + Weekly; dual scale (0–100 / 0–1) handled; clamped | `checkedAt` only (live) | network/429/5xx ×1 + Rust route fallback | full retention, tested | Debug-redacted; wire + TS tests | Isolated — register command + registry swap + first live verification (+ cadence decision) |
| xAI Grok | — (discovery only) | — | — | — | — | — | — | — | Mock only; excluded |

## Findings

**BLOCKER** — none. Every production provider guards last-good against empty successes,
keeps credentials backend-side with tested redaction, and behaves identically on retry and
retention.

**SHOULD FIX**

1. **Retry classification rides on message text across the IPC boundary**
   (transientRetry.ts:27–29 ⇄ codex.rs:257–264, zai.rs:241–248, opencode_go.rs:347–352,
   claude.rs:450–464). 429/5xx transiency is decided by regexing `"returned HTTP {status}"`
   / `"(code {code})"` out of human-readable messages. Both sides carry tests pinning the
   wording, but the contract is implicit and a wording edit silently disables retries.
   Smallest durable fix: add a structured field (`transient` or `httpStatus`) to the error
   payload and let the frontend read it.
2. **Dead wire field `planType`** (codex.rs:44–47 → codexProvider.ts:11): the Codex command
   returns it, the TS payload type declares it, nothing consumes it. Drop it or surface it.
3. **1-minute refresh option vs Claude's ≥180 s endpoint cadence** — resolve (gate or accept
   documented 429 churn) before the Claude registry swap, since the interval setting is
   global, not per-provider.

**RESOLVED (recorded for history)**

- **Antigravity freshness default** — the v0.2-era TS adapter defaulted an absent
  `dataFreshness` to `fresh`; that unsafe mapping was corrected before the v0.3.0 RC, so the
  frontend already coerced an absent/unknown verdict to `stale` at the RC. v0.3.1 (84d1ac6)
  then hardened the remaining indeterminate paths (a `fresh` verdict without a parseable
  `sourceUpdatedAt`; a source stamp far in the future), so both halves of the "indeterminate
  never reads fresh" guarantee now agree. No action remains; recorded so the stale claim is
  not re-invented.

**CONSISTENT** (verified uniform; no action)

- Empty-success → structured error at every backend; last-good can never be overwritten by
  zero windows (§9).
- Last-good retention semantics byte-for-byte identical across all five adapters, including
  Antigravity's freshness metadata (§8).
- Clamping double-applied (Rust + TS) everywhere; missing resets never invented anywhere.
- Credentials never cross IPC; `Debug` redaction and wire-format tests present on all five
  backends; TS boundary tests reject unknown payload fields.
- Retry taxonomy identical across live providers; provider-specific fallbacks (Z.ai key
  rotation, Claude probe route) correctly live below the retry layer.
- Single-flight on all live-fetch adapters (Antigravity omits it; harmless — the registry calls
  each adapter once per cycle and the coordinator serializes cycles).
- Cached-vs-live explicitness: `sourceUpdatedAt`/`dataFreshness` set exactly where a snapshot
  exists and nowhere else.

**FUTURE CLEANUP**

1. `schema_changed` (Antigravity) → fold into `unexpected_response` (one rename + test edits)
   when Antigravity is next touched.
2. `not_logged_in` (Codex) → `credential_missing` unification; same meaning, two names.
3. `home_unresolved` (Antigravity) → fold into its `cache_missing`/`cache_unreadable` paths,
   matching how the other four providers handle home-resolution failure.
4. Monthly-label drift: OpenCode Go `30-day` vs Z.ai `Monthly` vs Codex `30-day` — pick one
   display convention.
5. Z.ai's in-process re-implementation of the ZCode `enc:v1` scheme is a standing coupling to
   another app's private format — fine today, re-evaluate if ZCode changes its store.
6. `fetchAllUsage` rejection fallback uses `name: provider.id` (registry.ts:61) instead of the
   display name; cosmetic, near-unreachable.

---

## Final section

**1. Is the current `ProviderUsage` contract still sufficient?**
Yes. Every audited behavior — live vs cached, stale-but-visible, structured failure with
retained data, per-window resets — maps onto `status` / `limits` / `checkedAt` / `error` /
`sourceUpdatedAt` / `dataFreshness` without strain, and the isolated Claude provider needs
nothing new. Two thin spots, neither contract-breaking: the `error` field is a flattened
display string with no structured code channel (which is *why* the retry layer regexes
messages — the fix belongs in the command error payload, not in `ProviderUsage`), and the
`"unknown"` status is reachable only defensively/mocks. No new field is justified today;
Grok's billing-plane windows would also fit the existing shape.

**2. Is a shared backend provider abstraction justified yet?**
On the Rust side, **no**. What the five backends genuinely share is ~40 lines each (HTTP
client, error struct, clamp, reset validation); the substance — credential stores, response
shapes, freshness — is exactly what differs, and a trait would trade visible, independently
tested code for indirection. On the TS side the duplication is real: the four live adapters
are ~95 % identical (~100 lines each), and Claude makes five. The only abstraction the
concrete duplication justifies is a small factory — `createInvokingProvider(id, name, command)`
plus a freshness-aware variant for Antigravity — replacing ~400 lines with ~80. That is a
lightweight helper, not a framework, and it is reasonable to defer even that until the Claude
registry swap lands and the pattern is confirmed a fifth time. Keep the architecture as is.

**3. The three smallest contract improvements worth doing next**
1. Add a structured `transient` flag (or `httpStatus`) to the command error payloads and read
   it in `isTransientCommandError` — deletes the message-regex contract entirely (SHOULD FIX 1;
   touches four error structs, one TS helper, and their tests).
2. Antigravity freshness — **already aligned.** The v0.2-era frontend default (absent verdict
   → fresh) was corrected before the v0.3.0 RC, and v0.3.1 (84d1ac6) hardened the residual
   far-future and undatable paths, so both halves of the "indeterminate never reads fresh"
   guarantee now agree. No change remains.
3. Rename Antigravity's `schema_changed` to `unexpected_response` (one string + test edits),
   collapsing the only cross-provider duplicate name for the same verdict.

**4. What should explicitly NOT be done yet**
- **No Rust provider trait or provider framework.** Five backends share boilerplate, not
  behavior; the divergence *is* the value.
- **No shared credential abstraction.** `auth.json`, the ZCode `enc:v1` store, the OpenCode
  auth file, and `.claude/.credentials.json` are four different formats with different trust
  models; a common interface would be a fiction over them.
- **No unification of the auth/entitlement codes** (`auth_expired` vs `auth_invalid` vs
  `not_entitled`). The distinctions are real and user-actionable: Codex 403 *is* a session
  problem, Z.ai 403 is a plan-access problem. Collapsing them loses actionable text.
- **No shared TS↔Rust error-code enum/codegen.** 21 codes across five providers is still
  doc-table territory.
- **No `sourceUpdatedAt` for live providers.** The field is defined as a snapshot timestamp;
  live providers have no snapshot, and fabricating `checkedAt`-equivalents would muddy it.
- **No Grok implementation.** Per discovery: internal undocumented billing plane + full OAuth
  refresh loop for the only viable source. Keep the mock until that trade-off is consciously
  accepted.
