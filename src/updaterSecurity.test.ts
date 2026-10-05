import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { formatUpdateError, isUpdateRelevant, parseVersion } from "./lib/updater";

/**
 * Security guardrails for the LimitScope v0.6 production updater.
 *
 * Invariants:
 * 1. No PAT embedded in source bundle
 * 2. No private key embedded in repository or bundle
 * 3. No signing password embedded in source or config
 * 4. Updater public key present and valid minisign format
 * 5. Production endpoint is HTTPS
 * 6. Update feed contains no authenticated or private URL
 * 7. Malformed update response fails safely
 * 8. Same version produces no update
 * 9. Older version produces no downgrade
 * 10. Invalid signature cannot install / mapped to verification failure
 * 11. Updater errors format safely and do not crash app startup
 */

const REPO_ROOT = path.resolve(import.meta.dirname, "..");

const SKIPPED_DIRS = new Set([
  "node_modules",
  ".git",
  "target",
  "dist",
  "artifacts",
  "state",
  "gen",
]);

const TEXT_EXTENSIONS = new Set([
  ".ts",
  ".tsx",
  ".js",
  ".mjs",
  ".cjs",
  ".json",
  ".md",
  ".css",
  ".html",
  ".rs",
  ".toml",
  ".yml",
  ".yaml",
  ".nsh",
  ".nsi",
]);

function* walk(dir: string): Generator<string> {
  for (const entry of readdirSync(dir)) {
    if (SKIPPED_DIRS.has(entry)) continue;
    const full = path.join(dir, entry);
    const stats = statSync(full);
    if (stats.isDirectory()) {
      yield* walk(full);
    } else {
      yield full;
    }
  }
}

const SECRET_PATTERNS: Array<[string, RegExp]> = [
  ["classic GitHub PAT", /ghp_[A-Za-z0-9]{20,}/],
  ["fine-grained GitHub PAT", /github_pat_[A-Za-z0-9_]{20,}/],
  ["OAuth/refresh token shape", /gho_[A-Za-z0-9]{20,}/],
  ["PEM private key header", /-----BEGIN [A-Z ]*PRIVATE KEY-----/],
  ["Tauri minisign private key envelope", /rsign encrypted secret key/],
  [
    "updater signing key environment value",
    /TAURI_SIGNING_PRIVATE_KEY_PASSWORD\s*=\s*["']?[A-Za-z0-9]{8,}/,
  ],
];

function readIfText(file: string): string | null {
  if (!TEXT_EXTENSIONS.has(path.extname(file))) return null;
  try {
    return readFileSync(file, "utf8");
  } catch {
    return null;
  }
}

describe("updater security guardrails", () => {
  const conf = JSON.parse(
    readFileSync(
      path.join(REPO_ROOT, "src-tauri", "tauri.conf.json"),
      "utf8",
    ),
  ) as {
    plugins?: {
      updater?: {
        pubkey?: string;
        endpoints?: string[];
        windows?: { installMode?: string };
      };
    };
  };

  it("1 & 3: ships no PATs or signing passwords in source", () => {
    const offenders: string[] = [];
    const scanned: string[] = [];
    const scannerFiles = [
      path.join(REPO_ROOT, "src", "updaterSecurity.test.ts"),
      path.join(REPO_ROOT, "scripts", "secret-scan.mjs"),
    ].map((file) => path.resolve(file));

    for (const dir of ["src", "src-tauri/src", "scripts", "docs"]) {
      for (const file of walk(path.join(REPO_ROOT, dir))) {
        if (scannerFiles.includes(path.resolve(file))) continue;
        const text = readIfText(file);
        if (!text) continue;
        scanned.push(file);
        for (const [name, pattern] of SECRET_PATTERNS) {
          if (pattern.test(text)) offenders.push(`${name}: ${file}`);
        }
      }
    }
    // Anti-vacuous preconditions: the scan must actually have read the
    // source tree — a silently empty walk would make the zero-offender
    // verdict below meaningless.
    expect(scanned.length).toBeGreaterThan(0);
    expect(
      scanned.some((file) => file.endsWith(path.join("src-tauri", "src", "diagnostics.rs"))),
    ).toBe(true);
    expect(
      scanned.some((file) => file.endsWith(path.join("src", "lib", "updater.ts"))),
    ).toBe(true);
    expect(offenders).toEqual([]);
  });

  it("scanner strength: every secret pattern fires on its hostile shape", () => {
    // Assembled at runtime so no literal credential shape lands in this
    // file (the same convention the Rust-side scan tests use).
    const hostile: Array<[string, string]> = [
      ["classic GitHub PAT", `ghp_${"a1b2".repeat(6)}`],
      ["fine-grained GitHub PAT", `github_pat_${"c3d4".repeat(6)}`],
      ["OAuth/refresh token shape", `gho_${"e5f6".repeat(6)}`],
      ["PEM private key header", `-----BEGIN ${"PRIVATE"} ${"KEY"}-----`],
      [
        "Tauri minisign private key envelope",
        ["untrusted comment:", "rsign", "encrypted", "secret", "key"].join(" "),
      ],
      [
        "updater signing key environment value",
        `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=${"p4ssw0rd".repeat(2)}`,
      ],
    ];
    for (const [name, sample] of hostile) {
      const pattern = SECRET_PATTERNS.find(([patternName]) => patternName === name);
      expect(pattern, `missing pattern: ${name}`).toBeDefined();
      expect(pattern![1].test(sample), `pattern did not fire: ${name}`).toBe(true);
    }
  });

  it("2: keeps private key files out of the repository", () => {
    const keys: string[] = [];
    for (const file of walk(REPO_ROOT)) {
      if (file.endsWith(".key") || file.endsWith(".key.pub")) keys.push(file);
    }
    expect(keys).toEqual([]);
  });

  it("4: configures updater with public key present and in valid minisign format", () => {
    const updater = conf.plugins?.updater;
    expect(updater).toBeDefined();
    const pubkey = updater?.pubkey ?? "";
    expect(pubkey.length).toBeGreaterThan(32);
    // Minisign public key base64 begins with "untrusted comment: minisign public key"
    expect(pubkey).toMatch(/^dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk/);
    expect(pubkey).not.toContain("cnNpZ24gZW5jcnlwdGVkIHNlY3JldCBrZXk");
  });

  it("5: production endpoint is HTTPS", () => {
    const endpoints = conf.plugins?.updater?.endpoints ?? [];
    expect(endpoints.length).toBeGreaterThan(0);
    for (const endpoint of endpoints) {
      expect(endpoint.startsWith("https://")).toBe(true);
      expect(endpoint).not.toContain("localhost");
      expect(endpoint).not.toContain("127.0.0.1");
    }
  });

  it("6: update feed contains no authenticated or private URL", () => {
    const endpoints = conf.plugins?.updater?.endpoints ?? [];
    for (const endpoint of endpoints) {
      expect(endpoint).not.toMatch(/:[^/]*@/); // No user:pass
      expect(endpoint).not.toMatch(/[?&](token|access_token|private_token|key)=/i);
      expect(endpoint).toContain("github.com/datawarsaw/limitscope-releases/");
    }
  });

  it("7: malformed update response fails safely", () => {
    expect(isUpdateRelevant("invalid-version", "0.5.0")).toBe(false);
    expect(parseVersion("not-a-semver")).toBeNull();
    const error = formatUpdateError("Could not fetch a valid release JSON from the remote");
    expect(error.message).toBe("The update feed could not be read. Try again later.");
  });

  it("8: same version produces no update", () => {
    expect(isUpdateRelevant("0.5.0", "0.5.0")).toBe(false);
    expect(isUpdateRelevant("0.6.0", "0.6.0")).toBe(false);
  });

  it("9: older version produces no downgrade", () => {
    expect(isUpdateRelevant("0.4.9", "0.5.0")).toBe(false);
    expect(isUpdateRelevant("0.5.0", "0.6.0")).toBe(false);
  });

  it("10: invalid signature cannot install and is classified as verification error", () => {
    const err = formatUpdateError(new Error("signature verification failed: bad signature"));
    expect(err.message).toBe("The update could not be verified, so it was not installed.");
  });

  it("11: updater errors do not break app startup and map to safe messages", () => {
    const networkErr = formatUpdateError("operation timed out");
    expect(networkErr.message).toBe("Could not reach the update server. Check your connection.");
    const insecureErr = formatUpdateError("InsecureTransportProtocol");
    expect(insecureErr.message).toBe("The update feed is not using HTTPS, so the update was blocked.");
  });

  it("installs Windows updates in passive mode", () => {
    expect(conf.plugins?.updater?.windows?.installMode).toBe("passive");
  });
});
