/**
 * UI model of the two configured OpenAI/Codex accounts. In Tauri the
 * persisted Rust configuration is authoritative; WebView storage is a cache.
 *
 * Each account has its own CODEX_HOME, so each `auth.json` can be read
 * independently. The primary account falls back to the default Codex home,
 * which means it is always readable. The secondary account has no default
 * home, so it is readable only once the user points it at a directory.
 *
 * Identity is always the stable account id ("openai-codex",
 * "openai-codex:secondary"), never the alias. Visibility in the provider
 * strip belongs to the provider preferences, not to this model.
 */

import {
  CODEX_ACCOUNT_IDS,
  CODEX_PROVIDER_ID,
  CODEX_SECONDARY_PROVIDER_ID,
} from "./providerIds";

export type CodexAccountSettings = {
  /** Stable internal account id, one of CODEX_ACCOUNT_IDS. */
  accountId: string;
  /** User-facing name. May be empty; never shown beside compact bar percentages. */
  alias: string;
  /**
   * Absolute directory holding this account's own `auth.json`. Empty means
   * the default home for the primary account and not configured for the
   * secondary one.
   */
  codexHome: string;
  /** false = "Pause monitoring": the runtime stops collecting and the last reading freezes. */
  monitored: boolean;
};

export type CodexAccountPreferences = {
  accounts: CodexAccountSettings[];
};

/**
 * The initial editable values of the second Codex account.
 *
 * The account is offered with a name and a Codex home of its own, so the
 * user only has to create that folder and log that account in with the
 * Codex CLI. These are INITIAL values, not a stored configuration: they
 * apply only while the second account has no saved entry, and any
 * value the user saved — including a cleared home — always wins.
 *
 * The primary account is deliberately not part of this: it keeps its own
 * default Codex home and its own identity.
 */
export const SECONDARY_CODEX_ACCOUNT_DEFAULTS = {
  alias: "CoWork",
  codexHome: "C:\\codex-home-second",
} as const;

/**
 * Registry defaults for the two Codex accounts. Both start monitored. The
 * primary account keeps the single-login behavior it has always had, and
 * the secondary account starts on its own initial name and its own initial
 * Codex home, so it is readable on its own from the first run.
 */
export const DEFAULT_CODEX_ACCOUNT_PREFERENCES: CodexAccountPreferences = {
  accounts: [
    { accountId: CODEX_PROVIDER_ID, alias: "", codexHome: "", monitored: true },
    {
      accountId: CODEX_SECONDARY_PROVIDER_ID,
      alias: SECONDARY_CODEX_ACCOUNT_DEFAULTS.alias,
      codexHome: SECONDARY_CODEX_ACCOUNT_DEFAULTS.codexHome,
      monitored: true,
    },
  ],
};

function defaultAccount(accountId: string): CodexAccountSettings {
  const found = DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts.find(
    (account) => account.accountId === accountId,
  );
  return { ...(found ?? { accountId, alias: "", codexHome: "", monitored: true }) };
}

/**
 * One persisted entry. A PRESENT entry is the user's own configuration: a
 * field it lacks is neutral (empty name, empty home, monitoring on) — the
 * exact rule of the Rust store — and never receives the initial values.
 */
function sanitizeAccount(
  accountId: string,
  entry: Partial<CodexAccountSettings>,
): CodexAccountSettings {
  return {
    accountId,
    alias: typeof entry.alias === "string" ? entry.alias : "",
    codexHome: typeof entry.codexHome === "string" ? entry.codexHome : "",
    monitored: typeof entry.monitored === "boolean" ? entry.monitored : true,
  };
}

/**
 * Tolerantly parses persisted account preferences. The result always holds
 * both known accounts in fixed order, so a stored list that is missing an
 * entry, or has a bad one, still renders as a complete pair. Unknown ids are
 * dropped and duplicates keep their first occurrence, so the first value a
 * user saved wins over any later stale copy.
 */
export function sanitizeCodexAccountPreferences(value: unknown): CodexAccountPreferences {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { accounts: DEFAULT_CODEX_ACCOUNT_PREFERENCES.accounts.map((a) => ({ ...a })) };
  }
  const rawAccounts = (value as { accounts?: unknown }).accounts;
  const entries = Array.isArray(rawAccounts) ? rawAccounts : [];

  const firstByAccountId = new Map<string, Partial<CodexAccountSettings>>();
  for (const entry of entries) {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
    const accountId = (entry as { accountId?: unknown }).accountId;
    if (typeof accountId !== "string" || !CODEX_ACCOUNT_IDS.includes(accountId)) continue;
    if (firstByAccountId.has(accountId)) continue;
    firstByAccountId.set(accountId, entry as Partial<CodexAccountSettings>);
  }

  // Only an ABSENT account starts from its initial values. A saved entry —
  // including one whose home the user cleared — is never repopulated.
  const accounts = CODEX_ACCOUNT_IDS.map((accountId) => {
    const entry = firstByAccountId.get(accountId);
    return entry ? sanitizeAccount(accountId, entry) : defaultAccount(accountId);
  });
  return { accounts };
}

/**
 * The entry for one account id, or null for an unknown id. Returning null
 * instead of a default keeps a typo from silently reading the primary
 * account's settings.
 */
export function codexAccountSettings(
  accounts: readonly CodexAccountSettings[],
  accountId: string,
): CodexAccountSettings | null {
  return accounts.find((account) => account.accountId === accountId) ?? null;
}

/**
 * A new account list with one account patched. The input is never mutated,
 * so React state updates stay referentially honest. An unknown id returns
 * the input unchanged: inserting an account here would bypass the fixed
 * two-account contract.
 */
export function withCodexAccount(
  accounts: readonly CodexAccountSettings[],
  accountId: string,
  patch: Partial<Pick<CodexAccountSettings, "alias" | "codexHome" | "monitored">>,
): CodexAccountSettings[] {
  if (!codexAccountSettings(accounts, accountId)) {
    return accounts as CodexAccountSettings[];
  }
  return accounts.map((account) =>
    account.accountId === accountId ? { ...account, ...patch } : { ...account },
  );
}

/**
 * Whether the account can actually be read. The primary account always can,
 * because it falls back to the default Codex home. The secondary account needs
 * an explicit directory, since it has no default home of its own. A blank or
 * whitespace-only path counts as no path.
 */
export function isCodexAccountConfigured(account: CodexAccountSettings): boolean {
  if (account.accountId === CODEX_PROVIDER_ID) return true;
  return account.codexHome.trim().length > 0;
}
