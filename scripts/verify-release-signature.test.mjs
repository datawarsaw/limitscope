import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import {
  decodeCanonicalBase64,
  normalizeSignature,
  readTrustRoot,
} from "./verify-release-signature.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SCRIPT = path.join(REPO_ROOT, "scripts", "verify-release-signature.mjs");
const HELPER_MANIFEST = path.join(
  REPO_ROOT,
  "tools",
  "release-signature-verifier",
  "Cargo.toml",
);

// Fixed public test vectors copied from minisign-verify 0.2.5's own unit tests.
// No private key or signing operation is involved.
const PUBLIC_KEY_TEXT = `untrusted comment: minisign public key: E7620F1842B4E81F
RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3
`;
const PREHASHED_SIGNATURE = `untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1556193335\tfile:test
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==
`;
const LEGACY_SIGNATURE = `untrusted comment: signature from minisign secret key
RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=
trusted comment: timestamp:1555779966\tfile:test
QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==
`;

let workspace;
let targetDir;
let configPath;

function base64(text) {
  return Buffer.from(text, "utf8").toString("base64");
}

function fixture(name, installerBytes = Buffer.from("test"), signatureText = PREHASHED_SIGNATURE) {
  const dir = path.join(workspace, name);
  mkdirSync(dir, { recursive: true });
  const installerPath = path.join(dir, "LimitScope_0.0.0_x64-setup.exe");
  const signaturePath = `${installerPath}.sig`;
  writeFileSync(installerPath, installerBytes);
  writeFileSync(signaturePath, base64(signatureText));
  return { installerPath, signaturePath };
}

function runCli(arguments_, fixturePaths = fixture("default-cli"), configOverride = configPath) {
  const result = spawnSync(
    process.execPath,
    [
      SCRIPT,
      "--config",
      configOverride,
      "--installer",
      fixturePaths.installerPath,
      "--signature",
      fixturePaths.signaturePath,
      ...arguments_,
    ],
    {
      cwd: REPO_ROOT,
      encoding: "utf8",
      env: { ...process.env, CARGO_TARGET_DIR: targetDir },
      timeout: 120_000,
      windowsHide: true,
    },
  );
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

beforeAll(() => {
  workspace = mkdtempSync(path.join(tmpdir(), "limitscope-signature-tests-"));
  targetDir = path.join(workspace, "cargo-target");
  configPath = path.join(workspace, "tauri.conf.json");
  writeFileSync(
    configPath,
    JSON.stringify({ plugins: { updater: { pubkey: base64(PUBLIC_KEY_TEXT) } } }),
  );
  const build = spawnSync(
    process.env.CARGO ?? "cargo",
    ["build", "--quiet", "--release", "--locked", "--manifest-path", HELPER_MANIFEST],
    {
      cwd: REPO_ROOT,
      encoding: "utf8",
      env: { ...process.env, CARGO_TARGET_DIR: targetDir },
      timeout: 180_000,
      windowsHide: true,
    },
  );
  if (build.status !== 0) {
    throw new Error(`locked verifier helper did not build: ${build.stderr || build.stdout}`);
  }
}, 180_000);

afterAll(() => {
  if (workspace) rmSync(workspace, { recursive: true, force: true });
});

describe("updater signature parsing fails closed", () => {
  it("accepts only a modern four-line Ed25519ph signature", () => {
    const normalized = normalizeSignature(base64(PREHASHED_SIGNATURE));
    expect(normalized.text).toBe(PREHASHED_SIGNATURE);
    expect(normalized.keyId.equals(Buffer.from("1fe8b442180f62e7", "hex"))).toBe(true);
  });

  it("rejects legacy algorithm Ed", () => {
    expect(() => normalizeSignature(base64(LEGACY_SIGNATURE))).toThrow(/not modern Ed25519ph/);
  });

  it("rejects extra content that could create an ambiguous signature", () => {
    expect(() => normalizeSignature(base64(`${PREHASHED_SIGNATURE}extra\n`))).toThrow(
      /exactly four lines/,
    );
  });

  it("rejects non-canonical base64", () => {
    expect(() => decodeCanonicalBase64("Zg==\n", "field")).toThrow(/canonical base64/);
    expect(() => decodeCanonicalBase64("Zh==", "field")).toThrow(/canonical base64/);
  });
});

describe("embedded trust root parsing fails closed", () => {
  it("accepts the production trust root embedded in Tauri config", () => {
    const trustRoot = readTrustRoot();
    expect(trustRoot.text).toMatch(/^untrusted comment: minisign public key(?:[ :])/);
    expect(trustRoot.text.split("\n")).toHaveLength(3);
    expect(trustRoot.keyId.length).toBe(8);
  });

  it("reads the test trust root and exposes its key id", () => {
    const trustRoot = readTrustRoot(configPath);
    expect(trustRoot.text).toBe(PUBLIC_KEY_TEXT);
    expect(trustRoot.keyId.equals(Buffer.from("1fe8b442180f62e7", "hex"))).toBe(true);
  });

  it("rejects missing, malformed, and oversized trust-root material", () => {
    const missing = path.join(workspace, "missing-key.json");
    writeFileSync(missing, JSON.stringify({ plugins: { updater: {} } }));
    expect(() => readTrustRoot(missing)).toThrow(/no plugins\.updater\.pubkey/);

    const malformed = path.join(workspace, "malformed-key.json");
    writeFileSync(malformed, JSON.stringify({ plugins: { updater: { pubkey: "not base64!" } } }));
    expect(() => readTrustRoot(malformed)).toThrow(/canonical base64/);
  });
});

describe("standalone cryptographic verifier", () => {
  it("returns zero only for the exact signed bytes", () => {
    const result = runCli([], fixture("valid"));
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toMatch(/^PASS minisign-verify 0\.2\.5 allow_legacy=false algorithm=ED bytes=4$/m);
  });

  it("returns one for changed installer bytes", () => {
    const paths = fixture("changed-data", Buffer.from("Test"));
    const result = runCli([], paths);
    expect(result.status).toBe(1);
    expect(result.stderr).toMatch(/INVALID_SIGNATURE:/);
  });

  it("returns one for a tampered signed payload", () => {
    const lines = PREHASHED_SIGNATURE.split("\n");
    lines[1] = `${lines[1].slice(0, 20)}A${lines[1].slice(21)}`;
    const result = runCli([], fixture("tampered-payload", Buffer.from("test"), lines.join("\n")));
    expect(result.status).toBe(1);
    expect(result.stderr).toMatch(/INVALID_SIGNATURE:/);
  });

  it("returns one for a tampered trusted comment", () => {
    const tampered = PREHASHED_SIGNATURE.replace("timestamp:1556193335", "timestamp:1556193336");
    const result = runCli([], fixture("tampered-comment", Buffer.from("test"), tampered));
    expect(result.status).toBe(1);
    expect(result.stderr).toMatch(/INVALID_SIGNATURE:/);
  });

  it("returns two rather than skipping when the signature is legacy or malformed", () => {
    const legacy = runCli([], fixture("legacy", Buffer.from("test"), LEGACY_SIGNATURE));
    expect(legacy.status).toBe(2);
    expect(legacy.stderr).toMatch(/not modern Ed25519ph/);

    const malformed = runCli([], fixture("malformed", Buffer.from("test"), "not a signature\n"));
    expect(malformed.status).toBe(2);
    expect(malformed.stderr).toMatch(/UNVERIFIABLE:/);
  });

  it("returns two when the signature key does not match the trust root", () => {
    const keyLines = PUBLIC_KEY_TEXT.split("\n");
    const payload = decodeCanonicalBase64(keyLines[1], "test public key");
    payload.set(Buffer.from("0000000000000000", "hex"), 2);
    const wrongTrustRoot = `${keyLines[0]}
${payload.toString("base64")}
`;
    const wrongConfig = path.join(workspace, "wrong-trust-root.json");
    writeFileSync(wrongConfig, JSON.stringify({ plugins: { updater: { pubkey: base64(wrongTrustRoot) } } }));

    const result = runCli([], fixture("wrong-key"), wrongConfig);
    expect(result.status).toBe(2);
    expect(result.stderr).toMatch(/different key/);
  });

  it("returns two when required paths are absent", () => {
    const result = spawnSync(process.execPath, [SCRIPT], {
      cwd: REPO_ROOT,
      encoding: "utf8",
      env: { ...process.env, INSTALLER_PATH: "", SIG_PATH: "", CARGO_TARGET_DIR: targetDir },
      timeout: 120_000,
      windowsHide: true,
    });
    expect(result.status).toBe(2);
    expect(result.stderr).toMatch(/installer path is missing/);
  });
});
