// LimitScope v0.7 Execution Provenance CLI helper.
// No daemon. No Codex modification. No prompt injection. No network.
//
// Usage:
//   node scripts/provenance.mjs start --snapshot before.json [--run-id ID --model NAME --reasoning EFFORT --subscription PLAN --harness Codex --run-file .limitscope-provenance-run.json]
//   <run Codex or AI agent externally>
//   node scripts/provenance.mjs finish --snapshot after.json [--run-file .limitscope-provenance-run.json --out provenance.json --footer provenance.txt --perspective remaining]
//
// Snapshot files accept the canonical ExecutionQuotaSnapshot shape or the app
// ProviderUsage shape ({ id, checkedAt, limits, status, account: { identity }, planType }).
// Snapshots are read-only inputs; this script never touches credentials.

import { readFileSync, writeFileSync } from "node:fs";

const DEFAULT_RUN_FILE = ".limitscope-provenance-run.json";
const DEFAULT_MAX_AGE_MS = 15 * 60 * 1000;

function fail(message) {
  console.error(`provenance: ${message}`);
  process.exit(1);
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

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    fail(`cannot read JSON file ${path}: ${error.message}`);
  }
  return undefined;
}

function clampPercent(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, value));
}

function validTimestamp(value) {
  return typeof value === "string" && value.trim() !== "" && Number.isFinite(Date.parse(value));
}

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

function normalizeWindow(raw) {
  if (typeof raw !== "object" || raw === null) return undefined;
  const label = typeof raw.label === "string" ? raw.label.trim() : "";
  const percent =
    typeof raw.usedPercent === "number"
      ? raw.usedPercent
      : typeof raw.used_percent === "number"
        ? raw.used_percent
        : NaN;
  if (label === "" || !Number.isFinite(percent)) return undefined;
  const resetRaw =
    typeof raw.resetAt === "string" ? raw.resetAt : typeof raw.reset_at === "string" ? raw.reset_at : undefined;
  const out = { label, usedPercent: clampPercent(percent) };
  if (validTimestamp(resetRaw)) out.resetAt = new Date(resetRaw).toISOString();
  return out;
}

function normalizeSnapshot(raw) {
  if (typeof raw !== "object" || raw === null) return undefined;
  const providerId =
    typeof raw.providerId === "string" && raw.providerId.trim() !== ""
      ? raw.providerId.trim()
      : typeof raw.id === "string"
        ? raw.id.trim()
        : "";
  if (!providerId) return undefined;
  const capturedRaw =
    typeof raw.capturedAt === "string" ? raw.capturedAt : typeof raw.checkedAt === "string" ? raw.checkedAt : "";
  if (!validTimestamp(capturedRaw)) return undefined;
  const status = ["ok", "stale", "error", "unknown"].includes(raw.status) ? raw.status : "unknown";
  const list = Array.isArray(raw.windows) ? raw.windows : Array.isArray(raw.limits) ? raw.limits : [];
  const seen = new Map();
  for (const entry of list) {
    const w = normalizeWindow(entry);
    if (w) seen.set(w.label, w);
  }
  const windows = [...seen.values()].sort((a, b) => (a.label < b.label ? -1 : 1));
  let accountIdentity;
  if (typeof raw.accountIdentity === "string") accountIdentity = sanitizeAccountIdentity(raw.accountIdentity);
  else if (typeof raw.account === "object" && raw.account !== null)
    accountIdentity = sanitizeAccountIdentity(raw.account.identity);
  const rawPlan = raw.planType ?? raw.plan_type ?? raw.subscription;
  const planType = sanitizePlanType(rawPlan);
  const snap = { capturedAt: new Date(capturedRaw).toISOString(), providerId, status, windows };
  if (accountIdentity !== undefined) snap.accountIdentity = accountIdentity;
  if (planType !== undefined) snap.planType = planType;
  return snap;
}

function snapshotStale(snap, nowMs, maxAgeMs) {
  if (snap.windows.length === 0) return true;
  if (snap.status !== "ok") return true;
  const t = Date.parse(snap.capturedAt);
  if (!Number.isFinite(t)) return true;
  if (t - nowMs > 5 * 60 * 1000) return true;
  if (nowMs - t > maxAgeMs) return true;
  return false;
}

function buildProvenance({
  before,
  after,
  harness,
  runId,
  model,
  reasoningEffort,
  subscription,
  startedAt,
  endedAt,
  nowMs,
  maxAgeMs,
}) {
  const endMs = Date.parse(endedAt);
  const startMs = Date.parse(startedAt);
  const beforeBy = new Map(before.windows.map((w) => [w.label, w]));
  const afterBy = new Map(after.windows.map((w) => [w.label, w]));
  const labels = [...new Set([...beforeBy.keys(), ...afterBy.keys()])].sort();
  let resetCrossed = false;
  const deltas = labels.map((label) => {
    const b = beforeBy.get(label);
    const a = afterBy.get(label);
    if (!b || !a) {
      return {
        label,
        beforeUsedPercent: b ? b.usedPercent : a.usedPercent,
        afterUsedPercent: a ? a.usedPercent : b.usedPercent,
        deltaPoints: 0,
        comparable: false,
        reason: !b ? "window appeared after the run started" : "window disappeared before the run ended",
      };
    }
    const bReset = b.resetAt !== undefined ? Date.parse(b.resetAt) : NaN;
    const aReset = a.resetAt !== undefined ? Date.parse(a.resetAt) : NaN;
    const changed = Number.isFinite(bReset) && Number.isFinite(aReset) && bReset !== aReset;
    const passed =
      Number.isFinite(bReset) &&
      Number.isFinite(endMs) &&
      bReset <= endMs &&
      (!Number.isFinite(startMs) || bReset >= startMs);
    if (changed || passed) resetCrossed = true;
    return {
      label,
      beforeUsedPercent: b.usedPercent,
      afterUsedPercent: a.usedPercent,
      deltaPoints: a.usedPercent - b.usedPercent,
      comparable: true,
      ...(changed || passed ? { reason: "reset boundary crossed" } : {}),
    };
  });
  const providerMismatch = before.providerId !== after.providerId;
  const available =
    before.windows.length > 0 &&
    after.windows.length > 0 &&
    before.status !== "error" &&
    before.status !== "unknown" &&
    after.status !== "error" &&
    after.status !== "unknown";
  const accountChanged =
    before.accountIdentity !== undefined || after.accountIdentity !== undefined
      ? before.accountIdentity !== after.accountIdentity
      : false;
  const windowsStable = deltas.every((d) => beforeBy.has(d.label) && afterBy.has(d.label));
  let comparable = available && !providerMismatch && !accountChanged && !resetCrossed && labels.length > 0 && windowsStable;
  let reason;
  if (!available || labels.length === 0)
    reason = "Quota unavailable: one or both snapshots carry no usable windows.";
  else if (providerMismatch)
    reason = "Provider changed during execution. Direct quota delta is not comparable.";
  else if (accountChanged)
    reason = "Account changed during execution. Direct quota delta is not comparable.";
  else if (resetCrossed)
    reason = "Quota reset occurred during execution. Direct quota delta is not comparable.";
  else if (!windowsStable)
    reason = "Quota windows changed during execution. Direct quota delta is not comparable.";
  if (!comparable) {
    for (const d of deltas) {
      d.comparable = false;
      if (d.reason === undefined) d.reason = reason;
      d.deltaPoints = 0;
    }
  }
  let confidence;
  if (!available || providerMismatch || accountChanged || labels.length === 0) confidence = "UNAVAILABLE";
  else if (snapshotStale(before, Date.parse(startedAt), maxAgeMs) || snapshotStale(after, nowMs, maxAgeMs) || !runId)
    confidence = "INFERRED";
  else confidence = "BOUNDED";

  const explicitSub = sanitizePlanType(subscription ?? after.planType ?? before.planType);
  const account = after.accountIdentity ?? before.accountIdentity;

  const prov = {
    runId,
    startedAt,
    endedAt,
    harness,
    ...(model ? { model } : {}),
    ...(reasoningEffort ? { reasoningEffort } : {}),
    ...(explicitSub ? { subscription: explicitSub } : {}),
    ...(account ? { account } : {}),
    providerId: after.providerId || before.providerId,
    beforeSnapshot: before,
    afterSnapshot: after,
    windows: deltas,
    confidence,
    resetCrossed,
    comparable,
    ...(reason ? { incomparabilityReason: reason } : {}),
    // Aliases:
    before,
    after,
    deltas,
    attributionConfidence: confidence,
  };
  return prov;
}

function countdown(targetIso, nowMs) {
  const t = Date.parse(targetIso);
  if (!Number.isFinite(t)) return null;
  const ms = t - nowMs;
  if (ms <= 0) return null;
  const mins = Math.floor(ms / 60000);
  if (mins < 1) return "<1m";
  const days = Math.floor(mins / 1440);
  const hours = Math.floor((mins % 1440) / 60);
  const rest = mins % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${rest}m`;
  return `${rest}m`;
}

function footer(prov, perspective, nowMs) {
  const show = (used) => `${Math.round(perspective === "used" ? used : 100 - used)}% ${perspective}`;
  const lines = ["EXECUTION PROVENANCE", "", `Harness: ${prov.harness}`];
  if (prov.model) lines.push(`Model: ${prov.model}`);
  if (prov.reasoningEffort) lines.push(`Reasoning: ${prov.reasoningEffort}`);
  if (prov.subscription) lines.push(`Subscription: ${prov.subscription}`);
  const windowsList = prov.windows ?? prov.deltas ?? [];
  if (windowsList.length === 0) {
    lines.push("Quota observation: unavailable");
  } else {
    lines.push("Quota observation:");
    for (const d of [...windowsList].sort((a, b) => (a.label < b.label ? -1 : 1))) {
      lines.push(
        `- ${d.label}: ${show(d.beforeUsedPercent)} -> ${show(d.afterUsedPercent)}${d.comparable ? "" : " (not comparable)"}`,
      );
    }
  }
  if (!prov.comparable && prov.incomparabilityReason) lines.push(prov.incomparabilityReason);
  else if (prov.comparable) lines.push("Observed quota delta during execution (not exact task cost).");

  const afterSnap = prov.afterSnapshot ?? prov.after;
  const beforeSnap = prov.beforeSnapshot ?? prov.before;
  const snapForReset = afterSnap ?? beforeSnap;
  const upcoming = (snapForReset ? snapForReset.windows : [])
    .map((w) => w.resetAt)
    .filter((r) => validTimestamp(r))
    .map((r) => Date.parse(r))
    .filter((t) => t > nowMs)
    .sort((a, b) => a - b);
  if (upcoming.length > 0) {
    const cd = countdown(new Date(upcoming[0]).toISOString(), nowMs);
    lines.push(cd ? `Next reset: ${cd}` : "Reset time passed");
  } else if (snapForReset && snapForReset.windows.some((w) => w.resetAt !== undefined)) {
    lines.push("Reset time passed");
  }
  const conf = prov.confidence ?? prov.attributionConfidence ?? "unavailable";
  lines.push(`Attribution: ${conf.toLowerCase()}`);
  return lines.join("\n");
}

function newRunId() {
  return `run-${new Date().toISOString().replace(/[-:.]/g, "").replace("T", "-").slice(0, 17)}-${Math.floor(Math.random() * 10000).toString().padStart(4, "0")}`;
}

const [command, ...rest] = process.argv.slice(2);
const args = parseArgs(rest);

if (command === "start") {
  if (!args.snapshot) fail("missing --snapshot <before.json>");
  const snap = normalizeSnapshot(readJson(args.snapshot));
  if (!snap) fail("before snapshot is not usable (need providerId/id, capturedAt/checkedAt, and windows/limits)");
  const runFile = args["run-file"] || args.out || DEFAULT_RUN_FILE;
  const context = {
    runId: args["run-id"] || newRunId(),
    harness: args.harness || "Codex",
    startedAt: new Date().toISOString(),
    before: snap,
    beforeSnapshot: snap,
  };
  if (args.model) context.model = args.model;
  if (args.reasoning) context.reasoningEffort = args.reasoning;
  const sub = sanitizePlanType(args.subscription ?? snap.planType);
  if (sub) context.subscription = sub;
  if (snap.accountIdentity) context.account = snap.accountIdentity;
  writeFileSync(runFile, `${JSON.stringify(context, null, 2)}\n`, "utf8");
  console.log(`provenance run started: ${context.runId}`);
  console.log(`before snapshot: ${snap.providerId} (${snap.windows.length} window(s)) at ${snap.capturedAt}`);
  console.log(`run context: ${runFile}`);
} else if (command === "finish") {
  if (!args.snapshot) fail("missing --snapshot <after.json>");
  const runFile = args["run-file"] || DEFAULT_RUN_FILE;
  const context = readJson(runFile);
  if (!context || (!context.before && !context.beforeSnapshot))
    fail(`run context not found at ${runFile}; run 'start' first`);
  const before = normalizeSnapshot(context.beforeSnapshot || context.before);
  const after = normalizeSnapshot(readJson(args.snapshot));
  if (!before) fail("stored before snapshot is not usable");
  if (!after) fail("after snapshot is not usable (need providerId/id, capturedAt/checkedAt, and windows/limits)");
  const nowMs = Date.now();
  const maxAgeMs = args["max-age-ms"] !== undefined ? Number(args["max-age-ms"]) : DEFAULT_MAX_AGE_MS;
  const prov = buildProvenance({
    before,
    after,
    harness: args.harness || context.harness || "Codex",
    runId: args["run-id"] || context.runId,
    model: args.model || context.model,
    reasoningEffort: args.reasoning || context.reasoningEffort,
    subscription: args.subscription || context.subscription || after.planType,
    startedAt: context.startedAt,
    endedAt: new Date().toISOString(),
    nowMs,
    maxAgeMs,
  });
  const perspective = args.perspective === "used" ? "used" : "remaining";
  const text = footer(prov, perspective, nowMs);
  if (args.out) writeFileSync(args.out, `${JSON.stringify(prov, null, 2)}\n`, "utf8");
  if (args.footer) writeFileSync(args.footer, `${text}\n`, "utf8");
  if (!args.out && !args.footer) {
    console.log(JSON.stringify(prov, null, 2));
    console.log("");
    console.log(text);
  } else {
    if (args.out) console.log(`provenance: ${args.out}`);
    if (args.footer) console.log(`footer: ${args.footer}`);
    console.log("");
    console.log(text);
  }
} else {
  fail("usage: provenance.mjs start --snapshot before.json [...] | finish --snapshot after.json [...]");
}
