# Grok / xAI Provider Discovery — Rate Limits v0.4

Companion to `provider-discovery.md` (§3.3) for the **xAI Grok** quota provider.
Bounded discovery pass performed **2026-09-28**. Discovery only — no adapter was
implemented, no auth state was modified, no OAuth refresh was performed, no browser
cookies were read, and no paid inference calls were made. Exactly **one** live
non-inference HTTP request was sent.

This pass starts from current local reality. The 2026-09-27 pass (recorded in the
`codex/mimo-provider-discovery` worktree) found only an **expired** OpenCode xAI
credential, so the positive quota path could not be verified there. Local reality has
since changed: a second harness (`@bitkyc08/opencodex`) now holds a **valid** xAI
OAuth credential, and the quota surface was **live-verified positively** this pass
(HTTP 200 + full schema + weekly window + reset + account scoping).

**Evidence tiers:**

| Tag | Meaning |
| --- | --- |
| `[LOCAL]` | Verified on this machine today (files/keys inspected redacted; live HTTP probe) |
| `[LOCAL-prior]` | Verified on this machine in the 2026-09-27 pass (failure paths), not repeated here to keep the request count minimal |
| `[UPSTREAM]` | Verified from installed client source (`@bitkyc08/opencodex` ships TypeScript), official docs, or published third-party implementations (CodexBar, TokenTracker) |
| `[INFERRED]` | Reasonable conclusion, not confirmed by execution |

---

## 1. Local environment (re-verified 2026-09-28) `[LOCAL]`

| Check | Result |
| --- | --- |
| Grok CLI on PATH / npm / pip | **Absent** |
| `%USERPROFILE%\.grok\auth.json` (canonical CLI store) | **Does not exist** |
| `GROK_HOME` / `GROK_*` / `XAI_*` env vars | Not set |
| Windows Credential Manager (`cmdkey /list`) | No grok/xAI entries |
| `~/.local/share/opencode/auth.json` → `xai` (opencode-ai 1.18.28, a compiled binary wrapper) | `{ type: "oauth", access, refresh, expires }` — **access expired 2026-09-05T01:58:05Z (−23 days)**, refresh present. JWT: `iss https://auth.x.ai`, `aud/client_id <grok-cli-device-flow-client-id>` (the Grok CLI device-flow client), `scope: openid profile email offline_access grok-cli:access api:access`, `sub <redacted-sub>`, plus `tier`/`team_id`/`principal_type` claims |
| `~/.opencodex/auth.json` → `xai` (@bitkyc08/opencodex 2.68.0, ships readable TS source) | **Two accounts** under `xai.accounts[]`, each `{ id, credential: { access, refresh, expires, accountId, email?, source }, addedAt }`, plus `activeAccountId` and `selectionRevision` |
| ↳ `xai.accounts[0]` (id `<redacted-account-id>`, same user as the opencode entry, `sub <redacted-sub>`) | access token **expired today 14:48Z** but `iat 08:50Z` — refreshed by the harness **this morning**; proves the fork actively refreshes its own credentials |
| ↳ `xai.accounts[1]` = **active** (id `<redacted-account-id>`, `sub <redacted-sub>`, **a different xAI account**) | `iat 2026-09-28T13:32:29Z`, stored `expires 2026-09-28T19:30:28Z` (harness subtracts a 2-min skew), JWT `exp 19:32:29Z` → **VALID at probe time**; `tier 4`, `principal_type "User"`, `team_id <redacted-team-id>` |
| Other local harnesses (kilo, OpenCodex AppData runtime, Claude/Codex/Gemini stores) | No xAI credentials |

Access tokens are **~6-hour JWTs** issued by `https://auth.x.ai`. The two accounts in
the opencodex store have different `sub` values — multi-account is real on this
machine and the store carries explicit `activeAccountId` selection.

## 2. Product distinction — four separate worlds (not one provider)

| World | Credential | Quota surface | In scope? |
| --- | --- | --- | --- |
| **grok.com consumer (SuperGrok web)** | Browser session cookies | None found without cookies | **No** — cookies-only ⇒ security/maintenance problem, excluded (§8) |
| **SuperGrok / Grok subscription via the Grok CLI credential world** | OAuth access JWT (`aud` = Grok CLI device-flow client, scope `grok-cli:access`) | `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` — **live-verified §4** | **Yes — this is what every local credential is for** |
| **xAI API developer billing** (`api.x.ai`, `xai-` keys) | Static API key | No quota-query endpoint; only per-minute `x-ratelimit-*` headers on inference calls; console.x.ai is dashboard-only | **No** — different credential world, and RPM/TPM headers are poor quota-dashboard material |
| **Model context / rate-limit metadata** | none | Static per-model context windows and documented API rate-limit tiers | **No** — not usage state |

An API-billing endpoint was **not** assumed to represent the consumer subscription:
the verified endpoint is the subscription credit pool keyed by an OAuth `grok-cli`
token, and its payload has no dollar-billing semantics (`prepaidBalance` and legacy
`monthlyLimit`/`used` cents exist only as fallback fields — §4.3).

## 3. Auth model

**3.1 Canonical store** `[UPSTREAM]` — `~/.grok/auth.json` (Grok CLI; absent here):
top-level keys `https://auth.x.ai::<client-id>` (OIDC entries) or
`https://accounts.x.ai/sign-in` (legacy); per entry `key` (access token),
`refresh_token`, `expires_at`, `auth_mode: Oidc|ApiKey`, `email`, `team_id`,
`user_id`, `first_name`/`last_name`. `GROK_HOME` overrides the directory. Confirmed
by the installed fork's own detector (`src/oauth/local-token-detect.ts`), which reads
exactly this schema read-only.

**3.2 Stores present on this machine** `[LOCAL]` — OpenCode flat entry
(`{type:"oauth", access, refresh, expires}`) and the OpenCodex per-account store
(§1). The OpenCodex store is the only one with an unexpired token today.

**3.3 Token facts** `[LOCAL]` — JWT from `https://auth.x.ai`; ~6 h lifetime; audience
`<grok-cli-device-flow-client-id>` (Grok CLI device-flow client); scope includes `grok-cli:access` (the
CLI/proxy plane) and `api:access`; claims `sub`, `tier`, `team_id`, `principal_type`,
`principal_id`, `jti`.

**3.4 Refresh — the harness owns it; we must never do it** `[UPSTREAM]` + `[LOCAL]`
The installed fork registers `refreshXaiToken` (POST
`auth.x.ai/oauth2/token`, `grant_type=refresh_token`, PKCE client — endpoints
resolved from `/.well-known/openid-configuration`, host-pinned to
`auth.x.ai`/`accounts.x.ai`) into its OAuth controller
(`src/oauth/index.ts:253`) and refreshes on demand with a 2-minute skew
(`src/oauth/xai.ts`). The refresh token **may rotate** on use and the harness
persists rotated credentials — a refresh that isn't persisted can invalidate the
user's stored login. Two refresh events were observed on disk today (08:50Z,
13:32Z), both made by the harness, none by this discovery. **Adapter policy: read
the cached access token read-only, gate on local expiry; on expiry emit
`credential_expired` with a re-auth hint (use the owning harness / `grok login`) —
never POST to the token endpoint, never write any file.** `[LOCAL-prior]` reached
the same policy from CodexBar's identical stance.

## 4. Remote quota source — live-verified `[LOCAL]`

### 4.1 The request

`GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`

Headers (exactly what the installed harness's own quota probe sends,
`src/providers/quota/vendor-probes-oauth.ts` → `fetchXaiWeeklyCredits`):

```
Authorization: Bearer <access token>
x-xai-token-auth: xai-grok-cli
x-authenticateresponse: authenticate-response
x-grok-client-version: 0.2.93
x-userid: <user id = credential.accountId, else JWT sub>
Accept: application/json
```

- **Host**: `cli-chat-proxy.grok.com` — the same plane serves the official Grok CLI's
  inference (`/v1` Responses API; the harness lists it alongside `api.x.ai` as the
  two xAI Responses hosts, `src/providers/xai-transport.ts:7-10`).
- **Documented**: **NO.** Reverse-engineered billing plane of the official CLI,
  corroborated by three independent third-party implementations (CodexBar,
  TokenTracker, sandboxed.sh `[LOCAL-prior]`/`[UPSTREAM]`) and now by the installed
  harness source and a live 200.
- **Consumes inference/token quota**: **No** — it is a billing query, not a model
  call (`[INFERRED]` from purpose; the probe returned in 636 ms and burned nothing
  observable).
- **Account scoping**: by bearer token; `x-userid` echoes the user id (§6).

### 4.2 Live result (2026-09-28T18:39:23Z) — HTTP 200, `application/json`, cloudflare

```json
{"config":{
  "currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY",
                   "start":"2026-09-26T13:12:49.368933+00:00",
                   "end":"2026-10-03T13:12:49.368933+00:00"},
  "creditUsagePercent":54.0,
  "onDemandCap":{"val":0},"onDemandUsed":{"val":0},
  "productUsage":[{"product":"GrokBuild","usagePercent":54.0}],
  "isUnifiedBillingUser":true,
  "prepaidBalance":{"val":0},
  "topUpMethod":"TOP_UP_METHOD_SAVED_PAYMENT_METHOD",
  "billingPeriodStart":"2026-09-26T13:12:49.368933+00:00",
  "billingPeriodEnd":"2026-10-03T13:12:49.368933+00:00"}}
```

Schema notes against the upstream references:

- `creditUsagePercent` is the **USED percent (0–100) of the subscription credit
  pool** — not remaining, not a counter. `[UPSTREAM]` agreement + live value.
- `currentPeriod.type === "USAGE_PERIOD_TYPE_WEEKLY"` with absolute ISO-8601
  `start`/`end`. The fork's parser hard-requires exactly this type string.
- **`subscriptionTier` is absent from this payload** (older upstream schemas show
  it). Do not rely on it for a plan label; the JWT `tier` claim is a number (4)
  with unverified semantics — also do not map it.
- **New fields not in the 2026-09-27 schema notes**: `isUnifiedBillingUser`,
  `prepaidBalance`, `topUpMethod` — safe to ignore; they confirm this is a
  unified-billing subscription account with no prepaid balance.
- The response did **not** echo the bearer token (`TOKEN_LEAK_IN_BODY no`), carried
  no rate-limit headers, and no retry-after.

### 4.3 Fallback surface (harness-verified, not probed live)

`GET https://cli-chat-proxy.grok.com/v1/billing` (no `format` param, bearer +
`Accept` only) → legacy **monthly dollar pool**: `config.monthlyLimit` /
`config.used` (cents, `{"val": n}` wrapped) with `config.billingPeriodEnd` as reset.
The harness uses it only when the weekly credits window is unavailable
(`"xai:grok-billing"` vs `"xai:grok-billing-credits"` source tags). An adapter
should treat it the same way: fallback only, and dollar-pool percent
`used/limit×100`. `[UPSTREAM]`

### 4.4 Failure semantics

| Condition | Behavior | Tier |
| --- | --- | --- |
| No auth header | HTTP 401, plain-JSON `{"error":"…auth_kind=none…"}`, `WWW-Authenticate: Bearer error="invalid_token"` | `[LOCAL-prior]` |
| Expired bearer | HTTP 401, same shape with `auth_kind=bearer` | `[LOCAL-prior]` |
| 403 / 429 / 5xx body shapes | **Not live-verified**; classified defensively from upstream implementations (403 = not entitled; 429 honor `Retry-After`) | `[INFERRED]` |
| Non-JSON body / missing `config` | Schema drift → treat as `unexpected_response`, never fabricate windows | `[INFERRED]` |

Repeating the 401 probes today was skipped deliberately — the failure shapes were
verified on this machine one day earlier and the task caps request count.

## 5. Quota windows and reset semantics (live-verified)

- **One primary window**: the weekly credit pool. `usedPercent` =
  `config.creditUsagePercent` clamped to 0–100; `resetsAt` =
  `config.currentPeriod.end` (absolute ISO-8601, fallback `billingPeriodEnd`).
- The window is a **rolling 7-day period anchored to the subscription's billing
  anchor** (Sat 2026-09-26 13:12:49Z → Sat 2026-10-03 13:12:49Z), **not a calendar
  week** — labels must come from the `type` field, never from date arithmetic.
- **On-demand sub-cap**: a second window (`onDemandUsed`/`onDemandCap` × 100,
  `{"val": n}` amounts) only when `onDemandCap.val > 0` — zero here, so no second
  window for this account.
- **`productUsage` rows are shares of the same pool** (GrokBuild 54% == total 54%),
  never independent windows.
- **Percent absent ⇒ unknown usage ⇒ zero windows** (never 0%). Note the installed
  fork treats an omitted percent as 0 via proto3 implicit-presence reasoning; the
  honest display for a withheld field is "unknown", not "0%". `[INFERRED]`
  divergence from `[UPSTREAM]`, resolved conservatively.

## 6. Account semantics (live-verified)

- **Stable safe identity exists**: the JWT `sub` (also stored as
  `credential.accountId` by the harness, and as `user_id` in the canonical CLI
  file). Probe account: `sub <redacted-sub>`, `tier 4`, `principal_type "User"`,
  `team_id <redacted-team-id>`. No identity needs to be derived from raw OAuth material at
  render time beyond decoding the claim the file already stores.
- **`x-userid` header**: the harness always sends it (stored accountId, JWT `sub`
  fallback). CodexBar reportedly works without it, so the bearer alone scopes the
  account; sending both (as the harness does) is the verified combination.
- **Multi-account**: a store can hold several accounts with different `sub`s (two
  on this machine). Resolution: prefer the store's own active selection
  (`activeAccountId` / top-level entry), and render a masked account id so the user
  can tell *which* xAI account a card describes.

## 7. Proposed contract (draft — do not implement from memory, see §10)

```jsonc
{
  "providerId": "grok",
  "displayName": "Grok (xAI)",
  "status": "ok",                    // ok | error | unknown
  "checkedAt": "2026-09-28T18:39:23Z",
  "account": {
    "id": "<redacted-sub>",               // JWT sub / credential.accountId, masked for display
    "source": "opencodex"            // grok-cli | opencode | opencodex — which store
  },
  "limits": [
    // primary: weekly credit pool (type-derived label)
    { "label": "Weekly credits", "usedPercent": 54.0, "resetsAt": "2026-10-03T13:12:49Z" }
    // appended only when onDemandCap.val > 0:
    // { "label": "On-demand", "usedPercent": <used/cap*100>, "resetsAt": "<currentPeriod.end>" }
  ]
}
```

## 8. Security implications

- **Risk: LOW.** Credential access is a read-only parse of plaintext JSON the user's
  own harnesses already own; no keyring, **no browser cookies**, no writes, no
  refresh, no chmod. All HTTP is TLS to a host the user's installed harness already
  sends the same token to. Tokens are never logged or rendered; the wire payload
  carries only `label/usedPercent/resetsAt`.
- **Browser cookies**: the only route to grok.com consumer-web quota would be
  session cookies — classified explicitly as a **security and maintenance problem**
  (cookie theft surface, opaque schema, breakage on every UI change). **Not worked
  around; excluded by design.**
- **Residual**: `cli-chat-proxy.grok.com` is xAI's internal CLI plane — unofficial
  for third parties. Sending the user's `grok-cli`-scoped token there is exactly
  what their own installed tools already do, but it remains TOS-sensitive.

## 9. Maintenance risk

**MEDIUM.** Undocumented endpoint; compatibility headers carry a client-version
string that may age; the schema has already drifted once (`subscriptionTier`
absent); account variants (SuperGrok Heavy, on-demand cap > 0, non-unified
billing, legacy monthly pool) exist in upstream code but were not all observable on
this machine. Blast radius is bounded by conservative normalization (drift ⇒
`unexpected_response` or zero windows, never fabricated percentages) and by the
failure matrix in §4.4. Credential availability churns hourly (§3.4): expect
`credential_expired` whenever the user hasn't used their harness recently — this is
a designed state, not a bug.

## 10. Error mapping and refresh cadence

| Condition | Code | Transient | Notes |
| --- | --- | --- | --- |
| No credential store exists | `credential_missing` | no | hint: install Grok CLI or log in via a harness |
| Stores exist, no usable xai OAuth entry | `credential_missing` | no | `ApiKey`-mode entries are a different world — skip |
| Local expiry in the past / unparseable | `credential_expired` | no | **no network call is made at all**; re-auth hint per owning store |
| HTTP 401 | `auth_failed` | no | live-verified shape `[LOCAL-prior]` |
| HTTP 403 | `auth_failed` | no | entitlement variant; message must not embed the raw body |
| HTTP 429 | `rate_limited` | yes | honor `Retry-After`, backoff ≥ 60 s |
| HTTP 5xx | `unexpected_response` | yes | retry once |
| Connect / DNS / timeout | `network_error` | yes | retry once |
| Non-JSON body / missing `config` | `unexpected_response` | no | schema drift |
| 2xx but no usable percent | success with **zero windows** → `status "unknown"` | — | never 0% |

**Refresh cadence**: the quota window moves on a 7-day scale and access tokens
churn hourly. Recommend **poll the endpoint at most every 15 minutes**, and never
more often than every 5 minutes even if the global 1-minute refresh option is
selected. Gate every poll on the local expiry check first: an expired credential
produces **no network traffic** (the result is a foregone 401). Single-flight per
provider; keep last-good data with the original `checkedAt` on errors.

## 11. Implementation gate

**One concrete condition**: at implementation time, an unexpired cached xAI OAuth
credential must exist in a local store **and** the adapter's `#[ignore]`d live test
(same endpoint, same header set, credential read read-only, no refresh) must return
HTTP 200 and parse ≥ 1 window before the provider is registered in Rate Limits
v0.4. The quota surface itself already satisfied its verification once (§4.2);
this gate keeps registry integration tied to a currently-valid credential rather
than to this document. Implementation-time gate status is tracked in §12.

---

## RADAR

**ADOPT** — the safe quota surface is live-verified (HTTP 200 + full schema +
rolling 7-day window + absolute reset + bearer/account scoping) using exactly the
request the user's installed harness already makes; the design is bounded
(read-only files, no cookies, no refresh, conservative normalization).

**Destination: Rate Limits v0.4.**

---

## 12. Implementation status (appended 2026-09-28 — no evidence above was altered)

**Status: IMPLEMENTED, registry integration gated.** Implemented on branch
`codex/grok-provider` in `src-tauri/src/grok.rs` (backend adapter + tests +
`#[ignore]`d live test) and `src/providers/grokProvider.ts` (frontend adapter +
tests); wired as the `get_grok_usage` Tauri command. The production registry
still contains the Grok **mock**; the swap to the live provider happens only
after the §11 gate passes. The error wire shape is the shared
`{ code, message, httpStatus?, transient? }` contract; Grok stays a bespoke
error struct (like Antigravity) because its contract needs provider-specific
codes (`credential_expired`, `credential_ambiguous`, `rate_limited`,
`auth_failed`, `network_error`) plus a 429 `retryAfterMs` hint that the shared
`ProviderError` type does not carry.

**Resolution decisions taken at implementation time** (the one ambiguous area):

- **Cross-store precedence skips unusable stores.** A store that is absent,
  unreadable, malformed, or holds no usable xAI OAuth entry (including
  API-key-only) is skipped and the walk continues down §1's order; the first
  store with a usable selected credential wins. An **expired** selection does
  not block lower stores, but nothing within that store is substituted for it;
  if no store yields a usable credential, the **highest-precedence** expired
  selection reports `credential_expired` (with masked account and re-auth
  hint), and a machine with no xAI OAuth credential at all reports
  `credential_missing`. Rationale: §1 documents the OpenCodex store as the only
  one with an unexpired token — a strict first-store-wins-even-if-expired rule
  would permanently pin the provider to a 23-day-old expired token, while the
  within-store no-substitution rule from the task brief is kept verbatim.
- **Canonical-store multi-entry**: the official CLI client-id key
  (`https://auth.x.ai::<grok-cli-device-flow-client-id>`) is the store's own primary selection when
  present; otherwise the first usable entry in file order wins (the exact read
  order of the installed fork's read-only detector). API-key-mode entries are
  skipped; `auth_mode` absent requires a `refresh_token` (the detector's own
  marker); an absent/unparseable `expires_at` counts as expired — no network.
- **OpenCodex multi-account without a resolvable active selection** (missing
  `activeAccountId` or one that matches no account, with several accounts
  present) is terminal `credential_ambiguous` — never guessed, never
  aggregated. A single account is unambiguous.
- **Expiry semantics**: `expires`/`expires_at` accept epoch seconds/milliseconds
  or ISO-8601 (the stores' real formats); a token within a 60-second guard
  margin of expiry is treated as expired, mirroring the harnesses' own
  validity checks. Expired/missing/unparseable ⇒ `credential_expired` with
  **zero network traffic**.

**Payload semantics**: `creditUsagePercent` absent (or `currentPeriod.type`
≠ `USAGE_PERIOD_TYPE_WEEKLY`) is §10's permitted zero-window success — status
"unknown", never 0%, and it does **not** retire the last-good snapshot (the
frontend keeps showing the retained windows under the "unknown" status).
Non-JSON body, missing `config`, or a wrong-typed percent is schema drift →
`unexpected_response`, but only after the legacy endpoint (§4.3, bearer +
`Accept` only) has been given one conservative fallback attempt; the weekly
and legacy schemas are never combined, and the weekly window always wins.
`on-demand` is appended only when `onDemandCap.val > 0` alongside a parsed
weekly window. Redirects are never followed (`Policy::none`) so the bearer
header cannot leak to a redirect target; a 3xx surfaces as a plain
`unexpected_response`.

**Cadence and retry (frontend adapter)**: the remote endpoint is polled at
most every **15 minutes** (the recommended normal cadence; the 5-minute
absolute floor is respected with margin) regardless of the global refresh
interval — within the window the adapter serves its cached last result with
its original `checkedAt`. Remote failures (network/HTTP) are cached for the
same window so a rate-limited endpoint is never hammered; local credential
failures are never cached, so a re-auth applies on the next poll. Transient
retry stays the standard one-retry semantics; a 429 carrying `Retry-After`
skips the fast in-call retry (it could not honor the hint) and lets the next
cadence poll serve as the retry.

**Account attribution**: the backend sends `x-userid` (stored account id, JWT
`sub` fallback — §6) and returns only the **masked** id (first 8 characters +
ellipsis) plus the owning store (`grok-cli` / `opencode` / `opencodex`) in
`usage.account`. The token itself never leaves the Rust process; `Debug` and
serde output are covered by secret-leak tests.

**Gate log**: the §11 gate could not run at implementation time — the only
valid credential (OpenCodex account `<redacted-account-id>`, stored expiry
`2026-09-28T19:30:28Z`) expired at 19:30Z, minutes before the gate attempt.
The live test was executed anyway and behaved exactly as required: it refused
the expired credential with `credential_expired` and made **zero** HTTP
requests. Per §11 the provider therefore remains unregistered
(IMPLEMENTED_NOT_REGISTERED) until a currently-valid credential exists and the
live test returns HTTP 200 with ≥ 1 parsed window; the owning harness (which
refreshed this credential twice today, §1/§3.4) can restore eligibility
without any user login beyond using the harness itself.
