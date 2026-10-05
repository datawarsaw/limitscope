# Provider Discovery — real quota sources for the Rate Limits app

Discovery for the five remaining providers: **Z.ai**, **OpenCode Go**, **xAI Grok**, **Claude Code**, **Google Gemini / Antigravity**.
No application code was changed. Nothing was authenticated to, no credential values were read or copied — only file/key names, non-secret structures, and public sources.

**Evidence tiers used throughout:**

| Tag | Meaning |
| --- | --- |
| `[LOCAL]` | Verified on this machine (file present, structure inspected, values redacted) |
| `[UPSTREAM]` | Verified from official docs or open-source code (repo/file named) |
| `[INFERRED]` | Reasonable conclusion, not yet confirmed by execution or source |

---

## 1. What is actually on this machine (inventory)

| Provider | Installed? | Local state found |
| --- | --- | --- |
| Z.ai (GLM Coding Plan) | Yes — ZCode desktop + CLI, active (`account:zai-start-plan`) | `~/.zcode/v2/credentials.json`, `~/.zcode/v2/coding-plan-cache.json`, env `ZAI_*` |
| OpenCode Go | Yes — `opencode-ai@1.18.28` (npm) | `~/.local/share/opencode/auth.json` (has `opencode-go` API key), `opencode.db` (SQLite) |
| xAI Grok | Grok CLI **not** installed (`~/.grok` absent) | `~/.local/share/opencode/auth.json` has an `xai` **OAuth** entry (refresh token present, access token expired) |
| Claude Code | **Not installed** (`~/.claude/` has no credentials, no JSONL transcripts, no CLI on PATH) | — |
| Gemini / Antigravity | Antigravity CLI + IDE state present | `~/.gemini/oauth_creds.json`, `~/.config/opencode/antigravity-accounts.json` (contains cached quota), `~/.gemini/antigravity*/` |

No Windows Credential Manager entries exist for any of these providers (`cmdkey /list` checked) — everything is file-based on Windows.

---

## 2. Summary table

| Provider | Auth source | Quota source | Windows exposed | Reset timestamps | Confidence | Risk |
| --- | --- | --- | --- | --- | --- | --- |
| **Z.ai** | Plan API key in ZCode credential store `[LOCAL]` | `GET api.z.ai/api/monitor/usage/quota/limit` `[UPSTREAM]` + ZCode `zcode-plan/billing/*` plane `[LOCAL]` | 5h time limit, token pool, per-model usage (limits array) | **Yes** — `nextResetTime` epoch ms | High | Medium |
| **OpenCode Go** | `opencode-go` API key in `auth.json` `[LOCAL]` | `GET opencode.ai/zen/go/v1/usage` `[UPSTREAM]` | Rolling 5h, weekly, monthly (used %) | **Yes** — `resetsAt` ISO-8601 | High | Low–Med |
| **xAI Grok** | OAuth token in `~/.grok/auth.json` (CLI) or opencode `xai` OAuth entry `[LOCAL/UPSTREAM]` | `GET cli-chat-proxy.grok.com/v1/billing?format=credits` `[UPSTREAM]`; API-key users: `x-ratelimit-*` headers only `[UPSTREAM]` | Weekly/monthly pool (subscription); per-minute TPM/RPS (API) | **Yes** (subscription: period end); durations for API headers | Medium | **High** |
| **Claude Code** | OAuth in `%USERPROFILE%\.claude\.credentials.json` (`claudeAiOauth`) `[UPSTREAM]` | Response headers `anthropic-ratelimit-unified-{5h,7d}-{utilization,reset}` `[UPSTREAM]`; ccusage JSONL as offline fallback `[UPSTREAM]` | 5-hour + 7-day (utilization 0–1) | **Yes** — epoch seconds | High (headers) / Medium (overall, not testable locally) | Medium |
| **Gemini / Antigravity** | Google OAuth refresh token in `~/.gemini/oauth_creds.json` / antigravity plugin store `[LOCAL]` | `cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` `[UPSTREAM]`; cached quota file readable locally `[LOCAL]` | 5h windows per family (`gemini`, `non-gemini`), tier info | **Yes** — `resetTime` ISO per window | High (Antigravity route) / Medium (standalone Gemini CLI) | Med (passive file: Low) |

---

## 3. Per-provider detail

### 3.1 Z.ai (GLM Coding Plan) — *this machine's active plan*

**1. Authentication.** Bearer API key, `Authorization: Bearer <key>`.
- `[LOCAL]` ZCode stores per-plan API keys in `~/.zcode/v2/credentials.json` (flat key-value store, 0600). Relevant keys (names only): `account-provider:coding-plan:account:zai-individual-coding-plan:account:<uuid>:api-key`, same for `zai-team-coding-plan`, plus `oauth:zai:access_token` (JWT), `zcodejwttoken`, `oauth:active_provider`.
- `[LOCAL]` Env vars present: `ZAI_BUSINESS_BASE_URL`, `ZAI_OAUTH_CLIENT_ID`, `ZAI_OAUTH_ORIGIN`.
- `[UPSTREAM]` Community tools accept `ZAI_API_KEY` env or `--key`; zai-quota also reads `~/.hermes/auth.json`.
- No Credential Manager involvement. `[LOCAL]`

**2. Usage/quota source.**
- `[UPSTREAM]` **`GET https://api.z.ai/api/monitor/usage/quota/limit`** — unofficial monitoring endpoint; verified from `SeeYangZhi/zai-quota` (`zai_quota.py`), corroborated by `go-z-ai`, `unfixed3854/pi-usage`, and the dsh-quota-panel mapping. China region fallback: `https://open.bigmodel.cn/api/monitor/usage/quota/limit`.
- `[LOCAL]` ZCode desktop's own plan UI (`coding-plan-usage-button`, `coding-plan-health-chart` strings) calls the ZCode plane: `https://zcode.z.ai/api/v1/zcode-plan/billing/current`, `/billing/balance`, `/billing/preview`, `/billing/claim`, plus `api/v1/coding-plan/reset` — extracted from the installed `app.asar`. This is the same data the official app shows, but it is an internal, authenticated-by-ZCode-session surface.
- Local logs: no usage data cached in `~/.zcode/v2` (only plan *entitlements* in `coding-plan-cache.json` — statuses like `coding_plan_not_entitled`). `[LOCAL]`

**3. Limit windows.** `[UPSTREAM]` Response `data.limits[]` entries carry `type` ∈ {`TIME_LIMIT`, `TOKENS_LIMIT`, `RATE_LIMIT`, `TIMES_LIMIT`, `SESSION_LIMIT`} — i.e. a 5h time window, a token pool, and tool-call limits; `usageDetails[]` gives per-model counts. Exact windows vary by plan level (`data.level`: lite/standard/pro…).

**4. Reset timestamps.** **Yes** — `limits[].nextResetTime` (epoch **milliseconds**). `[UPSTREAM]`

**5. Authoritative?** **Yes** — server-reported used percentage, same source the z.ai console shows. `[UPSTREAM]`

**6. Security.** The plan API key is a bearer credential stored in plaintext; it can also drive model calls. Read it, never log it, never send it anywhere except `api.z.ai` / `open.bigmodel.cn`. The monitor endpoint is **unofficial** — polling it is tolerated but not contractual.

**7. Stability risk.** **Medium** — unofficial endpoint, unchanged long enough that 4+ independent tools depend on it; Z.ai can change it without notice.

**8. Implementation difficulty.** **Low–Medium** — one GET + JSON parse; the only wrinkle is sourcing the key from the ZCode credential store (flat JSON, easy) with an env-var fallback.

Runtime confirmation pending: the exact `limits[]` payload for *this* account was not fetched (no authenticated calls were made during discovery). `[INFERRED]` that the coding-plan key works on the monitor endpoint — zai-quota's key-resolution order implies plan keys are accepted.

---

### 3.2 OpenCode Go

**1. Authentication.** API key ("Zen/Go key").
- `[LOCAL]` `~/.local/share/opencode/auth.json` → entry `opencode-go: { type: "api", key: "<67 chars>" }`. Sibling entries (`openai` oauth, `openrouter` api, `xai` oauth, `google` oauth) confirm the schema.
- `[UPSTREAM]` Schema verified in `anomalyco/opencode` `packages/opencode/src/auth/index.ts`: `Oauth{refresh, access, expires, accountId?}` / `Api{key}` / `WellKnown{key, token}`; file is `XDG data dir / opencode/auth.json`, written 0600; env override `OPENCODE_AUTH_CONTENT`.

**2. Usage/quota source.**
- `[UPSTREAM]` **`GET https://opencode.ai/zen/go/v1/usage`** with `Authorization: Bearer <go key>` (+ `Accept: application/json`; `x-opencode-session` header expected since 09/06 per `can1357/oh-my-pi`). Verified from the upstream server route `anomalyco/opencode` → `packages/console/app/src/routes/zen/go/v1/usage.ts` (PR `anomalyco/opencode#16513`) and 5+ independent clients: `xiufengsun/TokenTracker` (`opencode-go-limits.js`), `steipete/CodexBar` (`docs/opencode.md`), `stablyai/orca`, `openchamber/openchamber`, `robinebers/openusage`, Raycast agent-usage.
- Response: `usage.rolling` / `usage.weekly` / `usage.monthly` = `{ status: "ok"|"rate-limited", percent: <used 0–100>, resetsAt: <ISO-8601> }`. Legacy shape also seen: `rollingUsage/weeklyUsage/monthlyUsage` + `usagePercent` + `resetInSec`. Errors: 401 `AuthError` (bad key), 403 `EntitlementError` (no Go plan); a 200 HTML page means sign-in redirect — treat as unauthorized.
- Fallback (estimate only): `~/.local/share/opencode/opencode.db` (SQLite, `part` table, assistant rows with `cost`, `tokens`, `modelID`) — device-local cost estimate, never account quota. `[LOCAL]` DB present, 23 MB.
- Console endpoints exist (`opencode.ai/console/api/go/status` with cookie + `x-org-id` → `meters.fiveHour/week/month.{usedMicroCents, limitMicroCents}`) but need browser cookies — not recommended for a tray app. `[UPSTREAM]` (CodexBar docs)
- **No public API documentation exists**; open feature request `anomalyco/opencode#31084` asks for exactly what the internal route already serves. `[UPSTREAM]`

**3. Limit windows.** Rolling 5h, weekly, monthly (monthly anchors to subscription anniversary). `[UPSTREAM]` — matches the registry mock (`Weekly` + `30-day`).

**4. Reset timestamps.** **Yes** — `resetsAt` ISO-8601 per window (server-computed). `[UPSTREAM]`

**5. Authoritative?** **Yes** — server-side used percent. `[UPSTREAM]`

**6. Security.** Read one 0600 JSON file; send the key only to `opencode.ai`. Lowest-risk provider of the five.

**7. Stability risk.** **Low–Medium** — the route is served by the official console and consumed by many tools, but it is not a documented public API; a response-shape change is possible (both shapes already observed in the wild).

**8. Implementation difficulty.** **Low** — read `auth.json`, one GET, map three windows straight onto `LimitWindow`.

#### 3.2.1 Account model — why the card shows one account (MIC-297)

Investigated 2026-09-28 after a report that the card showed "the" OpenCode Go state while two OpenCode accounts were logged in (one exhausted, one ~50%).

**What OpenCode actually stores locally (verified in `anomalyco/opencode` v1.18.28 sources):**

- `auth.json` is `Record<providerID, Auth.Info>` — **exactly one credential per provider id**. Every official flow that writes a Go key (`opencode auth login`, TUI `/connect`, desktop connect → `auth.set` on legacy servers, `Credential.create` on v2 servers — which *deletes all rows for the integration* before inserting) **replaces** the single `opencode-go` entry. Two coexisting Go keys are therefore not representable in local storage; the stored key is simply the one last connected.
- OpenCode *does* support multiple console accounts (`opencode account login`, device flow; rows with `email` in the `opencode.db` `account` table, single active account in `account_state`). But those are **console OAuth tokens, not Go keys**: the Go usage route joins the bearer key to `KeyTable → (userID, workspaceID)` and accepts nothing else, and only the *active* console account's remote config (`/api/config`) is merged into a session — it is never written back to `auth.json`. Mapping console accounts to Go keys would require reading and refreshing stored OAuth material, which the app must never do.
- The usage response carries **no identity fields** (verified live: `{usage: {rolling, weekly, monthly}}` only), so even the stored key cannot be resolved to an email locally.

**Decision (per the "prove attribution or don't aggregate" rule):** show only the stored account, attributed as precisely as locally possible:

- The Rust backend derives a `keyHint` = **last four characters** of the stored key — the same masking the OpenCode console itself prints (`first8 + *** + last4` in `workspace/[id]/new-user-section.tsx`), so users can match the card to a key in their console. Keys shorter than 12 chars or with non-key-alphabet tails get no hint.
- The card renders `Account key ••1234` plus a coverage note: *other OpenCode accounts are not included*.
- A non-string `key` in the `opencode-go` entry (ambiguous credential material) fails loudly as `credential_ambiguous` instead of silently using "the first value".
- No aggregation, no averaging, no invented "Account 1/2" labels, and **no `provider → accounts → windows` schema rewrite**: with upstream storing ≤ 1 key per provider, that layer could never be populated honestly. The generic `ProviderUsage.account?` attribution is the extension point if a future provider with real multi-credential local storage appears.
- Known limitation (unchanged, documented): local 24h prediction history is keyed `providerId|windowLabel` and does not distinguish a credential swap (reconnecting a different account's key keeps the same history series).

---

### 3.3 xAI Grok

Two distinct credential worlds; the tray app must pick based on what the user has.

**A. SuperGrok / Grok Code subscription (OAuth — the likely case on this machine)**

**1. Authentication.** `[LOCAL]` opencode's `xai` entry (`type: "oauth"`, `refresh` token present, `access` expired) uses the same device-flow OAuth as the official Grok CLI: `[UPSTREAM]` `POST https://auth.x.ai/oauth2/device/code`, client id `b1a00492-073a-47ea-816f-4c329264a828`, refresh via `POST https://auth.x.ai/oauth2/token`. Official CLI stores it in `%USERPROFILE%\.grok\auth.json` — `{ key (access token), refresh_token, auth_mode: Oidc|ApiKey, user_id, team_id, expires_at }`, no OS keyring (`xai-org/grok-build` → `xai-grok-login/src/credential_provider.rs`). `[LOCAL]` grok CLI itself is not installed here; the opencode OAuth entry is the available credential.

**2. Usage/quota source.** `[UPSTREAM]` **`GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`** with `Authorization: Bearer <access token>` + header **`x-xai-token-auth: xai-grok-cli`**. Verified from `xiufengsun/TokenTracker` (`grok-limits.js`), `steipete/CodexBar` (`GrokWebBillingFetcher.swift`), `Th0rgal/sandboxed.sh`. Fields: `creditUsagePercent` (used %), fallback `productUsage[].usagePercent`, windows `currentPeriod.{start,end,type}`, on-demand sub-window `onDemandCap`/`onDemandUsed`, plan `subscriptionTier` (also `/v1/settings` → `subscription_tier_display`). This is the official CLI's own billing plane — reverse-engineered from its traffic, not documented.

**3. Limit windows.** Unified weekly/monthly credit pool + separate on-demand cap. `[UPSTREAM]`

**4. Reset timestamps.** **Yes** — `currentPeriod.end` (ISO). `[UPSTREAM]`

**5. Authoritative?** **Yes** for the billing endpoint. `[UPSTREAM]`

**B. API-key users (api.x.ai)**

- `[UPSTREAM]` Official docs (`docs.x.ai/developers/rate-limits`) document per-model RPS + TPM tiers and 429s but **no header names and no quota-query endpoint** (verified by fetching the page).
- `[UPSTREAM]` Code-verified headers (OpenAI-style, **with** prefix): `x-ratelimit-limit-requests`, `x-ratelimit-limit-tokens`, `x-ratelimit-remaining-requests`, `x-ratelimit-remaining-tokens`, `x-ratelimit-reset-requests`, `x-ratelimit-reset-tokens` (`adaline/gateway`, `Wei-Shaw/claude-relay-service`, `janekbaraniewski/openusage` probes `GET /v1/models` to read them). Resets are **duration strings** ("1h30m"), not timestamps; not all headers appear on every endpoint. Unprefixed `ratelimit-*` claims in some articles could not be verified.
- Credits/balance: `GET api.x.ai/v1/api-key` returns `remainingCredits` but one Sept-2026 measurement got 401 with a plain inference key; the reliable route is the Management API (`management-api.x.ai`, `GET /v1/billing/teams/{team_id}/prepaid/balance`, inverted-cents ledger) which needs a **separate management key** most users don't have. `[UPSTREAM]`, mixed evidence

**6. Security.** OAuth access + refresh tokens (full Grok account scope); the `cli-chat-proxy.grok.com` plane is internal — sending user tokens there is exactly what the official CLI does, but it's unofficial for third parties. Token refresh adds the usual refresh-token handling burden.

**7. Stability risk.** **High** — undocumented internal endpoints, unspecified windows, and per-endpoint header inconsistency on the API side.

**8. Implementation difficulty.** **Medium–High** — OAuth refresh flow + unofficial billing endpoint; API-key fallback can only show transient TPM/RPS, which is poor material for a quota dashboard.

---

### 3.4 Claude Code

**1. Authentication.** `[UPSTREAM]` (official docs, `code.claude.com/docs/en/authentication`) — on Windows: `%USERPROFILE%\.claude\.credentials.json`, top-level key `claudeAiOauth` containing OAuth access/refresh tokens; macOS uses the Keychain ("Claude Code-credentials") instead. `[LOCAL]` **Not present on this machine** — Claude Code is not installed, so nothing here could be end-to-end verified.

**2. Usage/quota source.**
- `[UPSTREAM]` **Unified rate-limit headers** returned by `api.anthropic.com` on responses made with a subscription OAuth token: `anthropic-ratelimit-unified-5h-utilization` (decimal 0–1), `anthropic-ratelimit-unified-5h-reset` (epoch **seconds**, e.g. `1764554400`), and the `-7d-utilization` / `-7d-reset` pair. Example values visible in `anthropics/claude-code#12829`; format deep-dives at claudecodecamp.com and tzk.ar (statusline builders). They are response headers, so the app must issue its own small authenticated probe request (e.g. a minimal `/v1/messages` call or a cheap GET) and read the headers — it cannot passively watch Claude Code's traffic.
- `[UPSTREAM]` **ccusage** (`ryoppippi/ccusage`, ~18k stars) aggregates local JSONL transcripts (`~/.claude/projects/**/*.jsonl`) fully offline into 5-hour "blocks", daily/monthly reports, and live burn rate (`blocks --live`). Zero network, zero auth — but it is **inferred from token counts**, not server quota.
- No official usage/quota REST endpoint for subscription plans was found. `[UPSTREAM]` (absence)

**3. Limit windows.** 5-hour + 7-day rolling (subscription Max/Pro plans). `[UPSTREAM]`

**4. Reset timestamps.** **Yes** — epoch seconds in the `-reset` headers. `[UPSTREAM]`

**5. Authoritative?** **Yes** for the headers (server-computed utilization). ccusage-style JSONL aggregation is **estimated** (token counts × list prices — good proxy, not the plan's meter).

**6. Security.** The OAuth access token is full Claude Code account access; the app must implement refresh-token handling and never persist or log tokens. Probe requests consume a (tiny) slice of quota.

**7. Stability risk.** **Medium** — the headers are emitted by Anthropic's edge and widely relied upon, but they are undocumented and could change silently.

**8. Implementation difficulty.** **Low–Medium** — file read + token refresh + periodic probe + header parse. Slightly harder than Z.ai/OpenCode because of the refresh cycle, and it cannot be tested on this machine until Claude Code is installed.

---

### 3.5 Google Gemini / Antigravity

**1. Authentication.** Google OAuth refresh token.
- `[LOCAL]` `~/.gemini/oauth_creds.json`: `{ access_token, refresh_token, id_token, token_type, scope, expiry_date }` (Gemini CLI / Antigravity shared state; `google_accounts.json` holds the active account).
- `[LOCAL]` `~/.config/opencode/antigravity-accounts.json` (written by the `@cortexkit/opencode-antigravity-auth` opencode plugin): per-account `refreshToken`, `projectId: "aicode-consumers"`, `fingerprint.{deviceId, sessionToken, userAgent, apiClient: "antigravity-cli"}`.
- `[UPSTREAM]` Plugin OAuth scopes: `cloud-platform`, `userinfo.email/profile`, `cclog`, `experimentsandconfigs` (plugin dist inspected, v2.2.1).

**2. Usage/quota source.**
- `[UPSTREAM]` **`POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota`** (plus `loadCodeAssist` for tier) — verified in the plugin's code (35 + 48 string hits respectively). Also mirrored by `github.com/lbjlaq/Antigravity-Manager`, which the plugin links.
- `[LOCAL]` The plugin caches the result in `antigravity-accounts.json` → `cachedQuota` — captured on this machine (values shown are the user's own non-secret usage fractions):
  ```
  capturedTierId: "free-tier"          capturedPaidTierId: "g1-pro-tier"
  cachedQuota.gemini:      { remainingFraction: 0.90, resetTime: ISO, modelCount, windows: [{ window: "5h", remainingFraction, resetTime }] }
  cachedQuota.non-gemini:  { remainingFraction: 0.28, resetTime: ISO, modelCount, windows: [{ window: "5h", ... }] }
  cachedQuotaUpdatedAt: <epoch ms>
  ```
  ("non-gemini" = the Claude/Grok models Antigravity also serves.)
- Antigravity IDE itself (`%APPDATA%/Antigravity`, `%LOCALAPPDATA%/antigravity`) is an Electron profile with no clean quota file. `[LOCAL]`

**3. Limit windows.** 5-hour windows per model family (`gemini`, `non-gemini`), plus locally tracked `dailyRequestCounts` (date-keyed, per family). Standalone Gemini CLI free tier is documented as ~1000 model requests/day with `/stats` and `/usage` shown inside the CLI, but no public quota API for it was found — for the dashboard, the Antigravity-family quota is the real signal. `[LOCAL]` + `[UPSTREAM]` + `[INFERRED]`

**4. Reset timestamps.** **Yes** — `resetTime` (ISO-8601) per window/family. `[LOCAL]` (cached values) + `[UPSTREAM]`

**5. Authoritative?** **Yes** — server-reported remaining fraction. `[UPSTREAM]`

**6. Security.** Google OAuth refresh token with **broad scopes (`cloud-platform`)** — the most sensitive credential in this entire document. Prefer the passive route below when possible. `cloudcode-pa…v1internal` is an internal Google API surface.

**7. Stability risk.** **Medium–High** for actively calling `v1internal` (internal API, fast-moving product). **Low** for passively reading the plugin's cache file and freshness-checking `cachedQuotaUpdatedAt` (no network, no extra exposure).

**8. Implementation difficulty.** **Medium** (active: OAuth refresh + internal API shape) / **Low** (passive cache read; data already maps 1:1 onto `LimitWindow`).

---

## 4. Reference projects — what they actually do

| Project | Mechanism | Relevance |
| --- | --- | --- |
| **TokenTracker** (`xiufengsun/TokenTracker`) | Per-provider fetchers: `opencode-go-limits.js` (official usage endpoint → cookie dashboard scrape → local SQLite estimate, in that priority), `grok-limits.js` (Grok CLI billing plane) | Confirms the OpenCode Go and xAI mechanisms above |
| **TokenBar** (`Nanako0129/TokenBar`, now "Syrtis") | No provider APIs — local session-log parsing via Rust `tokscale-core` (`src/sessions/opencode.rs`, `src/paths.rs`) | The "local logs / inferred usage" pattern |
| **QuotaBar** (`majiayu000/quotabar`) | **Tauri v2 menubar app** monitoring Claude Code, Codex, Cursor, etc. — same stack as this project | Closest architectural sibling; related: `lazyfoxy33-dev/ai-agent-usage-widget` (ships a Windows version) |
| **ccusage** (`ryoppippi/ccusage`) | Offline JSONL aggregation (`~/.claude/projects/**`), 5-hour blocks, daily/monthly, live burn rate | Offline fallback for Claude; also has a Codex variant |
| (bonus) **CodexBar** (`steipete/CodexBar`) | Best-written protocol docs for OpenCode (`docs/opencode.md`), xAI (`docs/xai.md`), Grok billing | Use its docs as the protocol reference when implementing |
| (bonus) **codeburn** (`getagentseal/codeburn`, installed locally) | Local multi-CLI cost aggregation across 41 integrations; no provider quota endpoints in its bundle | Validates "local logs" as the estimate-only tier |

The mechanisms were verified independently — none of these were copied blindly; where multiple projects disagree, the discrepancy is noted (xAI header prefixes, OpenCode response shapes).

**Local-log evidence boundary.** Three of the patterns above (ccusage, TokenBar, codeburn) work by reading foreign-owned local transcript/log files. LimitScope has no such scanner today — its adapters read only their small, bounded credential/config files. If a future provider or evidence lane ever adopts local-log evidence, it inherits the dormant trigger contract in `docs/local-data-controls-v0.7.md` §14 (incremental, bounded, fail-closed, backlog-skipping, read-only, opt-in) before any scanning code is written; nothing in this discovery document authorizes one.

---

## 5. Recommended implementation order (next 3)

1. **Z.ai** — the actively used subscription on this machine, fully end-to-end testable here. One GET to `api.z.ai/api/monitor/usage/quota/limit` with the plan key from `~/.zcode/v2/credentials.json` (fallback: `ZAI_API_KEY` env). Returns `percentage` + `nextResetTime` directly. *(Low–Medium difficulty, Medium risk)*
2. **OpenCode Go** — cleanest integration of all five: key sits in `auth.json`, endpoint returns used-percent + `resetsAt` for rolling/weekly/monthly in one call; key is present locally so it can be tested immediately. Handle both observed response shapes. *(Low difficulty, Low–Medium risk)*
3. **Claude Code** — highest user demand, and the unified headers are authoritative with reset timestamps; but it requires OAuth refresh handling and a probe request, and it cannot be verified on this machine until Claude Code is installed (a functional blocker for testing, not for implementing). *(Low–Medium difficulty, Medium risk)*

**Then:** Gemini/Antigravity (start with the passive `cachedQuota` file read — near-zero cost, then optionally the active `retrieveUserQuota` call), and **xAI Grok last** — its SuperGrok billing plane is viable but internal, undocumented, and needs a full OAuth refresh loop; the API-key alternative exposes only transient TPM/RPS windows that don't fit a quota dashboard.

**Cross-cutting note for all adapters:** perform HTTP in a Rust/Tauri command (never in the WebView), treat every unofficial endpoint as schema-unstable (defensive parsing, feature-detect fields), and degrade to `status: "unknown"` rather than guessing percentages.

---

## 6. Inferred-evidence provenance trigger (dormant contract)

**Status: dormant — a trigger contract, not an active runtime requirement.** Nothing here describes shipped or planned functionality, and no field, enum, flag, catalog, or runtime logic exists or is being added. This section records, in advance, the conditions under which a *future* usage source whose quota numbers the vendor does not directly report may be adopted — so that an estimated number can never silently enter the same evidence path as a measured one. (Recorded 2026-10-03.)

### Current state (why this is dormant)

Every provider in the current production registry computes its quota percentages on the vendor's side; LimitScope only relays them:

| Provider | Quota value origin |
| --- | --- |
| OpenAI / Codex | `chatgpt.com/backend-api/wham/usage` (`src-tauri/src/codex.rs`) — server-reported usage |
| Z.ai | `api.z.ai/api/monitor/usage/quota/limit` (§3.1) — server-reported used % |
| OpenCode Go | `opencode.ai/zen/go/v1/usage` (§3.2) — server-side used percent |
| Google Antigravity | `v1internal` quota endpoint, or the plugin cache of that same server-reported fraction (§3.5) — with freshness verdicts |
| Grok (xAI) | `cli-chat-proxy.grok.com/v1/billing` (§3.3) — server-reported `creditUsagePercent` |

- **"Read locally" is not "inferred."** The Antigravity passive route reads a file on disk, but the number in it was computed by Google's server and cached by the plugin. Provenance follows *who computed the number*, not *where the file lives*; that route is measured data with freshness verdicts, not an estimate.
- Because every wire window is therefore measured, no `dataProvenance`-style field is required today, and no generic provider capability catalog is justified.

### Trigger condition

This contract activates the moment LimitScope adopts any usage source whose quota values are **not directly reported by the provider/vendor** — i.e. the percentage is derived on our side from other evidence. Shapes that would trigger it (examples only; none is planned, and each is already characterized in this document as estimate-only): ccusage-style JSONL transcript aggregation (§3.4), the OpenCode `opencode.db` cost-based fallback (§3.2), session-log reconstruction (the TokenBar pattern, §4), heuristic estimation, usage derived from activity data, or any fallback that estimates when a vendor endpoint fails.

### The contract

Any future non-vendor-measured quota source must:

1. **carry an explicit provenance marker on the wire** (a `dataProvenance`-style field, additive to the existing `ProviderUsage` shape next to `dataFreshness`) — naming it here is part of the trigger, not a pre-authorization to add it;
2. **distinguish at minimum `measured` (the vendor's meter) from `inferred` (LimitScope-derived)**, default-deny: a source without a marker cannot enter the measured path;
3. **never silently present inferred data as measured** — no Live label, no "Checked …" source note that implies a vendor read;
4. **be refused by prediction** (`visiblePrediction`, `src/lib/v03Integration.ts`) unless that feature is explicitly extended to support inferred evidence; the gate already refuses data it cannot date or trust, and inferred data joins that refusal list by default;
5. **be refused by notification eligibility** (`observations_from_usages` in `src-tauri/src/history.rs`, the one eligibility definition shared with `src-tauri/src/notifications.rs`) under the same rule — a threshold crossing computed from an estimate never fires a toast unless the lane explicitly supports inferred evidence;
6. **preserve visible uncertainty rather than converting inferred data into authoritative percentages** — estimates render as estimates, the same honesty rule as "degrade to `status: "unknown"` rather than guessing percentages" (§5).

This follows the precedent of "prove attribution or don't aggregate" (§3.2.1): a refusal rule that costs a feature is preferred to a silent blend of unequal evidence.

### Not this contract

- The `[LOCAL]` / `[UPSTREAM]` / `[INFERRED]` tags at the top of this document grade *discovery claims* (how we verified what we know). Data provenance grades *runtime quota values* (who computed the number). A `[LOCAL]` read of a vendor-computed cache is measured; a `[UPSTREAM]`-documented heuristic run on our side is inferred. The two axes are independent and must not be conflated.
- Adding the marker before any inferred source exists would be dead schema: the contract is the gate for the trigger, not a scaffold to build now.
- A future local-log source would also inherit the bounded-scanning trigger contract (`docs/local-data-controls-v0.7.md` §14): that contract governs *how* foreign log files may be read; this one governs *how an inferred quota value may enter the wire and the evidence path*. The gates are independent and both apply.
