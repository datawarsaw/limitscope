/**
 * The quota-history type contract (v0.5 phase 2).
 *
 * History persistence itself moved into the Rust shared runtime
 * (src-tauri/src/history.rs): recording, retention, deduplication,
 * self-healing, account partitioning, and the clear mutation all live
 * there now, and the frontend consumes read results only (see
 * `src/lib/historyClient.ts`). This module keeps the two pieces the TS
 * side still needs: the observation shape the prediction engine consumes,
 * and the legacy localStorage key the one-time migration reads.
 */

/**
 * One percentage snapshot of a single quota window at a point in time.
 *
 * Wire-compatible with the Rust `QuotaObservation`
 * (src-tauri/src/history.rs): `get_history` returns exactly this shape, so
 * a history read can be passed straight into `predictWindows` (whose
 * engine-local type omits only the optional `account` field). A logical
 * quota window is identified by the `(providerId, account, windowLabel)`
 * triple — never by the label alone, because labels ("5h", "weekly")
 * repeat across providers.
 */
export type QuotaObservation = {
  providerId: string;
  windowLabel: string;
  /** Percent of the window already consumed, clamped to 0–100. */
  usedPercent: number;
  /** When this snapshot was taken, canonicalized to ISO-8601 UTC. */
  observedAt: string;
  /**
   * When the window is scheduled to reset. Optional: some providers omit
   * it, and an unparseable value degrades to absent rather than rejecting
   * the observation (matching the engine's tolerance).
   */
  resetAt?: string;
  /**
   * Stable account identity the observation belongs to (the provider's
   * `AccountAttribution.identity` token, e.g. `key:3456`). Optional:
   * providers that cannot prove an account stay unpartitioned. Observations
   * of different accounts of one provider never mix — burn rates, dedup,
   * and the per-window bound all key on this field, so replacing a stored
   * credential starts independent history instead of inheriting the
   * previous account's observations.
   */
  account?: string;
};

/**
 * The legacy localStorage blob this app wrote before v0.5 phase 2 moved
 * history into Rust. The migration (`import_legacy_history`) reads it once,
 * hands it to the Rust store, and removes the key after a successful
 * import; it is never written again.
 */
export const LEGACY_QUOTA_HISTORY_KEY = "rate-limits.quota-history.v1";
