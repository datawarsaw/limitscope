# LimitScope v0.8.15 — Release Notes

This release adds prepaid API wallets. An OpenRouter account's remaining USD
balance now appears beside the quota providers, read from the official credits
endpoint and kept strictly separate from quota.

## Highlights

- **OpenRouter prepaid balance.** The remaining balance is read from
  `GET https://openrouter.ai/api/v1/credits` and reported as
  `total_credits - total_usage` in USD. The credential comes from the
  OpenCode auth entry that the OpenCode Go backend already uses, so no new
  credential store is introduced and the key never leaves the process.
- **A wallet is money, not quota.** Wallets live in their own snapshot list,
  separate from `providers`. A wallet has no percentage, no reset window and
  no period, and it never enters quota history, prediction or notifications.
- **A balance is never fabricated.** A missing, malformed, negative or
  non-finite amount is an explicit error, not a 0.00 reading. A real zero
  (usage equal to credits) is a valid observation and is preserved.
- **The refresh is credential-scoped.** The last-known balance is retained per
  credential, identified by a SHA-256 fingerprint of the key. If the
  credential changes during a refresh, the previous observation is discarded
  instead of being shown against the new account.
- **The floating bar gained a compact wallet slot.** It adds 44 px beside the
  unchanged provider meters. The resting dock stays 600 px wide and shows
  meters only; the floating window's declared native maximum is 646 px,
  matching that widest layout, and a regression test pins the two together.
- **Existing providers are unchanged.** Codex, OpenCode Go, Z.ai, Antigravity,
  Grok, Qwen and OpenDesign keep their established behavior, trust boundaries
  and updater signing configuration.

## Operational condition

The wallet balance requires a usable OpenRouter credential in the OpenCode
auth file. When it is absent or the endpoint cannot be reached, the wallet
reports that state explicitly and shows no amount; it never substitutes a
stale or estimated figure.

The updater feed carries these notes after the signed release workflow verifies
the exact tagged source SHA.
