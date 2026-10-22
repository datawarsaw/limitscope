# MiniMax Token Plan provider

LimitScope reads the MiniMax subscription (Coding Plan) quota from the
official MiniMax endpoint and presents it as a standard quota provider,
`minimax-token-plan`.

## Verified official API

| Item | Value |
|---|---|
| Endpoint | `GET https://www.minimax.io/v1/token_plan/remains` |
| Authentication | `Authorization: Bearer <Subscription Key>` (`sk-cp-…`) |
| Regional hosts | `https://www.minimax.io` (international), `https://api.minimax.io` (international alias), `https://www.minimaxi.com` (mainland China) |
| Plan page | <https://platform.minimax.io/subscribe/coding-plan> |
| Plan overview | <https://www.minimax.io/m-plan> |

The endpoint answers **HTTP 200** even for refusals; the refusal is inside
the body as `base_resp.status_code`:

- `0` — success.
- `1004` — no API secret key / not signed in (also the answer for a missing
  `Authorization` header).
- `1003` — the account is not a member of the requested group.

### Observed schema (live-verified 2026-10-22)

```json
{
  "model_remains": [
    {
      "model_name": "general",
      "start_time": 1792634466447,
      "end_time": 1792652466447,
      "remains_time": 17999988,
      "current_interval_status": 1,
      "current_interval_remaining_percent": 100,
      "weekly_start_time": 1792634466447,
      "weekly_end_time": 1793239266447,
      "weekly_remains_time": 604799988,
      "current_weekly_status": 1,
      "current_weekly_remaining_percent": 100
    }
  ],
  "base_resp": { "status_code": 0, "status_msg": "success" }
}
```

## Quota-window semantics

The measured reading, not an assumption:

- **Percentages are remaining, not used.** The wire contract carries
  `used_percent`, so the adapter converts explicitly: `used = 100 − remaining`.
- **`end_time` / `weekly_end_time` are absolute epoch milliseconds** and are
  the window boundaries. The observed interval span was exactly
  18 000 000 ms (5 h) and the weekly span exactly 604 800 000 ms (7 d).
- **`remains_time` / `weekly_remains_time` are relative seconds** (18000 s and
  604800 s) — a redundant countdown to the same boundary, never an epoch
  stamp. The adapter ignores them and uses only the absolute `end_time`.
- **The windows are rolling.** Both `start_time` values tracked the moment of
  the observation, so `end_time` is the truthful reset boundary.
- **`model_name: "general"` is the shared text-quota pool.** Media (`video`)
  and model-specific pools are separate allowances. The adapter reports the
  shared pool only, so one subscription is never counted twice. When a plan
  exposes only model-specific pools, exactly one non-media pool is accepted;
  several are treated as ambiguous rather than summed.
- **`current_interval_status` / `current_weekly_status` equal `3`** mean the
  window is unlimited. Such a window is reported as *no window* rather than
  as a fabricated 0 % used.

## Credential resolution

Read-only, in this process, nothing persisted:

1. `MINIMAX_SUBSCRIPTION_KEY` — an explicit subscription key override.
2. The MiniMax Code CLI's managed-login store
   (`~/.minimax/auth/prod/<region>/mcode-public/auth.json`). The store holds
   an OAuth **access token**, not the plan key. That access token is
   exchanged at `GET https://platform.minimax.io/backend/account/token_plan_credit`
   (mainland: `https://www.minimaxi.com/backend/account/token_plan_credit`),
   whose `api_key` field carries the account's own `sk-cp-…` subscription key.

The store's OAuth **refresh token is deliberately never used.** MiniMax
rotates the refresh token on every use: presenting it invalidates the copy
the MiniMax Code CLI holds and signs the user out of their own desktop app.
A refresh is the CLI's business, so this adapter only reads the access token
the CLI has already persisted. When that access token has expired, the
provider reports a deterministic `auth_expired` state and asks for a sign-in
instead of taking the session over.

A pay-as-you-go API key is **not** a subscription key. The two share the
`sk-` prefix but not the `sk-cp-` plan prefix, and the adapter rejects a
credential that is not a plan key as `not_entitled`.

## Presentation

- Provider id `minimax-token-plan`, name **MiniMax Token Plan**.
- Windows: `5-hour` and `Weekly`, both as used percent with the absolute
  reset timestamp.
- Rail short name: **MiniMax**.
- Floating bar: the **Weekly** pool is the primary window (the plan-level
  budget); the five-hour pool stays in the popover inventory. The bar's
  geometry is unchanged — MiniMax only adds a segment like every other
  provider.
- Floating mark: an `MM` monogram (the brand wordmark is not licensed into
  the app).
- Hiding the provider is one switch in Settings → Providers. Hiding never
  stops the runtime from refreshing it: history, prediction and notifications
  continue, exactly as for every other provider. That is what makes hiding
  MiniMax trivial after the one-month subscription ends.

## Errors

| Code | Meaning |
|---|---|
| `credential_missing` | No MiniMax credential on this machine. |
| `auth_expired` | The CLI's stored session expired — sign in again. |
| `auth_invalid` | The subscription key was refused. |
| `not_entitled` | The account has no active subscription. |
| `minimax_not_installed` | No MiniMax credential directory found. |
| `unexpected_response` | Schema change or malformed payload. |

All of these are deterministic (`transient: false`) and are never retried.
No message ever contains credential material; the key is scrubbed at the
error boundary by the shared `secret_scrub` layer.

## Live probe

`minimax_token_plan::tests::live_minimax_quota_returns_both_windows` is an
ignored test that needs a live plan:

```powershell
cd src-tauri
cargo test --locked live_minimax_quota -- --ignored --nocapture
```

