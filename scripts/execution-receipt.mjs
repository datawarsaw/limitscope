// LimitScope v0.7 deterministic execution receipt exporter.
//
// Reads a completed provenance run (the JSON written by
// `node scripts/provenance.mjs finish --out run.json`) and emits a byte-stable
// receipt in JSON and/or Markdown. No network, no daemon, no UI.
//
// Usage:
//   node scripts/execution-receipt.mjs --input run.json [--format json|markdown|both]
//        [--out-dir .] [--stdout]
//
// `--stdout` prints exactly one receipt to stdout (single format only) so the
// script can be piped; otherwise it writes
// `limitscope-run-<YYYY-MM-DDTHHMMZ>.<json|md>` under `--out-dir`.
//
// This module mirrors src/lib/executionReceipt.ts; a parity test pins the two
// implementations to the same bytes.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export const EXECUTION_RECEIPT_SCHEMA_VERSION = 1;
export const EXECUTION_RECEIPT_KIND = "limitscope-execution-receipt";
export const EXECUTION_RECEIPT_DISCLAIMER = "Observed quota delta during execution. Not exact task cost.";
export const EXECUTION_RECEIPT_FILE_PREFIX = "limitscope-run";

const CONFIDENCE_VALUES = ["EXACT", "BOUNDED", "INFERRED", "UNAVAILABLE"];

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

function looksSecretShaped(value) {
  const lower = value.toLowerCase();
  if (lower.includes("@")) return true;
  return SECRET_MARKERS.some((marker) => lower.includes(marker));
}

function sanitizeText(value, maxLength) {
  if (typeof value !== "string") return undefined;
  const collapsed = value.replace(CONTROL_CHARACTERS, " ").replace(/\s+/g, " ").trim();
  if (collapsed === "" || collapsed.length > maxLength) return undefined;
  if (looksSecretShaped(collapsed)) return undefined;
  return collapsed;
}

function sanitizeToken(value, maxLength) {
  const text = sanitizeText(value, maxLength);
  if (text === undefined || !TOKEN_SHAPE.test(text)) return undefined;
  return text;
}

// Mirrors sanitizeAccountIdentity / sanitizePlanType in src/lib/provenance.ts so
// the two receipt implementations agree on already-safe attribution.
function sanitizeAccountIdentity(value) {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  if (trimmed === "" || trimmed.length > 64 || /\s/.test(trimmed)) return undefined;
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

function sanitizePlanType(value) {
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

function toIsoTimestamp(value) {
  if (typeof value !== "string" || value.trim() === "") return undefined;
  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) return undefined;
  return new Date(ms).toISOString();
}

function toPercent(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) return undefined;
  return Math.min(100, Math.max(0, value));
}

function toConfidence(value) {
  if (typeof value === "string") {
    const upper = value.trim().toUpperCase();
    if (CONFIDENCE_VALUES.includes(upper)) return upper;
  }
  return "UNAVAILABLE";
}

function normalizeReceiptWindow(raw, resetCrossed) {
  if (typeof raw !== "object" || raw === null) return undefined;
  const label = sanitizeText(raw.label, 64);
  const beforeUsedPercent = toPercent(raw.beforeUsedPercent);
  const afterUsedPercent = toPercent(raw.afterUsedPercent);
  if (label === undefined || beforeUsedPercent === undefined || afterUsedPercent === undefined) return undefined;

  const comparable = raw.comparable === true && !resetCrossed;
  const status = comparable ? "measured" : resetCrossed ? "reset-crossed" : "incomparable";
  const rawDelta = typeof raw.deltaPoints === "number" && Number.isFinite(raw.deltaPoints) ? raw.deltaPoints : undefined;
  const deltaPoints = comparable && rawDelta !== undefined ? rawDelta : null;
  const reason = comparable ? undefined : sanitizeText(raw.reason, 200);

  const window = { label, beforeUsedPercent, afterUsedPercent, deltaPoints, comparable, status };
  if (reason !== undefined) window.reason = reason;
  return window;
}

export function createExecutionReceipt(raw) {
  if (typeof raw !== "object" || raw === null) return undefined;
  const startedAt = toIsoTimestamp(raw.startedAt);
  if (startedAt === undefined) return undefined;

  const endedAt = toIsoTimestamp(raw.endedAt);
  const startedMs = Date.parse(startedAt);
  const endedMs = endedAt !== undefined ? Date.parse(endedAt) : NaN;
  const durationMs = Number.isFinite(endedMs) && endedMs >= startedMs ? endedMs - startedMs : undefined;

  const confidence = toConfidence(raw.confidence ?? raw.attributionConfidence);
  const resetCrossed = raw.resetCrossed === true;
  const comparable = raw.comparable === true && confidence !== "UNAVAILABLE" && !resetCrossed;

  const windowsRaw = Array.isArray(raw.windows) ? raw.windows : Array.isArray(raw.deltas) ? raw.deltas : [];
  const byLabel = new Map();
  for (const entry of windowsRaw) {
    const window = normalizeReceiptWindow(entry, resetCrossed);
    if (window !== undefined) byLabel.set(window.label, window);
  }
  const windows = [...byLabel.values()].sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));

  const receipt = { schemaVersion: EXECUTION_RECEIPT_SCHEMA_VERSION, kind: EXECUTION_RECEIPT_KIND };
  const runId = sanitizeToken(raw.runId, 80);
  if (runId !== undefined) receipt.runId = runId;
  receipt.startedAt = startedAt;
  if (endedAt !== undefined) receipt.endedAt = endedAt;
  if (durationMs !== undefined) receipt.durationMs = durationMs;
  const harness = sanitizeText(raw.harness, 32);
  if (harness !== undefined) receipt.harness = harness;
  const model = sanitizeText(raw.model, 64);
  if (model !== undefined) receipt.model = model;
  const reasoningEffort = sanitizeText(raw.reasoningEffort, 32);
  if (reasoningEffort !== undefined) receipt.reasoningEffort = reasoningEffort;
  const subscription = sanitizePlanType(raw.subscription ?? raw.planType);
  if (subscription !== undefined) receipt.subscription = subscription;
  const account = sanitizeAccountIdentity(raw.account ?? raw.accountIdentity);
  if (account !== undefined) receipt.account = account;
  const providerId = sanitizeToken(raw.providerId ?? raw.provider, 64);
  if (providerId !== undefined) receipt.providerId = providerId;
  receipt.confidence = confidence;
  receipt.resetCrossed = resetCrossed;
  receipt.comparable = comparable;
  const incomparabilityReason = comparable ? undefined : sanitizeText(raw.incomparabilityReason, 240);
  if (incomparabilityReason !== undefined) receipt.incomparabilityReason = incomparabilityReason;
  receipt.windows = windows;
  receipt.disclaimer = EXECUTION_RECEIPT_DISCLAIMER;
  return receipt;
}

function serializeReceipt(receipt) {
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

export function formatReceiptJson(receipt) {
  return `${JSON.stringify(serializeReceipt(receipt), null, 2)}\n`;
}

function escapeMarkdown(value) {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function formatPercent(value) {
  return `${Math.round(value)}%`;
}

function formatDeltaPoints(value) {
  if (value === null || !Number.isFinite(value)) return "Delta unavailable";
  const rounded = Math.round(value);
  return `${rounded > 0 ? "+" : ""}${rounded} pp`;
}

export function formatReceiptDuration(durationMs) {
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

function formatWindowBlock(window) {
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

function formatClosingBlock(receipt) {
  const lines = [];
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

export function formatReceiptMarkdown(receipt) {
  const header = [];
  if (receipt.harness !== undefined) header.push(`Harness: ${escapeMarkdown(receipt.harness)}`);
  if (receipt.model !== undefined) header.push(`Model: ${escapeMarkdown(receipt.model)}`);
  if (receipt.reasoningEffort !== undefined) header.push(`Reasoning: ${escapeMarkdown(receipt.reasoningEffort)}`);
  if (receipt.subscription !== undefined) header.push(`Plan: ${escapeMarkdown(receipt.subscription)}`);
  if (receipt.account !== undefined) header.push(`Account: ${escapeMarkdown(receipt.account)}`);
  if (receipt.durationMs !== undefined) header.push(`Duration: ${formatReceiptDuration(receipt.durationMs)}`);
  header.push(`Confidence: ${receipt.confidence}`);

  const blocks = [header.join("\n")];
  if (receipt.windows.length === 0) {
    blocks.push("Quota observation unavailable");
  } else {
    for (const window of receipt.windows) blocks.push(formatWindowBlock(window));
  }
  blocks.push(formatClosingBlock(receipt));

  return `LimitScope Execution Receipt\n\n${blocks.join("\n\n")}\n`;
}

function compactStamp(iso) {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return "unknown";
  const normalized = new Date(ms).toISOString();
  return `${normalized.slice(0, 10)}T${normalized.slice(11, 13)}${normalized.slice(14, 16)}Z`;
}

export function receiptFileName(receipt, format = "markdown") {
  const extension = format === "json" ? "json" : "md";
  return `${EXECUTION_RECEIPT_FILE_PREFIX}-${compactStamp(receipt.startedAt)}.${extension}`;
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (!token.startsWith("--")) continue;
    const key = token.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith("--")) {
      args[key] = true;
    } else {
      args[key] = next;
      i += 1;
    }
  }
  return args;
}

function fail(message) {
  process.stderr.write(`execution-receipt: ${message}\n`);
  process.exit(1);
}

function main(argv) {
  const args = parseArgs(argv);
  if (!args.input) fail("missing --input <run.json>");
  let raw;
  try {
    raw = JSON.parse(readFileSync(args.input, "utf8"));
  } catch (error) {
    fail(`cannot read run JSON ${args.input}: ${error.message}`);
  }

  const receipt = createExecutionReceipt(raw);
  if (receipt === undefined) fail("run is not exportable (need a parseable startedAt timestamp)");

  const requested = args.format === undefined ? "both" : String(args.format);
  if (!["json", "markdown", "both"].includes(requested)) fail(`unknown --format ${requested}`);
  const formats = requested === "both" ? ["json", "markdown"] : [requested];

  if (args.stdout) {
    if (formats.length !== 1) fail("--stdout requires a single --format (json or markdown)");
    const format = formats[0];
    process.stdout.write(format === "json" ? formatReceiptJson(receipt) : formatReceiptMarkdown(receipt));
    return;
  }

  const outDir = args["out-dir"] === undefined ? "." : String(args["out-dir"]);
  mkdirSync(outDir, { recursive: true });
  for (const format of formats) {
    const content = format === "json" ? formatReceiptJson(receipt) : formatReceiptMarkdown(receipt);
    const target = `${outDir.replace(/[\\/]+$/, "")}/${receiptFileName(receipt, format)}`;
    writeFileSync(target, content, "utf8");
    process.stdout.write(`execution-receipt: ${target}\n`);
  }
}

if (process.argv[1] !== undefined && pathToFileURL(process.argv[1]).href === import.meta.url) {
  main(process.argv.slice(2));
}
