/**
 * Implementation of the LimitScope local update harness.
 *
 * `scripts/update-harness.mjs` is the CLI entry point; this module holds
 * everything testable and must stay shebang-free: vite's transform pipeline
 * hoists imports above a leading hashbang, producing an invalid token
 * mid-module, so vitest cannot import a shebang-bearing module.
 *
 * Secrets never reach output: the signing password is read from the
 * TAURI_SIGNING_PRIVATE_KEY_PASSWORD environment variable only (there is no
 * on-disk fallback), it is passed to the Tauri CLI through the child
 * environment instead of a command-line argument, and every error and
 * diagnostic line is redacted for the password, the private-key path and
 * the key directory. Diagnostics never print the private-key path; the
 * `keys` command prints the public-key location instead.
 */

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tauriCli = path.join(
  path.dirname(require.resolve("@tauri-apps/cli/package.json")),
  "tauri.js",
);

const DEFAULT_PORT = 8397;
const HARNESS_DIR = path.join(repoRoot, "state", "update-harness");
const PASSWORD_ENV_VAR = "TAURI_SIGNING_PRIVATE_KEY_PASSWORD";

/** A CLI failure whose message is safe to print once it passes the redactor. */
export class HarnessError extends Error {
  constructor(message) {
    super(message);
    this.name = "HarnessError";
  }
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Registry of secret strings that must never reach errors, logs or
 * diagnostics. Values are matched case-insensitively (Windows path casing)
 * longest-first, so replacing a key-directory prefix cannot leave the rest
 * of a private-key path exposed. Empty values are ignored: a missing secret
 * must not clobber output.
 */
export function createSecretRedactor() {
  const values = new Set();
  return {
    register(value) {
      if (typeof value === "string" && value.length > 0) values.add(value);
    },
    redact(text) {
      let output = String(text ?? "");
      const ordered = [...values].sort((a, b) => b.length - a.length);
      for (const value of ordered) {
        output = output.replace(new RegExp(escapeRegExp(value), "gi"), "[redacted]");
      }
      return output;
    },
  };
}

/** Registers a path plus its forward/backslash variants for redaction. */
function registerPath(redactor, value) {
  redactor.register(value);
  redactor.register(value.replaceAll("\\", "/"));
  redactor.register(value.replaceAll("/", "\\"));
}

/** Locates the updater key directory. Only file existence is probed; key contents are never read. */
export function resolveKeyConfig(home = os.homedir()) {
  const candidates = [
    {
      dir: path.join(home, ".tauri", "limitscope-updater"),
      file: path.join(home, ".tauri", "limitscope-updater", "limitscope.key"),
      pub: path.join(home, ".tauri", "limitscope-updater", "limitscope.key.pub"),
    },
    {
      dir: path.join(home, ".tauri", "rate-limits-updater"),
      file: path.join(home, ".tauri", "rate-limits-updater", "rate-limits.key"),
      pub: path.join(home, ".tauri", "rate-limits-updater", "rate-limits.key.pub"),
    },
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate.file)) return candidate;
  }
  return candidates[0];
}

/**
 * Resolves the signing password from the environment only. The key-directory
 * README was previously parsed as a plaintext-password fallback; on-disk
 * password storage is no longer consulted, so the environment variable is
 * the single supported source.
 */
export function resolveKeyPassword(env = process.env) {
  const value = env[PASSWORD_ENV_VAR];
  if (typeof value === "string" && value.length > 0) return value;
  throw new HarnessError(
    `no signing password found; set the ${PASSWORD_ENV_VAR} environment variable`,
  );
}

/**
 * Builds the Tauri signer invocation. The password travels only in the child
 * environment — the same mechanism the release workflow uses — never on a
 * command line, where process listings and crash diagnostics would expose it.
 */
export function buildSigningInvocation({ keyFile, password, target, baseEnv = process.env }) {
  return {
    args: ["signer", "sign", "-f", keyFile, target],
    env: { ...baseEnv, [PASSWORD_ENV_VAR]: password },
  };
}

/**
 * Formats a failed Tauri CLI run for diagnostics. The command arguments are
 * deliberately omitted — they carry the private-key path, and a failed sign
 * previously printed the whole argument vector verbatim — and all child
 * output passes through the redactor before it can reach a terminal or log.
 */
export function formatTauriFailure(result, redactor) {
  const reason = result?.error
    ? result.error.message
    : `exit code ${result?.status ?? "unknown"}`;
  const lines = [`tauri signer sign failed (${redactor.redact(reason)})`];
  const stdout = redactor.redact(result?.stdout ?? "").trimEnd();
  const stderr = redactor.redact(result?.stderr ?? "").trimEnd();
  if (stdout) lines.push(stdout);
  if (stderr) lines.push(stderr);
  return lines.join("\n");
}

/** Runs the Tauri signer for one target; throws a redacted HarnessError on failure. */
function runTauriSign({ keyCfg, password, target, redactor, spawn = spawnSync }) {
  const invocation = buildSigningInvocation({
    keyFile: keyCfg.file,
    password,
    target,
    baseEnv: process.env,
  });
  const result = spawn(process.execPath, [tauriCli, ...invocation.args], {
    stdio: ["ignore", "pipe", "pipe"],
    encoding: "utf8",
    cwd: repoRoot,
    env: invocation.env,
  });
  if (result.error || result.status !== 0) {
    throw new HarnessError(formatTauriFailure(result, redactor));
  }
  return result;
}

/** Finds a built NSIS installer for the given version in target bundles. */
function findInstaller(version) {
  const hits = [];
  for (const profile of ["debug", "release"]) {
    const dir = path.join(repoRoot, "src-tauri", "target", profile, "bundle", "nsis");
    if (!existsSync(dir)) continue;
    for (const entry of readdirSync(dir)) {
      if (entry.endsWith("_x64-setup.exe") && entry.includes(version)) {
        hits.push(path.join(dir, entry));
      }
    }
  }
  if (hits.length === 0) {
    throw new HarnessError(
      `no NSIS installer for version ${version} under src-tauri/target/*/bundle/nsis — ` +
        "build one first (see docs/updater-production.md)",
    );
  }
  return hits[0];
}

function stageInstaller(version, notes, { keyCfg, redactor, env, spawn, argv, emit }) {
  const installer = findInstaller(version);
  mkdirSync(HARNESS_DIR, { recursive: true });
  const staged = path.join(HARNESS_DIR, path.basename(installer));
  copyFileSync(installer, staged);
  const password = resolveKeyPassword(env);
  redactor.register(password);
  runTauriSign({ keyCfg, password, target: staged, redactor, spawn });
  const sigFile = `${staged}.sig`;
  if (!existsSync(sigFile)) throw new HarnessError(`signature was not written to ${sigFile}`);
  const signature = readFileSync(sigFile, "utf8").trim();
  const encoded = encodeURI(path.basename(installer));
  const feed = {
    version,
    pub_date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
    notes: notes ?? `Local harness build ${version}.`,
    platforms: {
      "windows-x86_64": {
        signature,
        url: `http://127.0.0.1:${flagValue(argv, "--port") ?? DEFAULT_PORT}/${encoded}`,
      },
    },
  };
  writeFileSync(path.join(HARNESS_DIR, "latest.json"), `${JSON.stringify(feed, null, 2)}\n`);
  emit(`staged: ${path.basename(installer)} (+ .sig)`);
  emit(`feed:   ${path.join(HARNESS_DIR, "latest.json")} -> version ${version}`);
}

function serve(port, emit) {
  if (!existsSync(path.join(HARNESS_DIR, "latest.json"))) {
    throw new HarnessError(`no feed staged yet — run: node scripts/update-harness.mjs prepare --version <V>`);
  }
  const server = createServer((request, response) => {
    const name = decodeURIComponent(new URL(request.url, "http://x").pathname.slice(1));
    const file = path.join(HARNESS_DIR, path.basename(name));
    if (!existsSync(file)) {
      emit(`MISS  ${request.url}`);
      response.writeHead(404).end("not found");
      return;
    }
    const body = readFileSync(file);
    response.writeHead(200, {
      "content-type": file.endsWith(".json")
        ? "application/json"
        : "application/octet-stream",
      "content-length": body.length,
    });
    response.end(body);
    emit(`SENT  ${request.url} (${body.length} bytes)`);
  });
  server.listen(port, "127.0.0.1", () => {
    emit(`update feed: http://127.0.0.1:${port}/latest.json`);
    emit("serve dir:", HARNESS_DIR);
    emit("press Ctrl+C to stop");
  });
}

function writeOverlay(version, port, argv, emit) {
  mkdirSync(HARNESS_DIR, { recursive: true });
  const overlay = {
    version,
    plugins: {
      updater: {
        endpoints: [`http://127.0.0.1:${port}/latest.json`],
      },
    },
  };
  // Optional side-by-side smoke: install the throwaway build next to the
  // real app instead of upgrading it. A distinct product name + identifier
  // means a distinct install dir, data dir and uninstall entry.
  const productName = flagValue(argv, "--product-name");
  const identifier = flagValue(argv, "--identifier");
  if (productName) overlay.productName = productName;
  if (identifier) overlay.identifier = identifier;

  const target = path.join(HARNESS_DIR, "tauri.overlay.json");
  writeFileSync(target, `${JSON.stringify(overlay, null, 2)}\n`);
  emit(`wrote:  ${target}`);
  emit("build with:");
  emit(
    `  npm run tauri build -- --debug --config "state\\update-harness\\tauri.overlay.json"`,
  );
}

function check(keyCfg) {
  const missing = [];
  if (!existsSync(keyCfg.file)) missing.push("signing private key (path withheld)");
  if (!existsSync(path.join(HARNESS_DIR, "latest.json"))) {
    missing.push("state/update-harness/latest.json");
  }
  if (missing.length > 0) {
    throw new HarnessError(`harness incomplete; missing:\n${missing.map((item) => `  ${item}`).join("\n")}`);
  }
  const feed = JSON.parse(readFileSync(path.join(HARNESS_DIR, "latest.json"), "utf8"));
  return [
    "harness OK:",
    `  version:   ${feed.version}`,
    `  asset url: ${feed.platforms["windows-x86_64"].url}`,
  ];
}

/**
 * Operator-facing key report. Built from public locations and booleans only:
 * the private-key path and the password value are never included. It is
 * printed verbatim (no redaction pass) because every field is public by
 * design; the public-key path is the operator's pointer to the key
 * directory now that private-key paths are withheld from all diagnostics.
 */
export function keysReport(keyCfg, env = process.env) {
  const password = env[PASSWORD_ENV_VAR];
  const passwordSet = typeof password === "string" && password.length > 0;
  return [
    `public key: ${keyCfg.pub}`,
    `private key present: ${existsSync(keyCfg.file) ? "yes" : "no"}`,
    `password source: ${passwordSet ? "environment" : "not set"}`,
  ];
}

/** CLI entry point. Returns the process exit code; never calls process.exit. */
export async function main(argv = process.argv.slice(2), env = process.env, runtime = {}) {
  const io = runtime.io ?? console;
  const spawn = runtime.spawn ?? spawnSync;
  const keyCfg = resolveKeyConfig(runtime.home);
  const redactor = createSecretRedactor();
  registerPath(redactor, keyCfg.file);
  registerPath(redactor, keyCfg.dir);
  const emit = (...parts) => io.log(...parts.map((part) => redactor.redact(String(part))));
  const fail = (message) => io.error(`error: ${redactor.redact(message)}`);

  try {
    switch (argv[0]) {
      case "keys":
        // Printed verbatim: keysReport emits public locations and booleans only.
        for (const line of keysReport(keyCfg, env)) io.log(line);
        break;
      case "sign": {
        const target = argv[1];
        if (!target) throw new HarnessError("usage: node scripts/update-harness.mjs sign <file>");
        const password = resolveKeyPassword(env);
        redactor.register(password);
        runTauriSign({ keyCfg, password, target, redactor, spawn });
        emit(`signed: ${target}.sig`);
        break;
      }
      case "prepare":
      case "metadata": {
        const version = flagValue(argv, "--version");
        if (!version) throw new HarnessError("missing --version <V>");
        stageInstaller(version, flagValue(argv, "--notes"), { keyCfg, redactor, env, spawn, argv, emit });
        break;
      }
      case "serve":
        serve(Number(flagValue(argv, "--port") ?? DEFAULT_PORT), emit);
        break;
      case "overlay": {
        const version = flagValue(argv, "--version");
        if (!version) throw new HarnessError("missing --version <V>");
        writeOverlay(version, Number(flagValue(argv, "--port") ?? DEFAULT_PORT), argv, emit);
        break;
      }
      case "check":
        for (const line of check(keyCfg)) emit(line);
        break;
      default:
        emit("usage: node scripts/update-harness.mjs <keys|sign|prepare|serve|overlay|check>");
        return argv[0] ? 1 : 0;
    }
    return 0;
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
    return 1;
  }
}

function flagValue(argv, name) {
  const index = argv.indexOf(name);
  return index === -1 ? undefined : argv[index + 1];
}
