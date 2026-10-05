/**
 * Execution provenance foundation (LimitScope v0.7 core contract).
 *
 * Provides honest, bounded before/after execution bracketing for AI coding runs.
 *
 * Core architectural guarantees:
 * - Canonical metric is always `usedPercent` (0-100). Remaining is an optional
 *   display perspective only.
 * - Confidence ladder: EXACT > BOUNDED > INFERRED > UNAVAILABLE.
 *   Automatic attribution ceiling is strictly BOUNDED for fresh manually-bracketed
 *   runs; EXACT is never automatically claimed because local monitors lack
 *   verifiable backend execution identifiers.
 * - Never claims "task cost". Reports observed quota deltas during execution.
 * - Reset boundary crossing (change in resetAt timestamp or passage of reset window)
 *   marks comparison incomparable; never claims negative delta (-85 pp) across reset.
 * - Account isolation: before/after snapshots from differing accounts are UNAVAILABLE.
 * - Security: No prompts, no responses, no credentials, no raw JWTs, no full account IDs.
 *   Only masked identities (e.g. key:3456) and explicit plan types (e.g. team, pro) pass.
 */

export type AttributionConfidence = "EXACT" | "BOUNDED" | "INFERRED" | "UNAVAILABLE";

export type ProvenanceWindowSnapshot = {
  label: string;
  usedPercent: number;
  resetAt?: string;
};

export type ExecutionQuotaSnapshot = {
  capturedAt: string;
  providerId: string;
  /** Stable masked identity only (e.g. `key:3456`). Never a secret or full identifier. */
  accountIdentity?: string;
  /** Explicit subscription tier (e.g. "team", "plus", "pro") when reported by provider. */
  planType?: string;
  status: "ok" | "stale" | "error" | "unknown";
  windows: ProvenanceWindowSnapshot[];
};

export type ProvenanceWindowDelta = {
  label: string;
  beforeUsedPercent: number;
  afterUsedPercent: number;
  /** Observed quota delta in percentage points (after minus before). 0 when incomparable. */
  deltaPoints: number;
  comparable: boolean;
  reason?: string;
};

export type ExecutionProvenanceRun = {
  runId: string;
  startedAt: string;
  endedAt?: string;
  harness?: string;
  model?: string;
  reasoningEffort?: string;
  /** Backend plan type when explicitly supplied by provider or caller. Never guessed from model. */
  subscription?: string;
  /** Stable masked account identity only (e.g. `key:3456`). Never a secret. */
  account?: string;
  beforeSnapshot: ExecutionQuotaSnapshot;
  afterSnapshot?: ExecutionQuotaSnapshot;
  windows: ProvenanceWindowDelta[];
  confidence: AttributionConfidence;
  resetCrossed: boolean;
  comparable: boolean;
  incomparabilityReason?: string;
  providerId: string;

  // Compatibility aliases for consumers and research contracts:
  before: ExecutionQuotaSnapshot;
  after?: ExecutionQuotaSnapshot;
  deltas: ProvenanceWindowDelta[];
  attributionConfidence: AttributionConfidence;
};

export type ExecutionProvenance = ExecutionProvenanceRun;
export type WindowDelta = ProvenanceWindowDelta;

export const PROVENANCE_DEFAULT_MAX_SNAPSHOT_AGE_MS = 15 * 60 * 1000;
export const PROVENANCE_FUTURE_SKEW_TOLERANCE_MS = 5 * 60 * 1000;

export function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, value));
}

export function isValidTimestamp(value: unknown): value is string {
  if (typeof value !== "string" || value.trim() === "") return false;
  return Number.isFinite(Date.parse(value));
}

/**
 * Accepts only stable masked identity tokens. Drops empty values, strings
 * with whitespace, strings > 64 chars, and anything secret-shaped (JWT
 * segments, sk- keys, emails, tokens, long hex blobs).
 */
export function sanitizeAccountIdentity(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  if (trimmed === "" || trimmed.length > 64) return undefined;
  if (/\s/.test(trimmed)) return undefined;
  const lower = trimmed.toLowerCase();
  if (
    lower.includes("eyj") ||
    lower.includes("sk-") ||
    lower.includes("token") ||
    lower.includes("secret") ||
    lower.includes("bearer") ||
    lower.includes("@")
  ) {
    return undefined;
  }
  if (trimmed.length > 40 && !trimmed.includes(":")) return undefined;
  return trimmed;
}

/**
 * Sanitizes explicit plan types (e.g. "team", "plus", "pro").
 * Never guessed from model name; only accepted when explicitly supplied.
 */
export function sanitizePlanType(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim().toLowerCase();
  if (trimmed === "" || trimmed.length > 32 || /\s/.test(trimmed)) return undefined;
  if (
    trimmed.includes("eyj") ||
    trimmed.includes("sk-") ||
    trimmed.includes("token") ||
    trimmed.includes("secret") ||
    trimmed.includes("@")
  ) {
    return undefined;
  }
  return trimmed;
}

function normalizeWindow(raw: unknown): ProvenanceWindowSnapshot | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const record = raw as Record<string, unknown>;
  const label = typeof record.label === "string" ? record.label.trim() : "";
  const percent =
    typeof record.usedPercent === "number"
      ? record.usedPercent
      : typeof record.used_percent === "number"
        ? (record.used_percent as number)
        : NaN;
  if (label === "" || !Number.isFinite(percent)) return undefined;
  const resetRaw =
    typeof record.resetAt === "string"
      ? record.resetAt
      : typeof record.reset_at === "string"
        ? (record.reset_at as string)
        : undefined;
  const resetAt = resetRaw !== undefined && isValidTimestamp(resetRaw) ? new Date(resetRaw).toISOString() : undefined;
  return {
    label,
    usedPercent: clampPercent(percent),
    ...(resetAt !== undefined ? { resetAt } : {}),
  };
}

/**
 * Normalizes caller-supplied snapshot JSON. Accepts the canonical
 * `ExecutionQuotaSnapshot` shape and the app `ProviderUsage` shape
 * (`{ id, checkedAt, limits, status, account: { identity }, planType }`).
 * Secrets are never read. Returns undefined when invalid.
 */
export function normalizeSnapshot(raw: unknown): ExecutionQuotaSnapshot | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const record = raw as Record<string, unknown>;
  const providerId =
    typeof record.providerId === "string" && record.providerId.trim() !== ""
      ? (record.providerId as string).trim()
      : typeof record.id === "string" && (record.id as string).trim() !== ""
        ? (record.id as string).trim()
        : "";
  if (providerId === "") return undefined;
  const capturedRaw =
    typeof record.capturedAt === "string"
      ? (record.capturedAt as string)
      : typeof record.checkedAt === "string"
        ? (record.checkedAt as string)
        : "";
  if (!isValidTimestamp(capturedRaw)) return undefined;
  const statusRaw = record.status;
  const status =
    statusRaw === "ok" || statusRaw === "stale" || statusRaw === "error" || statusRaw === "unknown"
      ? statusRaw
      : "unknown";
  const windowsRaw = Array.isArray(record.windows)
    ? (record.windows as unknown[])
    : Array.isArray(record.limits)
      ? (record.limits as unknown[])
      : [];
  const seen = new Map<string, ProvenanceWindowSnapshot>();
  for (const entry of windowsRaw) {
    const window = normalizeWindow(entry);
    if (window) seen.set(window.label, window);
  }
  const windows = [...seen.values()].sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
  let accountIdentity: string | undefined;
  if (typeof record.accountIdentity === "string") {
    accountIdentity = sanitizeAccountIdentity(record.accountIdentity);
  } else if (typeof record.account === "object" && record.account !== null) {
    const identity = (record.account as Record<string, unknown>).identity;
    accountIdentity = sanitizeAccountIdentity(identity);
  }
  const rawPlan = record.planType ?? record.plan_type ?? record.subscription;
  const planType = sanitizePlanType(rawPlan);
  return {
    capturedAt: new Date(capturedRaw).toISOString(),
    providerId,
    ...(accountIdentity !== undefined ? { accountIdentity } : {}),
    ...(planType !== undefined ? { planType } : {}),
    status,
    windows,
  };
}

export type SnapshotStaleness = { stale: boolean; reason?: string };

/**
 * A snapshot is usable for attribution only when it is fresh, successful,
 * and carries quota windows.
 */
export function snapshotStaleness(
  snapshot: ExecutionQuotaSnapshot,
  nowMs: number,
  maxAgeMs: number = PROVENANCE_DEFAULT_MAX_SNAPSHOT_AGE_MS,
): SnapshotStaleness {
  if (snapshot.windows.length === 0) return { stale: true, reason: "snapshot carries no quota windows" };
  if (snapshot.status !== "ok") return { stale: true, reason: `snapshot status is ${snapshot.status}` };
  const capturedMs = Date.parse(snapshot.capturedAt);
  if (!Number.isFinite(capturedMs)) return { stale: true, reason: "snapshot time is unknown" };
  if (capturedMs - nowMs > PROVENANCE_FUTURE_SKEW_TOLERANCE_MS) return { stale: true, reason: "snapshot is dated in the future" };
  if (nowMs - capturedMs > maxAgeMs) return { stale: true, reason: "snapshot is older than the freshness threshold" };
  return { stale: false };
}

export type BuildProvenanceInput = {
  before: ExecutionQuotaSnapshot;
  after: ExecutionQuotaSnapshot;
  harness?: string;
  runId?: string;
  model?: string;
  reasoningEffort?: string;
  subscription?: string;
  startedAt?: string;
  endedAt?: string;
  nowMs?: number;
  maxAgeMs?: number;
};

export type StartProvenanceRunInput = {
  before: ExecutionQuotaSnapshot;
  runId?: string;
  harness?: string;
  model?: string;
  reasoningEffort?: string;
  subscription?: string;
  startedAt?: string;
};

export type FinishProvenanceRunOptions = {
  endedAt?: string;
  nowMs?: number;
  maxAgeMs?: number;
  model?: string;
  reasoningEffort?: string;
  subscription?: string;
  harness?: string;
};

function pickTimestamp(explicit: string | undefined, fallback: string): string {
  return explicit !== undefined && isValidTimestamp(explicit) ? new Date(explicit).toISOString() : fallback;
}

export function generateRunId(): string {
  const ts = new Date().toISOString().replace(/[-:.]/g, "").replace("T", "-").slice(0, 17);
  const rand = Math.floor(Math.random() * 10000).toString().padStart(4, "0");
  return `run-${ts}-${rand}`;
}

/**
 * Initializes a new manual execution bracket.
 * Records the before snapshot, start timestamp, and optional context.
 */
export function startProvenanceRun(input: StartProvenanceRunInput): ExecutionProvenanceRun {
  const startedAt = pickTimestamp(input.startedAt, input.before.capturedAt);
  const runId = typeof input.runId === "string" && input.runId.trim() !== "" ? input.runId.trim() : generateRunId();
  const harness = input.harness !== undefined && input.harness.trim() !== "" ? input.harness.trim() : "Codex";
  const model = typeof input.model === "string" && input.model.trim() !== "" ? input.model.trim() : undefined;
  const reasoningEffort =
    typeof input.reasoningEffort === "string" && input.reasoningEffort.trim() !== ""
      ? input.reasoningEffort.trim()
      : undefined;
  const subscription = sanitizePlanType(input.subscription ?? input.before.planType);
  const account = input.before.accountIdentity;
  const initialWindows: ProvenanceWindowDelta[] = input.before.windows.map((w) => ({
    label: w.label,
    beforeUsedPercent: w.usedPercent,
    afterUsedPercent: w.usedPercent,
    deltaPoints: 0,
    comparable: false,
    reason: "execution in flight",
  }));

  return {
    runId,
    startedAt,
    harness,
    ...(model !== undefined ? { model } : {}),
    ...(reasoningEffort !== undefined ? { reasoningEffort } : {}),
    ...(subscription !== undefined ? { subscription } : {}),
    ...(account !== undefined ? { account } : {}),
    beforeSnapshot: input.before,
    windows: initialWindows,
    confidence: "BOUNDED",
    resetCrossed: false,
    comparable: false,
    incomparabilityReason: "Execution is currently in flight.",
    providerId: input.before.providerId,
    before: input.before,
    deltas: initialWindows,
    attributionConfidence: "BOUNDED",
  };
}

/**
 * Completes an active execution bracket with the after snapshot.
 */
export function finishProvenanceRun(
  run: ExecutionProvenanceRun,
  afterSnapshot: ExecutionQuotaSnapshot,
  options?: FinishProvenanceRunOptions,
): ExecutionProvenanceRun {
  return buildProvenance({
    before: run.beforeSnapshot,
    after: afterSnapshot,
    runId: run.runId,
    harness: options?.harness ?? run.harness,
    model: options?.model ?? run.model,
    reasoningEffort: options?.reasoningEffort ?? run.reasoningEffort,
    subscription: options?.subscription ?? run.subscription ?? afterSnapshot.planType,
    startedAt: run.startedAt,
    endedAt: options?.endedAt,
    nowMs: options?.nowMs,
    maxAgeMs: options?.maxAgeMs,
  });
}

/**
 * Brackets one known execution with before/after snapshots and derives the
 * honest attribution verdict. Never auto-assigns EXACT: BOUNDED is the ceiling.
 * Never emits task-cost language; deltas are observed deltas only.
 */
export function buildProvenance(input: BuildProvenanceInput): ExecutionProvenanceRun {
  const nowMs = input.nowMs ?? Date.now();
  const maxAgeMs = input.maxAgeMs ?? PROVENANCE_DEFAULT_MAX_SNAPSHOT_AGE_MS;
  const before = input.before;
  const after = input.after;
  const startedAt = pickTimestamp(input.startedAt, before.capturedAt);
  const endedAt = pickTimestamp(input.endedAt, after.capturedAt);
  const endedMs = Date.parse(endedAt);
  const startedMs = Date.parse(startedAt);
  const providerId = after.providerId !== "" ? after.providerId : before.providerId;
  const providerMismatch = before.providerId !== after.providerId;
  const providerAvailable =
    before.windows.length > 0 &&
    after.windows.length > 0 &&
    before.status !== "error" &&
    before.status !== "unknown" &&
    after.status !== "error" &&
    after.status !== "unknown";

  const accountChanged =
    (before.accountIdentity !== undefined || after.accountIdentity !== undefined)
      ? before.accountIdentity !== after.accountIdentity
      : false;

  const beforeByLabel = new Map(before.windows.map((w) => [w.label, w]));
  const afterByLabel = new Map(after.windows.map((w) => [w.label, w]));
  const labels = [...new Set([...beforeByLabel.keys(), ...afterByLabel.keys()])].sort();

  let resetCrossed = false;
  const deltas: ProvenanceWindowDelta[] = labels.map((label) => {
    const b = beforeByLabel.get(label);
    const a = afterByLabel.get(label);
    if (!b || !a) {
      return {
        label,
        beforeUsedPercent: b ? b.usedPercent : a ? a.usedPercent : 0,
        afterUsedPercent: a ? a.usedPercent : b ? b.usedPercent : 0,
        deltaPoints: 0,
        comparable: false,
        reason: !b ? "window appeared after the run started" : "window disappeared before the run ended",
      };
    }
    const bReset = b.resetAt !== undefined ? Date.parse(b.resetAt) : NaN;
    const aReset = a.resetAt !== undefined ? Date.parse(a.resetAt) : NaN;
    const resetChanged = Number.isFinite(bReset) && Number.isFinite(aReset) && bReset !== aReset;
    const resetPassed =
      Number.isFinite(bReset) &&
      Number.isFinite(endedMs) &&
      bReset <= endedMs &&
      (!Number.isFinite(startedMs) || bReset >= startedMs);
    if (resetChanged || resetPassed) resetCrossed = true;
    return {
      label,
      beforeUsedPercent: b.usedPercent,
      afterUsedPercent: a.usedPercent,
      deltaPoints: a.usedPercent - b.usedPercent,
      comparable: true,
      ...(resetChanged || resetPassed ? { reason: "reset boundary crossed" } : {}),
    };
  });

  const windowsStable = deltas.every((d) => beforeByLabel.has(d.label) && afterByLabel.has(d.label));
  let comparable =
    providerAvailable &&
    !providerMismatch &&
    !accountChanged &&
    !resetCrossed &&
    labels.length > 0 &&
    windowsStable;

  let incomparabilityReason: string | undefined;
  if (!providerAvailable) {
    incomparabilityReason = "Quota unavailable: one or both snapshots carry no usable windows.";
  } else if (providerMismatch) {
    incomparabilityReason = "Provider changed during execution. Direct quota delta is not comparable.";
  } else if (accountChanged) {
    incomparabilityReason = "Account changed during execution. Direct quota delta is not comparable.";
  } else if (resetCrossed) {
    incomparabilityReason = "Quota reset occurred during execution. Direct quota delta is not comparable.";
  } else if (labels.length === 0) {
    incomparabilityReason = "Quota unavailable: one or both snapshots carry no usable windows.";
  } else if (!windowsStable) {
    incomparabilityReason = "Quota windows changed during execution. Direct quota delta is not comparable.";
  }

  if (!comparable) {
    for (const d of deltas) {
      d.comparable = false;
      if (d.reason === undefined) d.reason = incomparabilityReason;
      d.deltaPoints = 0;
    }
  }

  let attributionConfidence: AttributionConfidence;
  if (!providerAvailable || providerMismatch || accountChanged || labels.length === 0) {
    attributionConfidence = "UNAVAILABLE";
  } else {
    // Before freshness is evaluated relative to startedAt; long runs do not retroactively stale baseline.
    const beforeStale = snapshotStaleness(before, Date.parse(startedAt), maxAgeMs);
    const afterStale = snapshotStaleness(after, nowMs, maxAgeMs);
    if (beforeStale.stale || afterStale.stale || !input.runId) {
      attributionConfidence = "INFERRED";
    } else {
      attributionConfidence = "BOUNDED";
    }
  }

  const harness = input.harness !== undefined && input.harness.trim() !== "" ? input.harness.trim() : "Codex";
  const model = typeof input.model === "string" && input.model.trim() !== "" ? input.model.trim() : undefined;
  const reasoningEffort =
    typeof input.reasoningEffort === "string" && input.reasoningEffort.trim() !== ""
      ? input.reasoningEffort.trim()
      : undefined;
  const explicitSub = sanitizePlanType(input.subscription ?? after.planType ?? before.planType);
  const runId = typeof input.runId === "string" && input.runId.trim() !== "" ? input.runId.trim() : generateRunId();
  const account = after.accountIdentity ?? before.accountIdentity;

  return {
    runId,
    startedAt,
    endedAt,
    harness,
    ...(model !== undefined ? { model } : {}),
    ...(reasoningEffort !== undefined ? { reasoningEffort } : {}),
    ...(explicitSub !== undefined ? { subscription: explicitSub } : {}),
    ...(account !== undefined ? { account } : {}),
    beforeSnapshot: before,
    afterSnapshot: after,
    windows: deltas,
    confidence: attributionConfidence,
    resetCrossed,
    comparable,
    ...(incomparabilityReason !== undefined ? { incomparabilityReason } : {}),
    providerId,

    // Aliases:
    before,
    after,
    deltas,
    attributionConfidence,
  };
}

/** Compact deterministic countdown (`4h 12m`, `18m`, `<1m`). Null when passed/invalid. */
export function formatCountdownShort(targetIso: string, nowMs: number): string | null {
  const target = Date.parse(targetIso);
  if (!Number.isFinite(target)) return null;
  const remainingMs = target - nowMs;
  if (remainingMs <= 0) return null;
  const totalMinutes = Math.floor(remainingMs / 60000);
  if (totalMinutes < 1) return "<1m";
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

export type FooterOptions = {
  perspective?: "used" | "remaining";
  nowMs?: number;
};

function displayPercent(usedPercent: number, perspective: "used" | "remaining"): string {
  const value = perspective === "used" ? usedPercent : 100 - usedPercent;
  return `${Math.round(value)}% ${perspective}`;
}

/**
 * Pure, deterministic provenance footer formatter.
 * Stable alphabetical ordering, no secrets, omits unknown optional fields,
 * and includes explicit confidence and non-task-cost disclaimer.
 */
export function formatProvenanceFooter(prov: ExecutionProvenanceRun, options?: FooterOptions): string {
  const perspective = options?.perspective ?? "remaining";
  const nowMs = options?.nowMs ?? (prov.endedAt ? Date.parse(prov.endedAt) : Date.now());
  const lines: string[] = ["EXECUTION PROVENANCE", "", `Harness: ${prov.harness}`];
  if (prov.model) lines.push(`Model: ${prov.model}`);
  if (prov.reasoningEffort) lines.push(`Reasoning: ${prov.reasoningEffort}`);
  if (prov.subscription) lines.push(`Subscription: ${prov.subscription}`);

  const windowsList = prov.windows ?? prov.deltas ?? [];
  const afterSnap = prov.afterSnapshot ?? prov.after;
  const beforeSnap = prov.beforeSnapshot ?? prov.before;

  if (
    windowsList.length === 0 ||
    (!prov.comparable && prov.confidence === "UNAVAILABLE" && beforeSnap.windows.length === 0)
  ) {
    lines.push("Quota observation: unavailable");
  } else {
    lines.push("Quota observation:");
    const sorted = [...windowsList].sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
    for (const d of sorted) {
      const extra = d.comparable ? "" : " (not comparable)";
      lines.push(
        `- ${d.label}: ${displayPercent(d.beforeUsedPercent, perspective)} -> ${displayPercent(d.afterUsedPercent, perspective)}${extra}`,
      );
    }
  }

  if (!prov.comparable && prov.incomparabilityReason) {
    lines.push(prov.incomparabilityReason);
  } else if (prov.comparable) {
    lines.push("Observed quota delta during execution (not exact task cost).");
  }

  const windowsForReset = afterSnap ? afterSnap.windows : beforeSnap.windows;
  const upcoming = windowsForReset
    .map((w) => w.resetAt)
    .filter((r): r is string => r !== undefined && Number.isFinite(Date.parse(r)))
    .map((r) => Date.parse(r))
    .filter((t) => t > nowMs)
    .sort((a, b) => a - b);

  if (upcoming.length > 0) {
    const countdown = formatCountdownShort(new Date(upcoming[0]).toISOString(), nowMs);
    lines.push(countdown ? `Next reset: ${countdown}` : "Reset time passed");
  } else if (windowsForReset.some((w) => w.resetAt !== undefined)) {
    lines.push("Reset time passed");
  }

  lines.push(`Attribution: ${prov.confidence.toLowerCase()}`);
  return lines.join("\n");
}
