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

The key is read from the Windows Credential Manager entry the account already
uses:

- target `ai.mistral.vibe`
- account name `MISTRAL_API_KEY`

LimitScope never writes, copies, rotates or migrates that entry. The secret
is resolved into a value whose `Debug` renders a fixed placeholder and whose
`Drop` overwrites the buffer; it is used exactly once, as a bearer token.
Only two things leave the adapter: the SHA-256 fingerprint of the key (the
account identity the runtime isolates last-good by) and a masked four
character tail. A missing entry, an unreadable store, an entry under a
different account name and an empty secret all fail closed — the provider is
*unavailable*, never a zero.

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

## Error policy

| Condition | Verdict | Health |
|---|---|---|
| no / unreadable / foreign credential | `credential_missing`, `credential_unreadable`, `credential_ambiguous` | unavailable |
| 401 | `auth_invalid` | unavailable |
| 403 | `not_entitled` | error |
| 402 | `billing_restricted` — a billing refusal of the *reading*, never an invalid-key verdict | error |
| 408, 429, 5xx, transport timeout | transient | error, last-good retained |
| malformed body | `unexpected_response` | error |

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
