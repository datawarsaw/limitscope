# Mistral monthly budgets (Vibe + API)

LimitScope reads two independent monthly budgets from the account's own
Mistral billing API and shows them as one Floating Deck segment.

## What is read

Two read-only GETs, issued together, against one canonical origin:

| Endpoint | Read |
|---|---|
| `GET https://api.mistral.ai/v1/billing/budget` | the `vibe_budget` and `api_budget` rows |
| `GET https://api.mistral.ai/v1/billing/subscription` | the plan name (`EDU`) |

No alternate host, no redirect target, no browser cookie, no admin API, no
POST/PUT/DELETE, and no inference call. The redirect policy is `none`, so a
3xx is a plain failure and the `Authorization` header never leaves the
canonical origin. The request budget is 15 s (the same budget the other live
backends use), the body is capped at 64 KiB, and the response must parse as
JSON.

## The credential

LimitScope checks two Windows Credential Manager generic targets in order:

1. `ai.mistral.vibe`
2. `MISTRAL_API_KEY@ai.mistral.vibe`

Both require the exact stored `UserName` `MISTRAL_API_KEY`. The second
lookup runs only when the base entry is absent or its metadata does not
match, for example when the base holds an MCP OAuth entry. When both
entries match, the base wins, following Windows Python keyring lookup
precedence. A store read error or an unusable payload on a matching entry
ends the lookup. LimitScope checks the target and user name before decoding
the payload, and performs no writes or migrations.

The key's `Debug` representation is a fixed placeholder and its secret
buffer is overwritten on drop. The adapter exposes a SHA-256 fingerprint
for account-bound last-good data and a masked four-character tail for
display.

## The two rows

Each row is measured on its own. A row is available only when every field it
needs is usable:

- a three-letter currency code,
- a budget total above zero,
- a used percentage inside 0–100,
- a reset that is either absent or a readable RFC-3339 instant.

Anything else makes that row **unknown**: it states no percentage and no
amount. It is never zero and never full, and it never borrows a number from
its sibling. The two budgets are never summed — they are two separate
allowances of the same account.

### Amounts

Amounts travel as integer cents in the row's own currency. The used amount
is the source's exact usage amount when the source reports one; otherwise it
is **derived** as `initial_budget x usage_percentage`, rounded to the cent.
The derived rows carry `amountDerived: true`, so a derived figure never reads
as an exact one the source did not state. The remaining amount is
`total - used`, never a second source field.

The source's reset instant is preserved verbatim; the UI reads it in the
viewer's own local time.

### Cold start

The persisted reading is re-validated at cold start, per row. A row whose
reset bound has already passed — or that cannot be read, or that lies
outside the runtime's reset plausibility horizon — hydrates as *unknown*
with no amount at all: its money belongs to a cycle that is no longer
running, so it is never presented as this month's spending. The check is
per row, so one expired budget never takes its sibling down and never
publishes a past cycle's EUR amounts. A row the source never gave a bound
for stays usable, exactly as in the live reading.

## Error policy

| Condition | Verdict | Health |
|---|---|---|
| both targets absent | `credential_missing` | unavailable |
| entry found, but neither target has matching metadata | `credential_ambiguous` | unavailable |
| store read error or unusable matching payload | `credential_unreadable` | unavailable |
| 401 | `auth_invalid` | unavailable |
| 403 | `not_entitled` | error |
| 402 | `billing_restricted` — a billing refusal of the *reading*, never an invalid-key verdict | error |
| 408, 429, 5xx, transport timeout | transient | error, last-good retained |
| malformed body | `unexpected_response` | error |

All three credential codes are local-state conditions: the runtime
classifies them as *unavailable* — never an error, never a zero. A refused
key (`auth_invalid`) keeps its source-absent classification, while a
permission refusal (`not_entitled`) and a billing refusal
(`billing_restricted`) stay errors.

A transient or deterministic failure keeps the previous reading only when it
belongs to the same account: the retained entry's identity is the
credential fingerprint, and a failure that could not resolve an identity
never reuses a fingerprinted reading. The plan is an independent read — a
plan that cannot be verified is absent, never guessed — while an auth
refusal on either endpoint fails the whole refresh.

## The display

One Floating Deck segment, `mistral`. **Vibe Budget** is the designated
primary window, so the segment's percentage is the Vibe remaining budget;
the API budget is its independent sibling. The expanded card lists both rows
with their own remaining percentage, their exact used/total/remaining amount
in EUR, their own reset, the verified plan chip, the account attribution and
the freshness of the reading. The generic window list is replaced by the
Mistral block so the same two pools are never shown twice.

The card rounds the remaining percentage to whole percent, exactly like every
other provider: a Vibe budget of EUR 0.39 used of EUR 127.50 reads as
`100% remaining` on the segment while the card states the exact money.

## Live evidence (2026-11-18, one bounded read, redacted)

One authenticated request pair through the adapter's own read path:

- observed at `2026-11-18T11:02:46Z`, read completed in ~250 ms;
- plan: `EDU` (verified by the subscription endpoint);
- account: masked hint `••ma9d`; credential fingerprint present;
- Vibe Budget: EUR 127.50 total, EUR 0.39 used, EUR 127.11 remaining,
  0.307 % used, reset `2026-12-01T00:00:00Z`, amount derived;
- API Budget: EUR 12.75 total, EUR 1.80 used, EUR 10.95 remaining,
  14.12 % used, reset `2026-12-01T00:00:00Z`, amount derived.

An earlier attempt with a tighter (8 s) budget timed out on the same path
and was classified as transient; the 15 s house-standard budget reads the
endpoint comfortably. The timeout path is the one the runtime already
handles: transient, account-bound last-good.

The historical baseline fixture used by the frontend tests (Vibe EUR 127.50
total / EUR 0.34 used, API EUR 12.75 total / EUR 1.80 used) is a fixed
sample of this account, not a live reading.

## Limits

- The provider is registered for every configuration. Without the credential
  it shows as unavailable; it can be hidden like any other provider.
- Only the EDU/API allowance is represented. The plan name is shown only when
  the source verifies it.
- No key rotation, no login flow and no credential migration: the account
  keeps owning its own credential store.
