export type LimitWindow = {
  label: string;
  usedPercent: number;
  resetAt?: string;
};

/**
 * Banked reset-credit observation for the Codex provider (v0.7).
 * Mirrors the Rust CodexResetCredits DTO (src-tauri/src/codex.rs).
 */
export type CodexResetCredits = {
  /** Explicitly reported banked balance. Never "replenishments available now". */
  bankedCredits: number;
  /**
   * Separate applicability field when the backend reports one.
   * Absent means unknown — never zero.
   */
  currentlyApplicable?: number;
  /**
   * RFC-3339 UTC time of the observation. Current only within
   * RESET_CREDITS_TTL_MS (see src/lib/resetCredits.ts).
   */
  checkedAt: string;
  /** Provenance; today always "codex-wham-usage". */
  source: "codex-wham-usage";
};

/**
 * ZCode reset-card observation (Z.ai provider only).
 * Mirrors the Rust ZCodeResetStatus DTO (src-tauri/src/zcode_reset.rs).
 *
 * These are specific GRANTS — each card targets one window (five-hour or
 * weekly) and carries its own expiry. Deliberately distinct from the Codex
 * banked-credit balance above: there is no common "available resets" meaning,
 * so the two shapes must never be merged or flattened.
 */
export type ZCodeResetTarget = "fiveHour" | "weekly";

export type ZCodeResetCard = {
  target: ZCodeResetTarget;
  /** RFC-3339 UTC expiry of the grant. Absent when the upstream stamp was
   * missing or implausible — the card itself still counts. */
  expiresAt?: string;
};

export type ZCodeResetStatus = {
  fiveHourCards: ZCodeResetCard[];
  weeklyCards: ZCodeResetCard[];
};

/**
 * Freshness of the underlying source data for providers that surface a
 * cached snapshot (e.g. a local quota cache file) instead of a live fetch.
 * Live providers never set it. "stale" also covers an indeterminate
 * snapshot time: unknown age must never read as fresh.
 */
export type DataFreshness = "fresh" | "stale";

/**
 * Attribution of the shown quota windows to one specific account of a
 * provider. Present only when the provider can prove which account the
 * windows belong to (a stable, display-safe identity — never a secret).
 * Absent means unattributed: the provider could not prove an identity.
 */
export type AccountAttribution = {
  label: string;
  /** Coverage statement for providers that may hold more accounts locally. */
  note?: string;
  /**
   * Stable, storage-safe identity token for partitioning local quota
   * history by account (e.g. `key:3456` — a fingerprint scheme + tail,
   * never secret material). Present only when the attribution is stable
   * enough to key persisted history on; absent attribution keeps a
   * provider's history unpartitioned, exactly as before.
   */
  identity?: string;
};

/**
 * Normalized provider health, computed once by the Rust runtime
 * (src-tauri/src/runtime.rs `ProviderHealth`) and consumed directly by the
 * UI — the frontend never reconstructs health from status/error/freshness
 * fragments. The full contract (definitions, transitions, history and
 * notification eligibility) lives in docs/runtime-status-contract.md.
 *
 * - "live": the current fetch succeeded and the data is within freshness
 *   policy. Never a "last-known" value.
 * - "stale": previously valid data retained for display that freshness
 *   policy no longer considers current (a persisted last-good maps here,
 *   never to live).
 * - "unknown": the provider answered but reported no usable quota windows.
 * - "cooldown": a server-directed cooldown is active, so the provider is
 *   intentionally not fetched; retained data may still be shown.
 * - "error": the current refresh attempt failed; last-good may be retained.
 * - "unavailable": nothing usable can be shown and the required local
 *   source/auth state is absent. Ordinary transient errors are never this.
 */
export type ProviderHealth =
  | "live"
  | "stale"
  | "unknown"
  | "cooldown"
  | "error"
  | "unavailable";

export type ProviderUsage = {
  id: string;
  name: string;
  /**
   * Provider-specific banked reset-credit observation (v0.7, Codex only).
   * Present only on a fresh, account-bound observation from the Codex usage
   * source. bankedCredits is an explicitly reported balance — never a count
   * of usable replenishments, windows, resets, or history transitions, and
   * it must never be derived from them. Other providers never set this.
   */
  resetCredits?: CodexResetCredits;
  /**
   * ZCode reset-card observation (Z.ai entry only). Refreshed passively by
   * the ordinary runtime cycle; retained by the backend only within its
   * freshness budget. Never present on other providers. No UI consumes this
   * yet — the shape is pinned for a future product decision.
   */
  zcodeResetCards?: ZCodeResetStatus;
  /** True for deterministic demo providers; they never enter history/prediction. */
  simulated?: boolean;
  /** Legacy status vocabulary, derived in Rust from {@link ProviderUsage.health}
   * ("ok" | "stale" | "error" | "unknown"). Kept for wire compatibility;
   * read `health` instead. */
  status: "ok" | "stale" | "error" | "unknown";
  /** Normalized provider health — the single field UI states read. */
  health: ProviderHealth;
  checkedAt: string;
  limits: LimitWindow[];
  /** Account attribution when the provider can prove it; all windows on
   * this usage belong to that one account. Masked and non-secret (see
   * {@link AccountAttribution}); retained unchanged through error/unknown
   * states so the card keeps naming its account. */
  account?: AccountAttribution;
  /**
   * Plan type or subscription tier (e.g. "team", "plus", "pro") explicitly
   * returned by the provider backend. Never guessed or inferred from model names.
   */
  planType?: string;
  /** Present when the last refresh attempt failed; limits may then be stale. */
  error?: string;
  /** Stable failure category (the structured error's `code`) when the entry
   * carries a failure; display-safe — never raw provider response text. */
  errorCategory?: string;
  /** HTTP status of the failed refresh, when the failure came from a
   * non-success response. */
  errorHttpStatus?: number;
  /** Exact timestamp of the cached source snapshot (RFC-3339), when the
   * provider reads one. Absent when the snapshot time is unknown. */
  sourceUpdatedAt?: string;
  /** Freshness of that snapshot; only meaningful together with limits.
   * Independent of health: absent means the provider has no source-snapshot
   * concept (freshness unknown, not stale). */
  dataFreshness?: DataFreshness;
};

/**
 * Structured error payload of the Rust provider commands: a stable `code`,
 * a display-safe `message`, plus retry metadata where the failure class
 * supports it — `httpStatus` (the offending HTTP status of a non-success
 * response) and `transient` (the backend's retry verdict). The shape and
 * the retry-verdict rule live once in `ProviderError`
 * (src-tauri/src/provider_error.rs), shared by the live provider backends.
 */
export type CommandError = {
  code?: string;
  message?: string;
  httpStatus?: number;
  transient?: boolean;
};

/**
 * Full state of the shared Rust provider runtime (src-tauri/src/runtime.rs),
 * pulled once on attach (`get_runtime_snapshot`) and re-delivered after every
 * completed cycle (`runtime://snapshot`). `seq` is a monotonic snapshot
 * counter: consumers apply a snapshot only when its `seq` is not older than
 * the newest one they applied, which makes the pull-after-subscribe race
 * harmless without a diff protocol.
 */
export type RuntimeSnapshot = {
  seq: number;
  providers: ProviderUsage[];
  /** RFC-3339 completion time of the last cycle with usable data. */
  lastUpdatedAt?: string;
  /** Whether the last completed cycle produced at least one usable source. */
  cycleSucceeded: boolean;
  cycleInFlight: boolean;
  refreshIntervalMinutes: number;
  /**
   * Monotonic revision of the Rust-owned quota history, bumped whenever the
   * runtime actually changed it (a new observation, a prune, an import, a
   * clear). Consumers re-pull `get_history` only when this differs from the
   * revision they last pulled, so the history file is never transferred on
   * unchanged cycles.
   */
  historyRevision: number;
};
