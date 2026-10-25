/**
 * Per-wallet preferences (v0.8.17 wallet configuration) - two independent
 * controls per prepaid wallet:
 *
 * - `monitored`: whether the runtime collects the wallet at all. Off means
 *   no automatic polling and no API request for that wallet — it is
 *   enforced by the Rust runtime (the settings lane mirrors the flags into
 *   a persisted Rust store before the first collection cycle). Disabling
 *   monitoring keeps the wallet's last known balance and never deletes
 *   historical data.
 * - `floatingBar`: whether the wallet appears in the floating API Wallets
 *   card. Presentation only, exactly like provider hiding: a hidden wallet
 *   keeps being monitored, and a monitored wallet stays displayable with
 *   no other configuration.
 *
 * Identity is always the stable wallet id ("openrouter", "deepseek"),
 * never the display name. An id with no explicit entry falls back to the
 * registry default: real collectors (OpenRouter, DeepSeek) are monitored
 * and visible, so existing users keep today's behavior unchanged.
 */

export type WalletPreferences = {
  /** Explicit per-wallet monitor flags. Absent = the wallet's default. */
  monitored: Record<string, boolean>;
  /** Explicit per-wallet floating-bar visibility. Absent = default. */
  floatingBar: Record<string, boolean>;
};

/**
 * Registry defaults for wallets that are not plain prepaid collectors.
 *
 * OpenAI is a registered placeholder, not a collector: the public OpenAI
 * API documents organization usage and cost endpoints but no prepaid
 * remaining-credit balance endpoint. It defaults to unmonitored and
 * hidden, and the settings drawer states why instead of offering a toggle
 * that cannot work.
 */
const WALLET_DEFAULTS: Record<
  string,
  { monitored: boolean; floatingBar: boolean }
> = {
  openai: { monitored: false, floatingBar: false },
};

const DEFAULT_FOR_KNOWN_WALLET: { monitored: boolean; floatingBar: boolean } =
  { monitored: true, floatingBar: true };

/** A registered wallet's defaults when no explicit preference exists. */
export function walletDefaultsFor(id: string): {
  monitored: boolean;
  floatingBar: boolean;
} {
  return WALLET_DEFAULTS[id] ?? DEFAULT_FOR_KNOWN_WALLET;
}

/**
 * The user-facing explanation for a registered wallet that cannot collect
 * a balance, when one exists. Rendered in the settings drawer in place of
 * a working monitor toggle.
 */
export function walletRegistryNote(id: string): string | null {
  if (id === "openai") {
    return (
      "The public OpenAI API exposes usage and cost reporting, not a " +
      "prepaid remaining balance, so LimitScope cannot show a live " +
      "OpenAI balance."
    );
  }
  return null;
}

export const DEFAULT_WALLET_PREFERENCES: WalletPreferences = {
  monitored: {},
  floatingBar: {},
};

/** Whether the wallet is collected: explicit flag first, then its default. */
export function isWalletMonitored(
  prefs: WalletPreferences | null | undefined,
  walletId: string,
): boolean {
  const explicit = prefs?.monitored?.[walletId];
  if (typeof explicit === "boolean") return explicit;
  return walletDefaultsFor(walletId).monitored;
}

/** Whether the wallet appears in the floating API Wallets card. */
export function isWalletInFloatingBar(
  prefs: WalletPreferences | null | undefined,
  walletId: string,
): boolean {
  const explicit = prefs?.floatingBar?.[walletId];
  if (typeof explicit === "boolean") return explicit;
  return walletDefaultsFor(walletId).floatingBar;
}

function cleanFlagRecord(value: unknown): Record<string, boolean> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, boolean> = {};
  for (const [id, flag] of Object.entries(value as Record<string, unknown>)) {
    if (typeof id !== "string" || id.trim().length === 0) continue;
    if (typeof flag !== "boolean") continue;
    out[id] = flag;
  }
  return out;
}

/**
 * Tolerantly parses persisted preferences. Unknown shapes and malformed
 * entries collapse to safe defaults instead of failing: existing users
 * without stored preferences get empty records, which resolve every
 * registered wallet to its registry default.
 */
export function sanitizeWalletPreferences(value: unknown): WalletPreferences {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { ...DEFAULT_WALLET_PREFERENCES };
  }
  const record = value as Partial<WalletPreferences>;
  return {
    monitored: cleanFlagRecord(record.monitored),
    floatingBar: cleanFlagRecord(record.floatingBar),
  };
}

/** A new preferences object with one wallet's monitor flag set explicitly. */
export function withWalletMonitored(
  prefs: WalletPreferences,
  walletId: string,
  monitored: boolean,
): WalletPreferences {
  if (prefs.monitored[walletId] === monitored) return prefs;
  return {
    monitored: { ...prefs.monitored, [walletId]: monitored },
    floatingBar: prefs.floatingBar,
  };
}

/** A new preferences object with one wallet's floating-bar visibility set. */
export function withWalletFloatingBar(
  prefs: WalletPreferences,
  walletId: string,
  visible: boolean,
): WalletPreferences {
  if (prefs.floatingBar[walletId] === visible) return prefs;
  return {
    monitored: prefs.monitored,
    floatingBar: { ...prefs.floatingBar, [walletId]: visible },
  };
}
