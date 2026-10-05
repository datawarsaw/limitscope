/**
 * Deterministic execution receipt export (LimitScope v0.7).
 *
 * Turns a completed `ExecutionProvenanceRun` into a byte-stable receipt in one
 * of two pure text formats:
 *
 * - `json`: versioned, machine-readable (HANDOFF metadata, developer logs,
 *   research comparisons, manual task accounting).
 * - `markdown`: compact, self-contained, embeddable into a HANDOFF without
 *   transformation.
 *
 * Design constraints:
 * - Deterministic: the same normalized run object produces byte-identical
 *   output. Window ordering, number formatting, timestamp format, and newline
 *   convention are pinned (see `docs/execution-receipts-v0.7.md`).
 * - Honest: the receipt reports an observation window, never a task cost, and
 *   never emits the misleading arithmetic delta across a quota reset.
 * - Private: only already-masked account attribution survives, optional
 *   metadata is omitted when absent (never `undefined` / `null` / `unknown`),
 *   and raw snapshots are not duplicated.
 * - No UI, no PDF, no HTML report generator.
 */

import { sanitizeAccountIdentity, sanitizePlanType, type AttributionConfidence } from "./provenance";

export const EXECUTION_RECEIPT_SCHEMA_VERSION = 1;
export const EXECUTION_RECEIPT_KIND = "limitscope-execution-receipt";
export const EXECUTION_RECEIPT_DISCLAIMER =
  "Observed quota delta during execution. Not exact task cost.";
export const EXECUTION_RECEIPT_FILE_PREFIX = "limitscope-run";

export type ExecutionReceiptFormat = "json" | "markdown";

/** `measured` is a comparable observation; the other two never carry a delta. */
export type ExecutionReceiptWindowStatus = "measured" | "reset-crossed" | "incomparable";

export type ExecutionReceiptWindow = {
  label: string;
  beforeUsedPercent: number;
  afterUsedPercent: number;
  /** `null` whenever the comparison is not comparable; never a reset-arithmetic delta. */
  deltaPoints: number | null;
  comparable: boolean;
  status: ExecutionReceiptWindowStatus;
  reason?: string;
};

export type ExecutionReceipt = {
  schemaVersion: number;
  kind: string;
  runId?: string;
  /** ISO-8601 UTC with milliseconds, e.g. `2026-09-30T09:00:00.000Z`. */
  startedAt: string;
  endedAt?: string;
  durationMs?: number;
  harness?: string;
  model?: string;
  reasoningEffort?: string;
  subscription?: string;
  account?: string;
  providerId?: string;
  confidence: AttributionConfidence;
  resetCrossed: boolean;
  comparable: boolean;
  incomparabilityReason?: string;
  windows: ExecutionReceiptWindow[];
  disclaimer: string;
};

export type ExecutionReceiptExport = {
  format: ExecutionReceiptFormat;
  fileName: string;
  content: string;
};

const CONFIDENCE_VALUES: readonly AttributionConfidence[] = ["EXACT", "BOUNDED", "INFERRED", "UNAVAILABLE"];

/** Credential/identity shapes that must never reach an exported receipt. */
const SECRET_MARKERS = [
  "eyj",
  "sk-",
  "ghp_",
  "gho_",
  "github_pat_",
  "-----begin",
  "bearer",
  "token",
  "secret",
  "password",
  "apikey",
  "api_key",
];

const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f]/g;
const TOKEN_SHAPE = /^[A-Za-z0-9._:-]+$/;

function looksSecretShaped(value: string): boolean {
  const lower = value.toLowerCase();
  if (lower.includes("@")) return true;
  return SECRET_MARKERS.some((marker) => lower.includes(marker));
}

/**
 * Collapses control characters and whitespace runs to single spaces so no
 * value can inject a line or block into the exported text.
 */
function sanitizeText(value: unknown, maxLength: number): string | undefined {
  if (typeof value !== "string") return undefined;
  const collapsed = value.replace(CONTROL_CHARACTERS, " ").replace(/\s+/g, " ").trim();
  if (collapsed === "" || collapsed.length > maxLength) return undefined;
  if (looksSecretShaped(collapsed)) return undefined;
  return collapsed;
}

function sanitizeToken(value: unknown, maxLength: number): string | undefined {
  const text = sanitizeText(value, maxLength);
  if (text === undefined || !TOKEN_SHAPE.test(text)) return undefined;
  return text;
}

function toIsoTimestamp(value: unknown): string | undefined {
  if (typeof value !== "string" || value.trim() === "") return undefined;
  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) return undefined;
  return new Date(ms).toISOString();
}

function toPercent(value: unknown): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value)) return undefined;
  return Math.min(100, Math.max(0, value));
}

function toConfidence(value: unknown): AttributionConfidence {
  if (typeof value === "string") {
    const upper = value.trim().toUpperCase();
    const match = CONFIDENCE_VALUES.find((candidate) => candidate === upper);
    if (match !== undefined) return match;
  }
  return "UNAVAILABLE";
}

function normalizeReceiptWindow(raw: unknown, resetCrossed: boolean): ExecutionReceiptWindow | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const record = raw as Record<string, unknown>;
  const label = sanitizeText(record.label, 64);
  const beforeUsedPercent = toPercent(record.beforeUsedPercent);
  const afterUsedPercent = toPercent(record.afterUsedPercent);
  if (label === undefined || beforeUsedPercent === undefined || afterUsedPercent === undefined) return undefined;

  const comparable = record.comparable === true && !resetCrossed;
  const status: ExecutionReceiptWindowStatus = comparable
    ? "measured"
    : resetCrossed
      ? "reset-crossed"
      : "incomparable";
  const rawDelta =
    typeof record.deltaPoints === "number" && Number.isFinite(record.deltaPoints) ? record.deltaPoints : undefined;
  const deltaPoints = comparable && rawDelta !== undefined ? rawDelta : null;
  const reason = comparable ? undefined : sanitizeText(record.reason, 200);

  return {
    label,
    beforeUsedPercent,
    afterUsedPercent,
    deltaPoints,
    comparable,
    status,
    ...(reason !== undefined ? { reason } : {}),
  };
}

/**
 * Normalizes a completed provenance run (already an object, or parsed JSON) into
 * a receipt model. Returns `undefined` when the run carries no usable start time.
 * Nothing is recomputed: the run's own comparable/confidence verdict is respected
 * and only tightened (never loosened) by the receipt rules.
 */
export function createExecutionReceipt(raw: unknown): ExecutionReceipt | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const record = raw as Record<string, unknown>;
  const startedAt = toIsoTimestamp(record.startedAt);
  if (startedAt === undefined) return undefined;

  const endedAt = toIsoTimestamp(record.endedAt);
  const startedMs = Date.parse(startedAt);
  const endedMs = endedAt !== undefined ? Date.parse(endedAt) : NaN;
  const durationMs = Number.isFinite(endedMs) && endedMs >= startedMs ? endedMs - startedMs : undefined;

  const confidence = toConfidence(record.confidence ?? record.attributionConfidence);
  const resetCrossed = record.resetCrossed === true;
  const comparable = record.comparable === true && confidence !== "UNAVAILABLE" && !resetCrossed;

  const windowsRaw = Array.isArray(record.windows)
    ? (record.windows as unknown[])
    : Array.isArray(record.deltas)
      ? (record.deltas as unknown[])
      : [];
  const byLabel = new Map<string, ExecutionReceiptWindow>();
  for (const entry of windowsRaw) {
    const window = normalizeReceiptWindow(entry, resetCrossed);
    if (window !== undefined) byLabel.set(window.label, window);
  }
  const windows = [...byLabel.values()].sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));

  const runId = sanitizeToken(record.runId, 80);
  const providerId = sanitizeToken(record.providerId ?? record.provider, 64);
  const harness = sanitizeText(record.harness, 32);
  const model = sanitizeText(record.model, 64);
  const reasoningEffort = sanitizeText(record.reasoningEffort, 32);
  const subscription = sanitizePlanType(record.subscription ?? record.planType);
  const account = sanitizeAccountIdentity(record.account ?? record.accountIdentity);
  const incomparabilityReason = comparable ? undefined : sanitizeText(record.incomparabilityReason, 240);

  return {
    schemaVersion: EXECUTION_RECEIPT_SCHEMA_VERSION,
    kind: EXECUTION_RECEIPT_KIND,
    ...(runId !== undefined ? { runId } : {}),
    startedAt,
    ...(endedAt !== undefined ? { endedAt } : {}),
    ...(durationMs !== undefined ? { durationMs } : {}),
    ...(harness !== undefined ? { harness } : {}),
    ...(model !== undefined ? { model } : {}),
    ...(reasoningEffort !== undefined ? { reasoningEffort } : {}),
    ...(subscription !== undefined ? { subscription } : {}),
    ...(account !== undefined ? { account } : {}),
    ...(providerId !== undefined ? { providerId } : {}),
    confidence,
    resetCrossed,
    comparable,
    ...(incomparabilityReason !== undefined ? { incomparabilityReason } : {}),
    windows,
    disclaimer: EXECUTION_RECEIPT_DISCLAIMER,
  };
}

function serializeReceipt(receipt: ExecutionReceipt): Record<string, unknown> {
  return {
    schemaVersion: receipt.schemaVersion,
    kind: receipt.kind,
    runId: receipt.runId,
    startedAt: receipt.startedAt,
    endedAt: receipt.endedAt,
    durationMs: receipt.durationMs,
    harness: receipt.harness,
    model: receipt.model,
    reasoningEffort: receipt.reasoningEffort,
    subscription: receipt.subscription,
    account: receipt.account,
    providerId: receipt.providerId,
    confidence: receipt.confidence,
    resetCrossed: receipt.resetCrossed,
    comparable: receipt.comparable,
    incomparabilityReason: receipt.incomparabilityReason,
    windows: receipt.windows.map((window) => ({
      label: window.label,
      beforeUsedPercent: window.beforeUsedPercent,
      afterUsedPercent: window.afterUsedPercent,
      deltaPoints: window.deltaPoints,
      comparable: window.comparable,
      status: window.status,
      reason: window.reason,
    })),
    disclaimer: receipt.disclaimer,
  };
}

/** Deterministic JSON receipt: 2-space indent, LF newlines, one trailing newline. */
export function formatReceiptJson(receipt: ExecutionReceipt): string {
  return `${JSON.stringify(serializeReceipt(receipt), null, 2)}\n`;
}

function escapeMarkdown(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function formatPercent(value: number): string {
  return `${Math.round(value)}%`;
}

function formatDeltaPoints(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return "Delta unavailable";
  const rounded = Math.round(value);
  return `${rounded > 0 ? "+" : ""}${rounded} pp`;
}

/** Compact duration: `<1m`, `18m`, `1h 5m`, `2h`, `3d 4h`, `3d`. */
export function formatReceiptDuration(durationMs: number): string {
  if (!Number.isFinite(durationMs) || durationMs < 0) return "unknown";
  const totalMinutes = Math.floor(durationMs / 60000);
  if (totalMinutes < 1) return "<1m";
  if (totalMinutes < 60) return `${totalMinutes}m`;
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours < 24) return minutes === 0 ? `${hours}h` : `${hours}h ${minutes}m`;
  const days = Math.floor(hours / 24);
  const restHours = hours % 24;
  return restHours === 0 ? `${days}d` : `${days}d ${restHours}h`;
}

function formatWindowBlock(window: ExecutionReceiptWindow): string {
  const lines = [escapeMarkdown(window.label)];
  if (window.status === "measured") {
    lines.push(`${formatPercent(window.beforeUsedPercent)} → ${formatPercent(window.afterUsedPercent)}`);
    lines.push(`Observed delta: ${formatDeltaPoints(window.deltaPoints)}`);
  } else if (window.status === "reset-crossed") {
    lines.push("Reset occurred during execution");
    lines.push("Delta unavailable");
  } else {
    lines.push("Comparison unavailable");
    lines.push("Delta unavailable");
  }
  return lines.join("\n");
}

function formatClosingBlock(receipt: ExecutionReceipt): string {
  const lines: string[] = [];
  if (receipt.comparable) {
    lines.push("Observed quota delta during execution.");
  } else if (receipt.resetCrossed) {
    lines.push("Quota reset occurred during execution.");
  } else {
    lines.push(escapeMarkdown(receipt.incomparabilityReason ?? "Quota observation unavailable."));
  }
  lines.push("Not exact task cost.");
  return lines.join("\n");
}

/** Deterministic Markdown receipt: fixed block order, LF newlines, one trailing newline. */
export function formatReceiptMarkdown(receipt: ExecutionReceipt): string {
  const header: string[] = [];
  if (receipt.harness !== undefined) header.push(`Harness: ${escapeMarkdown(receipt.harness)}`);
  if (receipt.model !== undefined) header.push(`Model: ${escapeMarkdown(receipt.model)}`);
  if (receipt.reasoningEffort !== undefined) header.push(`Reasoning: ${escapeMarkdown(receipt.reasoningEffort)}`);
  if (receipt.subscription !== undefined) header.push(`Plan: ${escapeMarkdown(receipt.subscription)}`);
  if (receipt.account !== undefined) header.push(`Account: ${escapeMarkdown(receipt.account)}`);
  if (receipt.durationMs !== undefined) header.push(`Duration: ${formatReceiptDuration(receipt.durationMs)}`);
  header.push(`Confidence: ${receipt.confidence}`);

  const blocks: string[] = [header.join("\n")];
  if (receipt.windows.length === 0) {
    blocks.push("Quota observation unavailable");
  } else {
    for (const window of receipt.windows) blocks.push(formatWindowBlock(window));
  }
  blocks.push(formatClosingBlock(receipt));

  return `LimitScope Execution Receipt\n\n${blocks.join("\n\n")}\n`;
}

/** `2026-09-30T09:00:00.000Z` -> `2026-09-30T0900Z`. Never carries an account identifier. */
function compactStamp(iso: string): string {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return "unknown";
  const normalized = new Date(ms).toISOString();
  return `${normalized.slice(0, 10)}T${normalized.slice(11, 13)}${normalized.slice(14, 16)}Z`;
}

/** Deterministic, account-free file name for a receipt. */
export function receiptFileName(receipt: ExecutionReceipt, format: ExecutionReceiptFormat = "markdown"): string {
  const extension = format === "json" ? "json" : "md";
  return `${EXECUTION_RECEIPT_FILE_PREFIX}-${compactStamp(receipt.startedAt)}.${extension}`;
}

/** Normalize and format a run in one step. `undefined` when the run is unusable. */
export function exportExecutionReceipt(raw: unknown, format: ExecutionReceiptFormat): ExecutionReceiptExport | undefined {
  const receipt = createExecutionReceipt(raw);
  if (receipt === undefined) return undefined;
  return {
    format,
    fileName: receiptFileName(receipt, format),
    content: format === "json" ? formatReceiptJson(receipt) : formatReceiptMarkdown(receipt),
  };
}
