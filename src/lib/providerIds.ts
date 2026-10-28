/**
 * Provider id vocabulary. A provider can have more than one configured
 * account, and account-scoped ids extend the base id with a colon:
 * "openai-codex" is the primary Codex account and "openai-codex:secondary"
 * is the second one. Both belong to the same provider.
 *
 * Every call site that needs "which provider is this?" goes through
 * `baseProviderId`, so the split on ":" lives in exactly one place and an
 * account-scoped id can never be mistaken for a different provider.
 */

export const CODEX_PROVIDER_ID = "openai-codex";
export const CODEX_SECONDARY_PROVIDER_ID = "openai-codex:secondary";

/**
 * The provider part of an id: everything before the FIRST colon. Splitting
 * on the first colon keeps any later colons inside the account part, and an
 * id without a colon is already a base id. Never throws, so malformed
 * persisted values degrade to a harmless non-matching string.
 */
export function baseProviderId(providerId: string): string {
  const index = providerId.indexOf(":");
  return index === -1 ? providerId : providerId.slice(0, index);
}

/**
 * True for the primary Codex id and for every account-scoped Codex id. The
 * provider check is by base id, so callers do not need to know how many
 * Codex accounts exist.
 */
export function isCodexProviderId(providerId: string): boolean {
  return baseProviderId(providerId) === CODEX_PROVIDER_ID;
}

/**
 * True only for account-scoped Codex ids. The UI uses this to tell "one of
 * several accounts of the same provider" apart from the primary account,
 * which keeps the unqualified id.
 */
export function isCodexAccountScopedId(providerId: string): boolean {
  return providerId.includes(":") && isCodexProviderId(providerId);
}

/**
 * The only two configured Codex account ids, in display order. Persisted
 * account lists are normalised against this list so the UI always has both.
 */
export const CODEX_ACCOUNT_IDS: readonly string[] = [
  CODEX_PROVIDER_ID,
  CODEX_SECONDARY_PROVIDER_ID,
];
