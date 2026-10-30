#!/usr/bin/env node
/**
 * Fail-closed verification of the updater signature against the trust root
 * embedded in this source tree.
 *
 * The release workflow calls this file without arguments. Paths then come from
 * INSTALLER_PATH and SIG_PATH. Optional long-form arguments exist for local
 * verification and tests. Every missing, malformed, legacy, mismatched, or
 * unverifiable input exits nonzero; there is no SKIP result.
 */

import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_CONFIG = path.join(REPO_ROOT, "src-tauri", "tauri.conf.json");
const HELPER_DIR = path.join(REPO_ROOT, "tools", "release-signature-verifier");
const HELPER_MANIFEST = path.join(HELPER_DIR, "Cargo.toml");
const MAX_CONFIG_BYTES = 1024 * 1024;
const MAX_SIGNATURE_BYTES = 16 * 1024;

class VerificationInputError extends Error {
  constructor(message) {
    super(message);
    this.name = "VerificationInputError";
  }
}

function requireRegularFile(filePath, label, maximumBytes) {
  let stats;
  try {
    stats = statSync(filePath);
  } catch (error) {
    throw new VerificationInputError(`${label} is unreadable: ${error.message}`);
  }
  if (!stats.isFile()) throw new VerificationInputError(`${label} is not a regular file`);
  if (stats.size === 0) throw new VerificationInputError(`${label} is empty`);
  if (maximumBytes !== undefined && stats.size > maximumBytes) {
    throw new VerificationInputError(`${label} exceeds ${maximumBytes} bytes`);
  }
  return stats;
}

function readUtf8(filePath, label, maximumBytes) {
  requireRegularFile(filePath, label, maximumBytes);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(readFileSync(filePath));
  } catch (error) {
    throw new VerificationInputError(`${label} is not valid UTF-8: ${error.message}`);
  }
}

/** Decode one canonical base64 token. Whitespace and ignored trailing bits fail closed. */
export function decodeCanonicalBase64(value, label) {
  if (typeof value !== "string" || value.length === 0) {
    throw new VerificationInputError(`${label} is empty`);
  }
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(value) || value.length % 4 !== 0) {
    throw new VerificationInputError(`${label} is not canonical base64`);
  }
  const decoded = Buffer.from(value, "base64");
  if (decoded.toString("base64") !== value) {
    throw new VerificationInputError(`${label} is not canonical base64`);
  }
  return decoded;
}

function normalizeLines(text) {
  return String(text).replace(/\r\n/g, "\n").trim();
}

/** Read and validate the Tauri updater trust root from a config file. */
export function readTrustRoot(configPath = DEFAULT_CONFIG) {
  const configText = readUtf8(configPath, "updater config", MAX_CONFIG_BYTES);
  let config;
  try {
    config = JSON.parse(configText);
  } catch (error) {
    throw new VerificationInputError(`updater config is not valid JSON: ${error.message}`);
  }

  const armored = config?.plugins?.updater?.pubkey;
  if (typeof armored !== "string" || !armored) {
    throw new VerificationInputError("updater config has no plugins.updater.pubkey");
  }

  const decodedText = new TextDecoder("utf-8", { fatal: true }).decode(
    decodeCanonicalBase64(armored, "updater public-key armor"),
  );
  const lines = normalizeLines(decodedText).split("\n");
  if (lines.length !== 2) {
    throw new VerificationInputError("updater public key must contain exactly two lines");
  }
  if (!lines[0].startsWith("untrusted comment: minisign public key")) {
    throw new VerificationInputError("updater public key has an unexpected comment");
  }

  const payload = decodeCanonicalBase64(lines[1], "updater public-key payload");
  if (payload.length !== 42) {
    throw new VerificationInputError(`updater public-key payload is ${payload.length} bytes, expected 42`);
  }
  if (payload.subarray(0, 2).toString("latin1") !== "Ed") {
    throw new VerificationInputError("updater public key is not an Ed25519 minisign key");
  }

  return {
    text: `${lines[0]}\n${lines[1]}\n`,
    keyId: payload.subarray(2, 10),
  };
}

/** Normalize Tauri's base64 armor or raw minisign text into exactly four verified-format lines. */
export function normalizeSignature(text) {
  if (typeof text !== "string" || !text.trim()) {
    throw new VerificationInputError("updater signature is empty");
  }
  const trimmed = normalizeLines(text);
  let minisignText = trimmed;

  if (!trimmed.startsWith("untrusted comment:")) {
    try {
      minisignText = normalizeLines(
        new TextDecoder("utf-8", { fatal: true }).decode(
          decodeCanonicalBase64(trimmed, "updater signature armor"),
        ),
      );
    } catch (error) {
      if (error instanceof VerificationInputError) throw error;
      throw new VerificationInputError(`updater signature armor is not valid UTF-8: ${error.message}`);
    }
  }

  const lines = minisignText.split("\n");
  if (lines.length !== 4) {
    throw new VerificationInputError("updater signature must contain exactly four lines");
  }
  if (!lines[0].startsWith("untrusted comment:")) {
    throw new VerificationInputError("updater signature has an unexpected untrusted comment");
  }
  if (!lines[2].startsWith("trusted comment: ")) {
    throw new VerificationInputError("updater signature has an unexpected trusted comment");
  }

  const signedPayload = decodeCanonicalBase64(lines[1], "updater signature payload");
  if (signedPayload.length !== 74) {
    throw new VerificationInputError(`updater signature payload is ${signedPayload.length} bytes, expected 74`);
  }
  if (signedPayload.subarray(0, 2).toString("latin1") !== "ED") {
    throw new VerificationInputError("updater signature is not modern Ed25519ph (algorithm ED)");
  }
  decodeCanonicalBase64(lines[3], "updater global signature");
  if (decodeCanonicalBase64(lines[3], "updater global signature").length !== 64) {
    throw new VerificationInputError("updater global signature must be 64 bytes");
  }

  return {
    text: `${lines.join("\n")}\n`,
    keyId: signedPayload.subarray(2, 10),
  };
}

function parseArguments(argv) {
  const options = {
    config: DEFAULT_CONFIG,
    installer: process.env.INSTALLER_PATH,
    signature: process.env.SIG_PATH,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--help" || argument === "-h") {
      options.help = true;
      continue;
    }
    const names = {
      "--config": "config",
      "--installer": "installer",
      "--signature": "signature",
    };
    const key = names[argument];
    if (!key) throw new VerificationInputError(`unknown argument: ${argument}`);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) {
      throw new VerificationInputError(`missing value for ${argument}`);
    }
    options[key] = value;
    index += 1;
  }
  return options;
}

function printHelp() {
  console.log(`usage: node scripts/verify-release-signature.mjs [options]

Options:
  --installer <path>   Installer bytes (default: INSTALLER_PATH)
  --signature <path>   Tauri .sig file (default: SIG_PATH)
  --config <path>      Tauri config containing the updater trust root
  --help               Show this help

Exit codes: 0 verified, 1 invalid signature, 2 input or verification tool failure.`);
}

export function verifyReleaseSignature({ installer, signature, config }, runtime = {}) {
  if (!installer) throw new VerificationInputError("installer path is missing (INSTALLER_PATH or --installer)");
  if (!signature) throw new VerificationInputError("signature path is missing (SIG_PATH or --signature)");

  const installerPath = path.resolve(installer);
  const signaturePath = path.resolve(signature);
  const configPath = path.resolve(config ?? DEFAULT_CONFIG);

  requireRegularFile(installerPath, "installer");
  const trustRoot = readTrustRoot(configPath);
  const signatureText = readUtf8(signaturePath, "updater signature", MAX_SIGNATURE_BYTES);
  const normalizedSignature = normalizeSignature(signatureText);
  if (!trustRoot.keyId.equals(normalizedSignature.keyId)) {
    throw new VerificationInputError("updater signature was created by a different key than the embedded trust root");
  }

  const workspace = mkdtempSync(path.join(tmpdir(), "limitscope-release-signature-"));
  try {
    const publicKeyPath = path.join(workspace, "trust-root.pub");
    const normalizedSignaturePath = path.join(workspace, "installer.minisign");
    writeFileSync(publicKeyPath, trustRoot.text, { encoding: "utf8", flag: "wx" });
    writeFileSync(normalizedSignaturePath, normalizedSignature.text, { encoding: "utf8", flag: "wx" });

    const environment = { ...process.env };
    if (!environment.CARGO_TARGET_DIR) {
      environment.CARGO_TARGET_DIR = path.join(workspace, "target");
    }
    const cargo = runtime.cargo ?? process.env.CARGO ?? "cargo";
    const result = spawnSync(
      cargo,
      [
        "run",
        "--quiet",
        "--release",
        "--locked",
        "--manifest-path",
        HELPER_MANIFEST,
        "--",
        publicKeyPath,
        installerPath,
        normalizedSignaturePath,
      ],
      {
        cwd: REPO_ROOT,
        env: environment,
        encoding: "utf8",
        timeout: 600_000,
        windowsHide: true,
        maxBuffer: 1024 * 1024,
      },
    );

    if (result.error) {
      throw new VerificationInputError(`verification tool could not run: ${result.error.message}`);
    }
    const stdout = (result.stdout ?? "").trim();
    const stderr = (result.stderr ?? "").trim();
    if (result.status === 0 && /^PASS minisign-verify 0\.2\.5 allow_legacy=false algorithm=ED bytes=\d+$/m.test(stdout)) {
      return { status: "PASS", detail: stdout };
    }
    if (result.status === 1 && stderr.includes("INVALID_SIGNATURE:")) {
      return { status: "FAIL", detail: stderr };
    }
    throw new VerificationInputError(
      `verification tool failed closed (exit ${result.status ?? "unknown"}): ${stderr || stdout || "no output"}`,
    );
  } finally {
    rmSync(workspace, { recursive: true, force: true });
  }
}

async function main(argv = process.argv.slice(2)) {
  try {
    const options = parseArguments(argv);
    if (options.help) {
      printHelp();
      return 0;
    }
    const result = verifyReleaseSignature(options);
    if (result.status === "PASS") {
      console.log(result.detail);
      return 0;
    }
    console.error(result.detail);
    return 1;
  } catch (error) {
    console.error(`UNVERIFIABLE: ${error instanceof Error ? error.message : String(error)}`);
    return 2;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await main();
}
