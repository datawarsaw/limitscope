# Provider defaults candidate — CoWork and the default provider order

Local source candidate, not released. Branch
`feature/claude-provider-ui-defaults-20261117`, based on the released
v0.8.25 commit `bdef586b1b94c39d854dbe32402e2a2f1e596a51`. No version
surface, installer, signature, tag or published artifact is touched.

## What this candidate implements

### The second Codex account's initial editable values

The second OpenAI/Codex account now starts with an initial name and its own
initial Codex home:

| Field | Initial value |
| --- | --- |
| Name | `CoWork` |
| Codex home | `C:\codex-home-second` |

They are INITIAL values, not a stored configuration. They apply only while
the second account has never been configured — that is, while it has no
Codex home of its own, the exact state in which the account cannot be read
at all. Nothing a user saved is ever replaced: a saved name stays, a saved
Codex home stays, and the primary account's home, identity and monitoring
flag are untouched.

The account keeps its own credential directory: the account owner creates
that folder and logs that account in with the Codex CLI inside it.
LimitScope only ever reads the resulting `auth.json`. No auth file is
written, copied or shared, and the two accounts never resolve to the same
credential home — the existing collision check rejects that configuration.

The same rule lives in both halves of the contract, so the WebView cache and
the Rust store cannot disagree:

- `src-tauri/src/codex_accounts.rs` — `default_for` and
  `apply_secondary_initial_defaults`;
- `src/lib/codexAccounts.ts` — `DEFAULT_CODEX_ACCOUNT_PREFERENCES` and
  `applySecondaryAccountDefaults`.

### The default provider order

A configuration that has no custom order now uses the confirmed default:

1. Codex
2. Codex · CoWork
3. Antigravity
4. Z.ai
5. OpenCode
6. Grok
7. Qwen
8. MiniMax
9. OpenDesign

The list lives in `DEFAULT_PROVIDER_ORDER` (`src/lib/providerPreferences.ts`)
and is applied by `effectiveProviderOrder`. It is a DEFAULT, not a stored
configuration:

- a persisted custom order always wins and is never rewritten;
- a persisted visibility choice is never rewritten — hiding is a separate
  preference, and a hidden provider keeps its place in the list;
- a provider the default does not name (one added by a later version) is
  appended in registry order, so it appears instead of disappearing.

## Claude Code monitoring — BLOCKED

Claude is NOT part of this candidate. No Claude provider, collector, API
call or placeholder was added, and no Claude entry exists in the default
order.

The blocker is provenance, not effort. Claude Code exposes no public,
documented quota source that a third-party desktop app may read:

- The installed CLI (2.1.318) publishes no quota command. `claude --help`
  lists no `usage` command, and `claude usage --help` is not a command.
  `claude auth status` reports authentication, not quota headroom.
- The only known source for Claude Code subscription headroom is the
  undocumented OAuth usage endpoint, read with the user's own Claude OAuth
  access token from the local credential store. That is a private endpoint
  reached with a copied token, which is outside the allowed boundary.
- Anthropic's documented usage and cost endpoints are organization Admin
  API endpoints. They report API usage and cost, not Claude Code
  subscription quota, and they require an admin key the user must create.
- The local session transcripts hold per-session token counts. Those are
  local session facts, not subscription headroom, and presenting them as
  subscription quota would be a false reading.

A provider that cannot report a real quota must not be added: the honest
state here is BLOCKED, not a PASS and not an "unavailable" placeholder.

The insertion point is one line in `DEFAULT_PROVIDER_ORDER`: Claude belongs
in third position, between `openai-codex:secondary` and `antigravity`, once
a collector with a verified, permitted data source exists.

## Verification

- Frontend, targeted: 85 PASS across
  `src/lib/providerPreferences.test.ts` (28),
  `src/lib/codexAccounts.test.ts` (19),
  `src/lib/settings.test.ts` (31) and
  `src/App.providerPreferences.test.tsx` (7).
- Rust, targeted: 32 PASS / 0 FAIL for the `codex_accounts` module.
- `git diff --check`: clean.

Human Acceptance is pending. A native session must confirm that the second
account appears as `Codex · CoWork`, that the provider list shows the nine
providers in the order above, that a saved custom order and a saved
visibility choice both survive a restart, and that the second account
reports an honest error while its Codex home does not exist yet.
