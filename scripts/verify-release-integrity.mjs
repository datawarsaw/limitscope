/**
 * verify-release-integrity.mjs — read-only LimitScope release integrity verifier.
 *
 * Run via `node scripts/verify-release-integrity.mjs` (no shebang: this module
 * is imported by verify-release-integrity.test.mjs, and a shebang breaks the
 * module under vite-node's import pipeline).
 *
 * Automates the release-integrity checks that were manually proven during the
 * v0.8.4 and v0.8.5 deliveries (see docs/release-runbook-v0.8.0.md Phase 4,
 * docs/updater-production.md and the hardened .github/workflows/release.yml).
 * It verifies; it never mutates:
 * no tagging, no publishing, no signing, no writes inside the repository.
 *
 * Usage:
 *   node scripts/verify-release-integrity.mjs --version 0.8.5 [options]
 *
 * Modes:
 *   Full (default)          source checks at the release ref + live companion
 *                           release/feed checks via `gh` and anonymous HTTPS.
 *   --source-only           pre-dispatch candidate mode: source checks only
 *                           (release/feed checks do not exist yet).
 *   --metadata <latest.json>
 *                           offline metadata mode: feed checks run against a
 *                           local file; network checks (R1/R7/R8) are skipped.
 *   --rc                    release-candidate mode (opt-in): verifies an
 *                           x.y.z-rc.N companion release published as a
 *                           GitHub prerelease and proves the production stable
 *                           feed still serves the stable release. Requires
 *                           --stable-feed-version.
 *
 * Options:
 *   --version <v>            Required release version: x.y.z, or x.y.z-rc.N
 *                            with --rc.
 *   --expected-sha <sha>     Expected product SHA. When omitted, the local tag
 *                            v<version> (if present) defines it; otherwise HEAD
 *                            is verified as the release candidate.
 *   --expect-tag             Fail when the local tag v<version> is absent.
 *   --metadata <path>        Verify this latest.json instead of the live feed.
 *   --sig <path>             Minisign .sig file to check against the feed
 *                            (offline mode) or the published asset (live).
 *   --installer <path>       Installer for cryptographic signature verification.
 *   --verify-signature       Run real minisign verification (Ed25519ph,
 *                            allow_legacy=false) via an ephemeral cargo helper
 *                            that reproduces the method proven in v0.8.4
 *                            (minisign-verify 0.2.5). Skipped (never faked)
 *                            when cargo or the installer is unavailable.
 *   --manifest <path>        Cross-check a release-artifacts-manifest.txt
 *                            (workflow artifact) against the product SHA and,
 *                            when the installer is available, its SHA-256.
 *   --allow-experimental-claude
 *                            Downgrade the forbidden-Claude-registration check
 *                            to a skip (for future explicitly enabled builds).
 *   --rc                    Opt-in RC mode (see Modes above).
 *   --stable-feed-version <v>
 *                            Mandatory with --rc: the stable x.y.z version the
 *                            production `releases/latest` feed must still serve.
 *   --json                   Emit the structured check report instead of the
 *                            human-readable PASS/FAIL lines.
 *   --help                   Show this help.
 *
 * Checks (IDs are stable):
 *   S1 worktree-clean        Current worktree has no modifications. Enforced
 *                            only in candidate mode (verifying HEAD); tag/SHA
 *                            verification reads the committed tree, so the
 *                            local scratch state is not applicable (skip).
 *   S2 source-sha-exists     The verified tree resolves to a real commit.
 *   S3 version-surfaces      package.json, package-lock.json (root and
 *                            packages[""]), src-tauri/tauri.conf.json,
 *                            src-tauri/Cargo.toml and src-tauri/Cargo.lock
 *                            (rate-limits crate) all carry the release
 *                            version; productName/mainBinaryName LimitScope.
 *   S4 tag-identity          Annotated tag v<version> exists (when required)
 *                            and dereferences to the expected product SHA.
 *   S5 updater-contract      Updater endpoint is exactly the production feed,
 *                            identifier is the stable app identity, and the
 *                            embedded updater public key matches the pinned
 *                            production trust root (key id 025FF36DE3FF44EB).
 *   S6 no-dev-url            No dev URL (localhost/127.0.0.1) leaks outside
 *                            build.devUrl; CSP uses the production IPC custom
 *                            protocol and does not reference the dev origin.
 *   S7 providers-registered  The five canonical providers (codex, zai,
 *                            opencode-go, antigravity, grok) are registered.
 *   S8 claude-absent         No experimental Claude registration is present.
 *   R1 release-exists        Companion release v<version> exists, is not draft
 *                            or prerelease, and is the live latest release.
 *                            With --rc: the release must instead BE a
 *                            published (non-draft) prerelease.
 *   R2 asset-set             Release carries exactly the installer, its .sig
 *                            and latest.json.
 *   R3 feed-version          latest.json version equals the release version.
 *   R4 feed-url              latest.json windows-x86_64 URL is the canonical
 *                            download URL of the published installer asset.
 *   R5 feed-signature        latest.json signature decodes to the published
 *                            .sig asset bytes (feed and asset cannot drift).
 *   R6 no-private-urls       No local/private URLs or credential shapes in
 *                            the feed (paths reported, values never printed).
 *   R7 ci-source-sha         A successful release.yml workflow run exists
 *                            whose head SHA is the expected product SHA.
 *   R8 live-feed-identity    Stable mode: the live
 *                            `releases/latest/download/latest.json` endpoint
 *                            serves bytes identical to this release's feed
 *                            (fails with a note when a newer release has
 *                            superseded it). RC mode: the live stable endpoint
 *                            must still redirect to and serve exactly the
 *                            supplied stable version — never the RC.
 *   G1 signature-structure   The .sig parses as a modern minisign signature
 *                            (algorithm ED) whose key id equals the trust
 *                            root's key id.
 *   G3 minisign-verify       Cryptographic verification of the installer
 *                            against the trust root (--verify-signature only).
 *
 * Exit codes: 0 = all enabled checks passed (skips allowed, reasons printed);
 * 1 = at least one check failed (fail closed); 2 = could not verify
 * (missing tool, unreadable input) — treat as failure.
 *
 * Security: read-only (git read commands, `gh` read commands, anonymous HTTPS
 * GETs, temp files outside the repository). No secret output, no credential
 * discovery, no private-key dependency: signature work uses only the public
 * trust root embedded in src-tauri/tauri.conf.json.
 */

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const PUBLIC_FEED_REPO = "datawarsaw/limitscope-releases";
const SOURCE_REPO = "datawarsaw/limitscope";
const PRODUCTION_FEED_URL = `https://github.com/${PUBLIC_FEED_REPO}/releases/latest/download/latest.json`;
const APP_IDENTIFIER = "com.ratelimits.desktop";
// The production minisign trust root (key id as printed in the key comment,
// little-endian over the raw 8 key-id bytes). A change here is a managed key
// rotation per docs/updater-production.md and must fail loudly.
const PRODUCTION_KEY_ID = "025FF36DE3FF44EB";
// Canonical registry order from docs/release-runbook-v0.8.0.md.
const CANONICAL_PROVIDER_MODULES = ["codex", "opencode_go", "zai", "antigravity", "grok"];
// Strict release-channel matchers. The RC channel is derived from the version
// shape — there is no operator toggle — and anything malformed or ambiguous
// fails closed (docs/updater-production.md "Public RC channel").
export const STABLE_VERSION_PATTERN = /^\d+\.\d+\.\d+$/;
export const RC_VERSION_PATTERN = /^\d+\.\d+\.\d+-rc\.\d+$/;

function usage() {
  console.log(`verify-release-integrity — read-only LimitScope release integrity verifier

Usage:
  node scripts/verify-release-integrity.mjs --version <v> [options]

Options:
  --version <v>                 Release version to verify (required)
  --expected-sha <sha>          Expected product commit SHA
  --expect-tag                  Require the local tag v<version> to exist
  --source-only                 Verify source surfaces only (candidate mode)
  --metadata <latest.json>      Verify a local feed file instead of the live one
  --sig <path>                  Minisign signature file for feed/structure checks
  --installer <path>            Installer for cryptographic verification
  --verify-signature            Cryptographically verify the installer (cargo helper)
  --manifest <path>             Cross-check release-artifacts-manifest.txt
  --allow-experimental-claude   Skip the forbidden-Claude-registration check
  --rc                          Opt-in RC mode: verify an x.y.z-rc.N prerelease
                                release and prove stable-feed isolation
  --stable-feed-version <v>     Stable x.y.z the live feed must still serve
                                (required with --rc)
  --json                        Structured output
  --help                        This help
`);
}

// ---------------------------------------------------------------------------
// Pure helpers (exported for scripts/verify-release-integrity.test.mjs).
// ---------------------------------------------------------------------------

/** Parse a minisign public key file text into { algorithm, keyId } (raw big-endian hex). */
export function parsePubKey(text) {
  const lines = String(text).trim().split(/\r?\n/);
  if (lines[0] === undefined || !lines[0].startsWith("untrusted comment:")) {
    throw new Error("public key file does not start with an untrusted comment");
  }
  const keyLine = lines.find((line, index) => index > 0 && line.trim() !== "");
  if (!keyLine) throw new Error("public key file has no base64 payload");
  const raw = Buffer.from(keyLine.trim(), "base64");
  if (raw.length < 42) throw new Error("public key payload is too short");
  const algorithm = raw.subarray(0, 2).toString("latin1");
  if (algorithm !== "Ed") throw new Error(`unexpected public key algorithm ${JSON.stringify(algorithm)}`);
  return { algorithm, keyId: raw.subarray(2, 10).toString("hex").toUpperCase() };
}

/** Minisign signature text is optionally wrapped in Tauri's .sig armor: the
 *  whole signature file base64-encoded on one line (what the bundler emits and
 *  what latest.json embeds verbatim). Normalize either form to minisign text
 *  with LF line endings so comparisons are checkout-independent (a git
 *  autocrlf checkout hands the .sig fixture CRLF line endings). */
export function minisignSigText(text) {
  const trimmed = String(text).trim().replace(/\r\n/g, "\n");
  if (trimmed.startsWith("untrusted comment:")) return trimmed;
  const decoded = Buffer.from(trimmed, "base64").toString("utf8").replace(/\r\n/g, "\n");
  if (!decoded.startsWith("untrusted comment:")) throw new Error("signature is neither minisign text nor its base64 armor");
  return decoded;
}

/** Parse a minisign signature file text into { algorithm, keyId }. Accepts the
 *  raw minisign text or its base64 armor. */
export function parseSig(text) {
  const lines = minisignSigText(text).split(/\r?\n/);
  if (lines[0] === undefined || !lines[0].startsWith("untrusted comment:")) {
    throw new Error("signature file does not start with an untrusted comment");
  }
  const sigLine = lines[1];
  if (!sigLine) throw new Error("signature file has no base64 payload");
  const raw = Buffer.from(sigLine.trim(), "base64");
  if (raw.length < 2 + 8 + 64) throw new Error("signature payload is too short");
  const algorithm = raw.subarray(0, 2).toString("latin1");
  if (algorithm !== "ED") throw new Error(`unexpected signature algorithm ${JSON.stringify(algorithm)} (legacy or unknown)`);
  return { algorithm, keyId: raw.subarray(2, 10).toString("hex").toUpperCase() };
}

/** Minisign prints key ids little-endian; the comments and docs use that form. */
export function keyIdCommentForm(rawKeyIdHex) {
  return Buffer.from(rawKeyIdHex, "hex").reverse().toString("hex").toUpperCase();
}

/** The updater public key embedded in tauri.conf.json is base64 of the key FILE text. */
export function decodeConfigPubkey(pubkeyB64) {
  const fileText = Buffer.from(String(pubkeyB64), "base64").toString("utf8");
  return parsePubKey(fileText);
}

export function validateVersionSurfaces({ pkg, pkgLock, conf, cargoToml, cargoLock }, version) {
  const issues = [];
  const pkgLockVersions = parsePkgLockVersions(pkgLock);
  // Five authoritative surfaces (six version fields): every release bump in
  // this repository's history (v0.8.0, v0.8.1, v0.8.3, v0.8.4, v0.8.5) bumped
  // exactly these five files, and the v0.8.0 RC record enumerates them as the
  // canonical set. release.yml's agreement gate reads the
  // same fields except package-lock.json (its npm ci step fails on lock drift).
  const surfaces = [
    ["package.json", pkg?.version],
    ["package-lock.json root version", pkgLockVersions.root],
    ['package-lock.json packages[""] version', pkgLockVersions.packages],
    ["src-tauri/tauri.conf.json", conf?.version],
    ["src-tauri/Cargo.toml", parseCargoPackageVersion(cargoToml)],
    ["src-tauri/Cargo.lock", parseCargoLockVersion(cargoLock)],
  ];
  for (const [name, found] of surfaces) {
    if (found !== version) issues.push(`${name} ${JSON.stringify(found ?? "missing")} != ${version}`);
  }
  for (const field of ["productName", "mainBinaryName"]) {
    if (conf?.[field] !== "LimitScope") {
      issues.push(`tauri.conf.json ${field} is ${JSON.stringify(conf?.[field] ?? "missing")}, installer-name contract requires LimitScope`);
    }
  }
  return issues;
}

/** Parse the two version fields release practice keeps in agreement inside
 *  package-lock.json: the root "version" and packages[""].version. */
export function parsePkgLockVersions(pkgLockText) {
  let lock;
  try {
    lock = JSON.parse(String(pkgLockText));
  } catch {
    return { root: undefined, packages: undefined };
  }
  return {
    root: typeof lock?.version === "string" ? lock.version : undefined,
    packages: typeof lock?.packages?.[""]?.version === "string" ? lock.packages[""].version : undefined,
  };
}

export function parseCargoPackageVersion(cargoToml) {
  const lines = String(cargoToml).split(/\r?\n/);
  let inPackage = false;
  for (const line of lines) {
    if (line.startsWith("[")) inPackage = line.trim() === "[package]";
    else if (inPackage) {
      const match = line.match(/^\s*version\s*=\s*"([^"]+)"/);
      if (match) return match[1];
    }
  }
  return undefined;
}

export function parseCargoLockVersion(cargoLock) {
  const match = String(cargoLock).match(/\[\[package\]\]\s*name = "rate-limits"\s*version = "([^"]+)"/);
  return match ? match[1] : undefined;
}

/** S5: updater endpoint/identifier/trust-root contract of tauri.conf.json. */
export function validateUpdaterContract(conf, { pinnedKeyId = PRODUCTION_KEY_ID } = {}) {
  const issues = [];
  const endpoints = conf?.plugins?.updater?.endpoints;
  if (!Array.isArray(endpoints) || endpoints.length !== 1 || endpoints[0] !== PRODUCTION_FEED_URL) {
    issues.push(`updater endpoints ${JSON.stringify(endpoints ?? "missing")} != exactly [${PRODUCTION_FEED_URL}]`);
  }
  if (conf?.identifier !== APP_IDENTIFIER) {
    issues.push(`identifier ${JSON.stringify(conf?.identifier ?? "missing")} != ${APP_IDENTIFIER} (stable identity required for update continuity)`);
  }
  const pubkeyB64 = conf?.plugins?.updater?.pubkey;
  if (typeof pubkeyB64 !== "string" || !pubkeyB64) {
    issues.push("plugins.updater.pubkey is missing — the updater trust root must stay embedded");
    return { issues, pubkey: undefined };
  }
  let pubkey;
  try {
    pubkey = decodeConfigPubkey(pubkeyB64);
  } catch (error) {
    issues.push(`plugins.updater.pubkey does not decode to a minisign public key: ${error.message}`);
    return { issues, pubkey: undefined };
  }
  if (keyIdCommentForm(pubkey.keyId) !== pinnedKeyId) {
    issues.push(
      `updater trust root key id ${keyIdCommentForm(pubkey.keyId)} != pinned key id ${pinnedKeyId} — ` +
        "a trust-root change is a managed key rotation (docs/updater-production.md) and must be explicit",
    );
  }
  return { issues, pubkey };
}

/** S6: dev URLs must exist only under build.devUrl; the CSP must use the
 *  production IPC custom protocol and must not reference the dev origin. */
export function findDevUrlLeaks(conf) {
  const leaks = [];
  const LOCAL_URL = /localhost|127\.0\.0\.1/;
  const walk = (value, jsonPath) => {
    if (typeof value === "string") {
      if (jsonPath === "app.security.csp") {
        // The production IPC custom protocol entry is required and safe.
        if (!/ipc:|http:\/\/ipc\.localhost/.test(value)) {
          leaks.push({ path: jsonPath, reason: "CSP does not use the production IPC custom protocol (ipc: / http://ipc.localhost)" });
        }
        if (LOCAL_URL.test(value.replace(/http:\/\/ipc\.localhost/g, ""))) {
          leaks.push({ path: jsonPath, reason: "CSP references a dev origin" });
        }
        return;
      }
      if (jsonPath === "build.devUrl") {
        if (!LOCAL_URL.test(value)) leaks.push({ path: jsonPath, reason: "build.devUrl is not a local dev URL (unexpected)" });
        return;
      }
      if (LOCAL_URL.test(value)) leaks.push({ path: jsonPath, reason: "local/dev URL outside build.devUrl" });
      return;
    }
    if (Array.isArray(value)) {
      value.forEach((entry, index) => walk(entry, `${jsonPath}[${index}]`));
      return;
    }
    if (value && typeof value === "object") {
      for (const [key, entry] of Object.entries(value)) walk(entry, jsonPath ? `${jsonPath}.${key}` : key);
    }
  };
  walk(conf, "");
  return leaks;
}

/** S7/S8: provider registration status of src-tauri/src/main.rs. */
export function extractProviderStatus(mainRs) {
  const text = String(mainRs);
  const missingMods = [];
  const missingHandlers = [];
  for (const provider of CANONICAL_PROVIDER_MODULES) {
    if (!new RegExp(`^mod\\s+${provider}\\s*;`, "m").test(text)) missingMods.push(provider);
    if (!text.includes(`${provider}::get_${provider}_usage`)) missingHandlers.push(provider);
  }
  const claudeRegistered = /\bmod\s+claude\b/.test(text) || /\bclaude::/.test(text);
  return { missingMods, missingHandlers, claudeRegistered };
}

/** R3/R4: feed version and canonical artifact URL. */
export function validateFeed(feed, { version, installerName }) {
  const issues = [];
  if (!feed || typeof feed !== "object") return ["latest.json is not a JSON object"];
  if (feed.version !== version) issues.push(`latest.json version ${JSON.stringify(feed.version ?? "missing")} != ${version}`);
  const platform = feed.platforms?.["windows-x86_64"];
  if (!platform) {
    issues.push('latest.json has no platforms["windows-x86_64"] entry');
    return issues;
  }
  const expectedUrl = `https://github.com/${PUBLIC_FEED_REPO}/releases/download/v${version}/${installerName}`;
  if (platform.url !== expectedUrl) issues.push(`latest.json url ${JSON.stringify(platform.url)} != ${expectedUrl}`);
  if (typeof platform.signature !== "string" || !platform.signature) {
    issues.push("latest.json windows-x86_64 signature is missing or empty");
  }
  return issues;
}

const PRIVATE_URL_PATTERNS = [
  ["localhost URL", /localhost/i],
  ["loopback URL", /127\.0\.0\.1|\[::1\]/],
  ["unspecified host", /0\.0\.0\.0/],
  ["private-range IPv4", /(?:^|[^.\w])(?:10|192\.168|172\.(?:1[6-9]|2\d|3[01]))\.\d+\.\d+/],
  ["link-local IPv4", /169\.254\.\d+\.\d+/],
  ["file URL", /file:\/\//i],
  ["plaintext http URL", /http:\/\//],
    ["private source repo slug", /datawarsaw\/limitscope(?!-releases)/],
  ["GitHub PAT shape", /gh[pousr]_[A-Za-z0-9_]{16,}/],
  ["fine-grained PAT shape", /github_pat_[A-Za-z0-9_]{20,}/],
];

/** R6: scan every string value of the feed; findings report paths, never values. */
export function scanFeedForPrivateUrls(feed) {
  const findings = [];
  const walk = (value, jsonPath) => {
    if (typeof value === "string") {
      for (const [name, pattern] of PRIVATE_URL_PATTERNS) {
        if (pattern.test(value)) findings.push({ path: jsonPath, finding: name });
      }
      return;
    }
    if (Array.isArray(value)) {
      value.forEach((entry, index) => walk(entry, `${jsonPath}[${index}]`));
      return;
    }
    if (value && typeof value === "object") {
      for (const [key, entry] of Object.entries(value)) walk(entry, jsonPath ? `${jsonPath}.${key}` : key);
    }
  };
  walk(feed, "");
  return findings;
}

/** R2: exact companion-release asset set. */
export function validateAssetSet(assetNames, version) {
  const issues = [];
  const installerName = `LimitScope_${version}_x64-setup.exe`;
  const expected = new Set([installerName, `${installerName}.sig`, "latest.json"]);
  const found = new Set(assetNames);
  for (const name of expected) if (!found.has(name)) issues.push(`missing release asset: ${name}`);
  for (const name of assetNames) if (!expected.has(name)) issues.push(`unexpected release asset: ${name}`);
  return issues;
}

/** R5: latest.json embeds the .sig asset content verbatim (the base64 armor
 *  of the minisign signature); compare the normalized forms. */
export function sigMatchesFeed(feed, sigText) {
  const embedded = feed?.platforms?.["windows-x86_64"]?.signature;
  if (typeof embedded !== "string" || !embedded) return false;
  try {
    return minisignSigText(embedded) === minisignSigText(sigText);
  } catch {
    return false;
  }
}

/** S4: tag identity decision from git facts (pure, unit-testable). */
export function evaluateTagIdentity({ tagExists, tagType, tagCommit, expectedSha, requireTag }) {
  if (!tagExists) {
    return requireTag
      ? { status: "FAIL", detail: `tag not found locally (--expect-tag requires it)` }
      : { status: "SKIP", detail: "no local tag for this version (candidate verification)" };
  }
  if (tagType !== "tag") return { status: "FAIL", detail: `tag exists but is ${tagType ?? "unknown"}, not an annotated tag object` };
  if (!expectedSha) return { status: "SKIP", detail: "no expected SHA supplied; cannot bind the tag to a product SHA" };
  if (tagCommit !== expectedSha) {
    return { status: "FAIL", detail: `tag dereferences to ${tagCommit}, not the expected product SHA ${expectedSha}` };
  }
  return { status: "PASS", detail: `annotated tag dereferences to the expected product SHA ${expectedSha}` };
}

/** Release channel derived from the version shape: "stable", "rc", or null
 *  for anything malformed/unsupported (build metadata, other prerelease
 *  flavors). The channel is never operator-selected; it is derived. */
export function classifyVersionChannel(version) {
  if (typeof version !== "string") return null;
  if (RC_VERSION_PATTERN.test(version)) return "rc";
  if (STABLE_VERSION_PATTERN.test(version)) return "stable";
  return null;
}

/** CLI invocation contract (pure, unit-testable): channel/version agreement,
 *  mandatory --stable-feed-version in RC mode, and no stray RC options in
 *  stable mode. Every issue is a fail-closed refusal to verify. */
export function evaluateRcInvocation({ version, rcMode, stableFeedVersion }) {
  const issues = [];
  if (rcMode) {
    if (version === undefined) issues.push("--rc requires --version");
    else if (!RC_VERSION_PATTERN.test(version)) {
      issues.push(`--rc requires an RC version shaped x.y.z-rc.N (e.g. 0.8.7-rc.1), got ${JSON.stringify(version)}`);
    }
    if (stableFeedVersion === undefined) {
      issues.push("--rc requires --stable-feed-version <x.y.z> naming the stable release the production feed must still serve");
    } else if (!STABLE_VERSION_PATTERN.test(stableFeedVersion)) {
      issues.push(`--stable-feed-version must be a stable x.y.z version, got ${JSON.stringify(stableFeedVersion)}`);
    }
  } else {
    if (version === undefined) {
      issues.push("--version <x.y.z> is required (see --help)");
    } else if (RC_VERSION_PATTERN.test(version)) {
      issues.push(`${JSON.stringify(version)} is an RC version; RC verification is opt-in — pass --rc (plus --stable-feed-version)`);
    } else if (!STABLE_VERSION_PATTERN.test(version)) {
      issues.push(`--version must be a stable x.y.z version (or x.y.z-rc.N with --rc), got ${JSON.stringify(version)}`);
    }
    if (stableFeedVersion !== undefined) issues.push("--stable-feed-version is only meaningful with --rc");
  }
  return issues;
}

/** R1 release-flag decision (pure, unit-testable). Stable mode rejects drafts
 *  and prereleases; RC mode requires exactly a published prerelease — the
 *  stable-feed isolation invariant (releases/latest excludes prereleases). */
export function evaluateReleaseFlags({ isDraft, isPrerelease }, { rcMode }) {
  if (rcMode) {
    if (isDraft === true) {
      return { status: "FAIL", detail: "the RC release is a draft — the feed-bearing RC must be a published prerelease", fix: "Publish the draft release or re-run release.yml; a draft RC must never carry the feed." };
    }
    if (isPrerelease !== true) {
      return { status: "FAIL", detail: "the RC release is a full release, not a prerelease — the stable-feed isolation invariant is broken", fix: "Re-create the release as a prerelease (release.yml does this atomically); never flip an RC to a full release." };
    }
    return { status: "PASS", detail: "published prerelease (isPrerelease=true, isDraft=false) — releases/latest can never serve it" };
  }
  if (isDraft || isPrerelease) {
    return {
      status: "FAIL",
      detail: `release is ${[isDraft ? "a draft" : null, isPrerelease ? "a prerelease" : null].filter(Boolean).join(" and ")}`,
    };
  }
  return { status: "PASS", detail: "published full release" };
}

/** RC-mode R8: stable-feed isolation (pure, unit-testable). The production
 *  `releases/latest` endpoint must redirect to — and serve bytes of — exactly
 *  the supplied stable release, never the RC. An empty issues list means the
 *  stable channel is provably untouched by the RC. */
export function evaluateStableFeedIsolation({ rcTag, stableVersion, redirectLocation, liveFeedText, stableReleaseFeedText }) {
  const issues = [];
  if (!redirectLocation) {
    issues.push("the stable endpoint did not redirect to a concrete release asset (unexpected response shape)");
  } else if (String(redirectLocation).includes(`/releases/download/${rcTag}/`)) {
    issues.push(`the stable endpoint redirects to the RC tag ${rcTag} — releases/latest is serving the RC`);
  }
  let liveFeed;
  try {
    liveFeed = JSON.parse(String(liveFeedText));
  } catch {
    issues.push("the live stable feed is not valid JSON");
  }
  if (liveFeed && typeof liveFeed === "object") {
    if (liveFeed.version !== stableVersion) {
      issues.push(`the live stable feed serves version ${JSON.stringify(liveFeed.version ?? "missing")}, expected the stable release ${stableVersion} — the stable channel has moved`);
    }
    const url = liveFeed.platforms?.["windows-x86_64"]?.url;
    if (typeof url === "string" && url.includes(`/releases/download/${rcTag}/`)) {
      issues.push(`the live stable feed points at an RC release asset (${rcTag})`);
    }
  }
  if (stableReleaseFeedText !== undefined && liveFeedText !== stableReleaseFeedText) {
    issues.push(`the live stable feed bytes differ from the v${stableVersion} release latest.json — feed and stable release have drifted`);
  }
  return issues;
}

/** G3 helper source: the verification method proven in the v0.8.4 delivery
 *  (minisign-verify 0.2.5, allow_legacy=false — Ed25519ph, the updater's own call). */
export const SIGCHECK_CARGO_TOML = `[package]
name = "sigcheck"
version = "0.1.0"
edition = "2021"

[dependencies]
minisign-verify = "=0.2.5"
`;

export const SIGCHECK_MAIN_RS = `use minisign_verify::{PublicKey, Signature};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: sigcheck <public-key-file> <data-file> <signature-file>");
        std::process::exit(3);
    }
    let pk = match PublicKey::from_file(&args[1]) {
        Ok(pk) => pk,
        Err(e) => { eprintln!("ERROR: public key: {e}"); std::process::exit(3); }
    };
    let data = match std::fs::read(&args[2]) {
        Ok(data) => data,
        Err(e) => { eprintln!("ERROR: data file: {e}"); std::process::exit(3); }
    };
    let sig = match Signature::from_file(&args[3]) {
        Ok(sig) => sig,
        Err(e) => { eprintln!("ERROR: signature file: {e}"); std::process::exit(3); }
    };
    println!("installer bytes: {}", data.len());
    match pk.verify(&data, &sig, false) {
        Ok(()) => println!("PASS minisign-verify 0.2.5, allow_legacy=false (Ed25519ph, the updater's own call)"),
        Err(e) => { eprintln!("FAIL: {e}"); std::process::exit(1); }
    }
}
`;

// ---------------------------------------------------------------------------
// Process helpers.
// ---------------------------------------------------------------------------

function run(command, args, { cwd = repoRoot, timeout = 120_000 } = {}) {
  const result = spawnSync(command, args, { cwd, encoding: "utf8", timeout, maxBuffer: 64 * 1024 * 1024, windowsHide: true });
  if (result.error) throw new Error(`${command} could not run: ${result.error.message}`);
  return { code: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

function git(args) {
  return run("git", args);
}

function gh(args) {
  return run("gh", args, { timeout: 60_000 });
}

async function fetchText(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`GET ${url} -> HTTP ${response.status}`);
  return response.text();
}

class Checks {
  constructor() {
    this.entries = [];
  }

  add(id, name, status, detail, fix) {
    this.entries.push({ id, name, status, detail: detail ?? "", ...(fix ? { fix } : {}) });
    return this.entries.at(-1);
  }
}

function fail(checks, id, name, detail, fix) {
  return checks.add(id, name, "FAIL", detail, fix);
}

// ---------------------------------------------------------------------------
// Main.
// ---------------------------------------------------------------------------

async function main() {
  const argv = process.argv.slice(2);
  if (argv.includes("--help") || argv.includes("-h")) {
    usage();
    process.exit(0);
  }

  const options = { sourceOnly: false, expectTag: false, verifySignature: false, allowClaude: false, json: false, rcMode: false };
  let version;
  let expectedShaInput;
  let metadataPath;
  let sigPath;
  let installerPath;
  let manifestPath;
  let stableFeedVersion;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const value = argv[i + 1];
    switch (arg) {
      case "--version": version = value; i += 1; break;
      case "--expected-sha": expectedShaInput = value; i += 1; break;
      case "--metadata": metadataPath = path.resolve(value); i += 1; break;
      case "--sig": sigPath = path.resolve(value); i += 1; break;
      case "--installer": installerPath = path.resolve(value); i += 1; break;
      case "--manifest": manifestPath = path.resolve(value); i += 1; break;
      case "--source-only": options.sourceOnly = true; break;
      case "--expect-tag": options.expectTag = true; break;
      case "--verify-signature": options.verifySignature = true; break;
      case "--allow-experimental-claude": options.allowClaude = true; break;
      case "--rc": options.rcMode = true; break;
      case "--stable-feed-version": stableFeedVersion = value; i += 1; break;
      case "--json": options.json = true; break;
      default:
        console.error(`verify-release-integrity: unknown option ${arg} (see --help)`);
        process.exit(2);
    }
  }
  // Channel/version contract: the RC channel is opt-in and derived from a
  // strict x.y.z-rc.N shape; malformed or ambiguous versions fail closed.
  const invocationIssues = evaluateRcInvocation({ version, rcMode: options.rcMode, stableFeedVersion });
  if (invocationIssues.length > 0) {
    for (const issue of invocationIssues) console.error(`verify-release-integrity: ${issue}`);
    process.exit(2);
  }
  options.stableFeedVersion = stableFeedVersion;

  const checks = new Checks();
  const tagName = `v${version}`;
  const installerName = `LimitScope_${version}_x64-setup.exe`;

  let tempDir;
  try {
    // ----- Resolve the source tree to verify -------------------------------
    let treeSha;
    let mode;
    if (expectedShaInput) {
      const probe = git(["cat-file", "-e", `${expectedShaInput}^{commit}`]);
      if (probe.code !== 0) {
        console.error(`verify-release-integrity: expected source SHA ${expectedShaInput} does not resolve to a commit (git: ${probe.stderr.trim()})`);
        process.exit(2);
      }
      treeSha = git(["rev-parse", `${expectedShaInput}^{commit}`]).stdout.trim();
      mode = "sha";
    } else {
      const tagProbe = git(["rev-parse", "--verify", "--quiet", `${tagName}^{commit}`]);
      if (tagProbe.code === 0) {
        treeSha = tagProbe.stdout.trim();
        mode = "tag";
      } else {
        treeSha = git(["rev-parse", "HEAD"]).stdout.trim();
        mode = "candidate";
      }
    }
    const tree = expectedShaInput ?? (mode === "tag" ? tagName : "HEAD");
    const readAtTree = (relPath) => {
      if (mode === "candidate") {
        const absolute = path.join(repoRoot, relPath);
        return existsSync(absolute) ? readFileSync(absolute, "utf8") : null;
      }
      const shown = git(["show", `${tree}:${relPath}`]);
      return shown.code === 0 ? shown.stdout : null;
    };

    // ----- S1: worktree cleanliness (candidate mode only) ------------------
    if (mode === "candidate") {
      const status = git(["status", "--porcelain"]);
      if (status.code !== 0) {
        fail(checks, "S1", "git worktree cleanliness", `git status failed: ${status.stderr.trim()}`);
      } else if (status.stdout.trim() === "") {
        checks.add("S1", "git worktree cleanliness", "PASS", "worktree is clean");
      } else {
        const entries = status.stdout.trim().split("\n");
        fail(
          checks,
          "S1",
          "git worktree cleanliness",
          `worktree is not clean (${entries.length} entr${entries.length === 1 ? "y" : "ies"}); a release candidate must be built from a clean tree`,
          "Commit, stash, or remove the local changes before tagging/dispatching a release.",
        );
      }
    } else {
      checks.add("S1", "git worktree cleanliness", "SKIP", `verifying the committed tree ${tree.slice(0, 12)}; current worktree state is not applicable`);
    }

    // ----- S2: source SHA exists -------------------------------------------
    checks.add("S2", "expected source SHA exists", "PASS", `verified tree: ${treeSha} (${mode} mode)`);

    // ----- S3: version surfaces --------------------------------------------
    const confText = readAtTree("src-tauri/tauri.conf.json");
    if (!confText) {
      console.error(`verify-release-integrity: cannot read src-tauri/tauri.conf.json at ${tree}`);
      process.exit(2);
    }
    const conf = JSON.parse(confText);
    const surfaceIssues = validateVersionSurfaces(
      {
        pkg: JSON.parse(readAtTree("package.json") ?? "null"),
        pkgLock: readAtTree("package-lock.json") ?? "",
        conf,
        cargoToml: readAtTree("src-tauri/Cargo.toml") ?? "",
        cargoLock: readAtTree("src-tauri/Cargo.lock") ?? "",
      },
      version,
    );
    if (surfaceIssues.length === 0) {
      checks.add("S3", "canonical version surfaces agree", "PASS", `all five surfaces (six version fields) + product/binary name at ${version}`);
    } else {
      fail(checks, "S3", "canonical version surfaces agree", surfaceIssues.join("; "), "Bump every version surface together (the release bump commit touches all five files).");
    }

    // ----- S4: tag identity -------------------------------------------------
    let tagCommit;
    let tagType;
    const tagObject = git(["rev-parse", "--verify", "--quiet", tagName]);
    const tagExists = tagObject.code === 0;
    if (tagExists) {
      tagType = git(["cat-file", "-t", tagName]).stdout.trim();
      tagCommit = git(["rev-parse", `${tagName}^{commit}`]).stdout.trim();
    }
    const expectedSha = expectedShaInput ?? (mode === "tag" ? treeSha : undefined);
    const tagResult = evaluateTagIdentity({
      tagExists,
      tagType,
      tagCommit,
      expectedSha: expectedSha ? git(["rev-parse", `${expectedSha}^{commit}`]).stdout.trim() : undefined,
      requireTag: options.expectTag,
    });
    checks.add("S4", "annotated tag dereferences to product SHA", tagResult.status, tagResult.detail,
      tagResult.status === "FAIL" ? "Create or move the annotated tag v<version> onto the release commit — never re-tag a published release." : undefined);

    // ----- S5: updater contract + trust root -------------------------------
    const contract = validateUpdaterContract(conf);
    if (contract.issues.length === 0) {
      checks.add("S5", "updater contract and production trust root", "PASS",
        `endpoint = production feed; identifier = ${APP_IDENTIFIER}; trust-root key id ${PRODUCTION_KEY_ID} (key id bytes ${contract.pubkey?.keyId})`);
    } else {
      fail(checks, "S5", "updater contract and production trust root", contract.issues.join("; "),
        "Restore the production updater contract (docs/updater-production.md); a trust-root change is a managed rotation, never an edit.");
    }

    // ----- S6: no dev URL in release config --------------------------------
    const devLeaks = findDevUrlLeaks(conf);
    if (devLeaks.length === 0) {
      checks.add("S6", "no dev URL in release configuration", "PASS", "CSP uses the production IPC custom protocol; no dev origin outside build.devUrl");
    } else {
      fail(checks, "S6", "no dev URL in release configuration",
        devLeaks.map((leak) => `${leak.path}: ${leak.reason}`).join("; "),
        "Production configuration must not reference the dev server (vite port 1420) outside build.devUrl.");
    }

    // ----- S7/S8: provider registration ------------------------------------
    const mainRs = readAtTree("src-tauri/src/main.rs");
    if (!mainRs) {
      console.error(`verify-release-integrity: cannot read src-tauri/src/main.rs at ${tree}`);
      process.exit(2);
    }
    const providers = extractProviderStatus(mainRs);
    if (providers.missingMods.length === 0 && providers.missingHandlers.length === 0) {
      checks.add("S7", "five canonical providers registered", "PASS", CANONICAL_PROVIDER_MODULES.join(", "));
    } else {
      fail(checks, "S7", "five canonical providers registered",
        [...providers.missingMods.length ? `missing mod declarations: ${providers.missingMods.join(", ")}` : [],
             ...providers.missingHandlers.length ? `missing registrations: ${providers.missingHandlers.join(", ")}` : []].join("; "),
        "Register exactly the five canonical providers (docs/release-runbook-v0.8.0.md registry order).");
    }
    if (providers.claudeRegistered && !options.allowClaude) {
      fail(checks, "S8", "experimental Claude registration absent",
        "main.rs registers Claude (mod declaration or command wiring)",
        "Claude is not a production provider; remove the registration or re-run with --allow-experimental-claude if explicitly enabled.");
    } else if (options.allowClaude) {
      checks.add("S8", "experimental Claude registration absent", "SKIP", "allowed by --allow-experimental-claude");
    } else {
      checks.add("S8", "experimental Claude registration absent", "PASS", "no Claude registration in main.rs");
    }

    if (options.sourceOnly) {
      return finish(checks, { version, tree: treeSha, mode: "source-only", channel: options.rcMode ? "rc" : "stable" }, options);
    }

    // ----- Release metadata -------------------------------------------------
    let feedText;
    let sigText;
    let releaseView;
    const offline = Boolean(metadataPath);

    if (offline) {
      if (!existsSync(metadataPath)) {
        console.error(`verify-release-integrity: metadata file not found: ${metadataPath}`);
        process.exit(2);
      }
      feedText = readFileSync(metadataPath, "utf8");
      checks.add("R1", "companion release exists", "SKIP", "offline metadata mode");
      checks.add("R2", "exact release asset set", "SKIP", "offline metadata mode");
      checks.add("R7", "CI run built from product SHA", "SKIP", "offline metadata mode");
      checks.add("R8", options.rcMode ? "live stable feed isolation" : "live feed serves this release", "SKIP", "offline metadata mode");
      if (sigPath) sigText = readFileSync(sigPath, "utf8");
    } else {
      if (gh(["--version"]).code !== 0) {
        console.error("verify-release-integrity: gh CLI is required for live release checks (https://cli.github.com)");
        process.exit(2);
      }
      const view = gh(["release", "view", tagName, "--repo", PUBLIC_FEED_REPO, "--json", "tagName,name,isDraft,isPrerelease,assets"]);
      if (view.code !== 0) {
        fail(checks, "R1", "companion release exists", `gh release view ${tagName} failed: ${view.stderr.trim()}`,
          options.rcMode
            ? "Publish the RC companion release via release.yml with an x.y.z-rc.N version (never by hand)."
            : "Publish the companion release via release.yml (never by hand).");
      } else {
        releaseView = JSON.parse(view.stdout);
        const latest = gh(["release", "view", "--repo", PUBLIC_FEED_REPO, "--json", "tagName"]);
        const latestTag = latest.code === 0 ? JSON.parse(latest.stdout).tagName : undefined;
        if (releaseView.tagName !== tagName) {
          fail(checks, "R1", "companion release exists", `companion tag is ${releaseView.tagName}, expected ${tagName}`);
        } else if (options.rcMode) {
          // RC mode: the release must BE a published prerelease — the
          // isolation invariant that keeps releases/latest on the stable feed.
          // An RC is never expected to be the live latest release, so the
          // stable-mode latest-release comparison does not apply.
          const flags = evaluateReleaseFlags(releaseView, { rcMode: true });
          if (flags.status === "FAIL") {
            fail(checks, "R1", "companion release exists", flags.detail, flags.fix);
          } else {
            checks.add("R1", "companion release exists", "PASS", `${tagName} is a published prerelease (RC channel)`);
          }
        } else if (releaseView.isDraft || releaseView.isPrerelease) {
          fail(checks, "R1", "companion release exists", `release ${tagName} is ${[releaseView.isDraft ? "a draft" : null, releaseView.isPrerelease ? "a prerelease" : null].filter(Boolean).join(" and ")}`);
        } else if (latestTag !== tagName) {
          checks.add("R1", "companion release exists", "PASS", `${tagName} published (live latest release is ${latestTag}; R8 will judge whether the live feed still serves this release)`);
        } else {
          checks.add("R1", "companion release exists", "PASS", `${tagName} is the live latest release`);
        }

        const assetNames = (releaseView.assets ?? []).map((asset) => asset.name);
        const assetIssues = validateAssetSet(assetNames, version);
        if (assetIssues.length === 0) {
          checks.add("R2", "exact release asset set", "PASS", assetNames.join(", "));
        } else {
          fail(checks, "R2", "exact release asset set", assetIssues.join("; "),
            "The companion release must carry exactly the installer, its .sig and latest.json (atomic publication contract).");
        }
      }

      feedText = await fetchText(`https://github.com/${PUBLIC_FEED_REPO}/releases/download/${tagName}/latest.json`)
        .catch((error) => {
          fail(checks, "R3", "latest.json version matches tag", `could not download the release latest.json asset: ${error.message}`);
          return undefined;
        });

      if (sigPath) {
        if (!existsSync(sigPath)) {
          console.error(`verify-release-integrity: --sig file not found: ${sigPath}`);
          process.exit(2);
        }
        sigText = readFileSync(sigPath, "utf8");
      } else {
        const sigUrl = `https://github.com/${PUBLIC_FEED_REPO}/releases/download/${tagName}/${installerName}.sig`;
        sigText = await fetchText(sigUrl).catch((error) => {
          fail(checks, "R5", "latest.json signature matches .sig asset", `could not download the published .sig asset: ${error.message}`);
          return undefined;
        });
      }

      // R7: a successful release.yml run must exist at the product SHA.
      const runs = gh(["run", "list", "--repo", SOURCE_REPO, "--workflow=release.yml", "--json", "headSha,conclusion", "--limit", "200"]);
      if (runs.code !== 0) {
        fail(checks, "R7", "CI run built from product SHA", `gh run list failed: ${runs.stderr.trim()}`);
      } else {
        const parsed = JSON.parse(runs.stdout);
        const productSha = git(["rev-parse", `${treeSha}^{commit}`]).stdout.trim();
        const matching = parsed.filter((candidate) => candidate.headSha === productSha);
        if (matching.some((candidate) => candidate.conclusion === "success")) {
          checks.add("R7", "CI run built from product SHA", "PASS", `a successful release.yml run exists at ${productSha.slice(0, 12)}`);
        } else {
          fail(checks, "R7", "CI run built from product SHA",
            matching.length > 0
              ? `release.yml run(s) exist at ${productSha.slice(0, 12)} but none concluded successfully`
              : `no release.yml run exists at ${productSha.slice(0, 12)} — the published artifacts may not be CI-built`,
            "Verify via Actions that the release workflow ran from the tagged commit; do not substitute local artifacts.");
        }
      }
    }

    // ----- Feed contract checks (shared by live and offline modes) ----------
    let feed;
    if (feedText !== undefined) {
      try {
        feed = JSON.parse(feedText);
      } catch (error) {
        fail(checks, "R3", "latest.json version matches tag", `latest.json is not valid JSON: ${error.message}`);
      }
      if (feed) {
        const feedIssues = validateFeed(feed, { version, installerName });
        if (feedIssues.length === 0) {
          checks.add("R3", "latest.json version matches tag", "PASS", `version ${version}`);
          checks.add("R4", "latest.json artifact URL matches release asset", "PASS", feed.platforms?.["windows-x86_64"]?.url);
        } else {
          if (feedIssues.some((issue) => issue.includes("version"))) {
            fail(checks, "R3", "latest.json version matches tag", feedIssues.filter((issue) => issue.includes("version")).join("; "),
              "Regenerate latest.json for the release (release.yml does this from the version input).");
          } else {
            checks.add("R3", "latest.json version matches tag", "PASS", `version ${version}`);
          }
          if (feedIssues.some((issue) => issue.includes("url") || issue.includes("platforms"))) {
            fail(checks, "R4", "latest.json artifact URL matches release asset", feedIssues.filter((issue) => !issue.includes("version")).join("; "),
              "The feed URL must be the canonical download URL of the published installer asset.");
          } else {
            checks.add("R4", "latest.json artifact URL matches release asset", "PASS", feed.platforms?.["windows-x86_64"]?.url);
          }
        }

        const privateFindings = scanFeedForPrivateUrls(feed);
        if (privateFindings.length === 0) {
          checks.add("R6", "no private/local URLs in updater metadata", "PASS", "only public feed URLs present");
        } else {
          fail(checks, "R6", "no private/local URLs in updater metadata",
            privateFindings.map((leak) => `${leak.path}: ${leak.finding}`).join("; "),
            "Remove private hosts, paths or credential shapes from the feed — it is served unauthenticated.");
        }
      }
    }

    // R5: feed signature vs .sig bytes (live download or --sig).
    if (sigText !== undefined && feed) {
      const embedded = feed?.platforms?.["windows-x86_64"]?.signature;
      if (!sigMatchesFeed(feed, sigText)) {
        fail(checks, "R5", "latest.json signature matches .sig asset",
          embedded ? "latest.json signature does not decode to the published .sig bytes" : "latest.json has no signature field",
          "The feed signature and the published .sig asset must be byte-identical (release.yml embeds the .sig into latest.json).");
      } else {
        checks.add("R5", "latest.json signature matches .sig asset", "PASS", "feed signature == published .sig bytes");
      }

      // G1: structural signature check against the trust root.
      if (contract.pubkey) {
        try {
          const parsed = parseSig(sigText);
          if (parsed.keyId === contract.pubkey.keyId) {
            checks.add("G1", "signature structure and trust-root key id", "PASS",
              `algorithm ${parsed.algorithm}, key id ${keyIdCommentForm(parsed.keyId)} (trust root)`);
          } else {
            fail(checks, "G1", "signature structure and trust-root key id",
              `signature key id ${keyIdCommentForm(parsed.keyId)} != trust root ${PRODUCTION_KEY_ID}`,
              "The signature was produced by a different key than the one embedded in the app.");
          }
        } catch (error) {
          fail(checks, "G1", "signature structure and trust-root key id", error.message,
            "The .sig asset must be a modern minisign (tauri signer) signature.");
        }
      }
    }

    // R8: live feed identity (anonymous endpoint). Stable mode proves the
    // live feed serves THIS release; RC mode proves the live stable feed is
    // still exactly the supplied stable release — i.e. the RC publication
    // cannot satisfy or displace the stable channel.
    if (!offline && options.rcMode) {
      try {
        const probe = await fetch(PRODUCTION_FEED_URL, { redirect: "manual" });
        const redirectLocation = probe.headers.get("location");
        const liveFeed = await fetchText(PRODUCTION_FEED_URL);
        const stableReleaseFeed = await fetchText(
          `https://github.com/${PUBLIC_FEED_REPO}/releases/download/v${options.stableFeedVersion}/latest.json`,
        );
        const isolationIssues = evaluateStableFeedIsolation({
          rcTag: tagName,
          stableVersion: options.stableFeedVersion,
          redirectLocation,
          liveFeedText: liveFeed,
          stableReleaseFeedText: stableReleaseFeed,
        });
        if (isolationIssues.length === 0) {
          checks.add("R8", "live stable feed isolation", "PASS",
            `${PRODUCTION_FEED_URL} still resolves to stable ${options.stableFeedVersion} (redirect ${redirectLocation}); the RC tag ${tagName} is not served`);
        } else {
          fail(checks, "R8", "live stable feed isolation", isolationIssues.join("; "),
            "The stable feed must keep resolving to the stable release; investigate the companion repository immediately.");
        }
      } catch (error) {
        fail(checks, "R8", "live stable feed isolation", `could not verify stable-feed isolation: ${error.message}`);
      }
    } else if (!offline) {
      try {
        const liveFeed = await fetchText(PRODUCTION_FEED_URL);
        if (feedText !== undefined && liveFeed === feedText) {
          checks.add("R8", "live feed serves this release", "PASS", `${PRODUCTION_FEED_URL} serves bytes identical to the ${tagName} feed`);
        } else if (feedText === undefined) {
          checks.add("R8", "live feed serves this release", "SKIP", "release feed unavailable; nothing to compare");
        } else {
          fail(checks, "R8", "live feed serves this release",
            `${PRODUCTION_FEED_URL} serves different bytes than the ${tagName} release feed — a newer release is likely live`,
            "Verify the current live release instead, or confirm this release was intentionally superseded.");
        }
      } catch (error) {
        fail(checks, "R8", "live feed serves this release", `could not fetch the live feed: ${error.message}`);
      }
    }

    // ----- Optional: manifest cross-check -----------------------------------
    if (manifestPath) {
      if (!existsSync(manifestPath)) {
        console.error(`verify-release-integrity: manifest file not found: ${manifestPath}`);
        process.exit(2);
      }
      const manifest = Object.fromEntries(
        readFileSync(manifestPath, "utf8").trim().split(/\r?\n/).map((line) => {
          const at = line.indexOf(":");
          return at === -1 ? [line.trim(), ""] : [line.slice(0, at).trim(), line.slice(at + 1).trim()];
        }),
      );
      const manifestIssues = [];
      if (manifest.installer !== installerName) manifestIssues.push(`manifest installer ${JSON.stringify(manifest.installer)} != ${installerName}`);
      if (manifest.source_sha && expectedSha && manifest.source_sha !== expectedSha) {
        manifestIssues.push(`manifest source_sha ${manifest.source_sha} != product SHA ${expectedSha}`);
      }
      if (manifestIssues.length === 0) {
        checks.add("M1", "release manifest matches product SHA", "PASS", `installer ${installerName}, source_sha bound`);
      } else {
        fail(checks, "M1", "release manifest matches product SHA", manifestIssues.join("; "));
      }
    }

    // ----- Optional: cryptographic signature verification --------------------
    if (options.verifySignature) {
      if (!sigText) {
        checks.add("G3", "minisign verification of installer", "SKIP", ".sig unavailable (download failed and no --sig given)");
      } else if (contract.issues.length > 0 || !contract.pubkey) {
        checks.add("G3", "minisign verification of installer", "SKIP", "trust root could not be established (see S5)");
      } else {
        tempDir = mkdtempSync(path.join(os.tmpdir(), "limitscope-verify-"));
        try {
          const pubkeyFile = path.join(tempDir, "trust-root.pub");
          writeFileSync(pubkeyFile, Buffer.from(conf.plugins.updater.pubkey, "base64"));
          // minisign-verify consumes the minisign text, not the Tauri .sig armor.
          const sigFile = path.join(tempDir, "release.minisign");
          writeFileSync(sigFile, minisignSigText(sigText));
          const dataFile = installerPath ?? path.join(tempDir, installerName);
          if (!installerPath) {
            const installerUrl = `https://github.com/${PUBLIC_FEED_REPO}/releases/download/${tagName}/${installerName}`;
            const response = await fetch(installerUrl);
            if (!response.ok) throw new Error(`GET ${installerUrl} -> HTTP ${response.status}`);
            writeFileSync(dataFile, Buffer.from(await response.arrayBuffer()));
          } else if (!existsSync(installerPath)) {
            throw new Error(`installer not found: ${installerPath}`);
          }

          if (run("cargo", ["--version"]).code !== 0) {
            checks.add("G3", "minisign verification of installer", "SKIP",
              "cargo is not installed; the installer was NOT cryptographically verified",
              "Install Rust (stable MSVC) to enable the minisign-verify 0.2.5 helper proven in v0.8.4 delivery.");
          } else {
            const result = runSigcheck(pubkeyFile, dataFile, sigFile);
            if (result.status === "PASS") {
              checks.add("G3", "minisign verification of installer", "PASS", result.detail);
            } else if (result.status === "SKIP") {
              checks.add("G3", "minisign verification of installer", "SKIP", result.detail);
            } else {
              fail(checks, "G3", "minisign verification of installer", result.detail,
                "The installer does not verify against the updater trust root — do not distribute; investigate immediately.");
            }
          }
        } catch (error) {
          checks.add("G3", "minisign verification of installer", "SKIP", `could not prepare verification: ${error.message}`);
        }
      }
    }

    return finish(checks, {
      version,
      tree: treeSha,
      mode: options.sourceOnly ? "source-only" : offline ? "offline-metadata" : "full",
      channel: options.rcMode ? "rc" : "stable",
    }, options);
  } finally {
    if (tempDir) rmSync(tempDir, { recursive: true, force: true });
  }
}

/** Run the ephemeral sigcheck helper (compiled once, cached in the OS temp dir). */
function runSigcheck(pubkeyFile, dataFile, sigFile) {
  const helperDir = path.join(os.tmpdir(), "limitscope-sigcheck", createHash("sha256").update(SIGCHECK_MAIN_RS + SIGCHECK_CARGO_TOML).digest("hex").slice(0, 16));
  mkdirSync(path.join(helperDir, "src"), { recursive: true });
  writeFileSync(path.join(helperDir, "Cargo.toml"), SIGCHECK_CARGO_TOML);
  writeFileSync(path.join(helperDir, "src", "main.rs"), SIGCHECK_MAIN_RS);
  const result = run("cargo", ["run", "--quiet", "--release", "--", pubkeyFile, dataFile, sigFile], { cwd: helperDir, timeout: 600_000 });
  if (result.code === 0 && /PASS/.test(result.stdout)) {
    return { status: "PASS", detail: result.stdout.trim().replace(/\n+/g, " | ") };
  }
  if (result.code === 1 && /FAIL/.test(result.stderr)) {
    return { status: "FAIL", detail: result.stderr.trim().replace(/\n+/g, " | ") };
  }
  return { status: "SKIP", detail: `sigcheck helper could not run (exit ${result.code}): ${(result.stderr || result.stdout).trim().slice(0, 300)}` };
}

function finish(checks, meta, options) {
  const failed = checks.entries.filter((entry) => entry.status === "FAIL");
  const summary = {
    ...meta,
    passed: checks.entries.filter((entry) => entry.status === "PASS").length,
    failed: failed.length,
    skipped: checks.entries.filter((entry) => entry.status === "SKIP").length,
    result: failed.length > 0 ? "FAIL" : "PASS",
  };
  if (options.json) {
    console.log(JSON.stringify({ ...summary, checks: checks.entries }, null, 2));
  } else {
    console.log(`verify-release-integrity: LimitScope ${meta.version} @ ${meta.tree.slice(0, 12)} (${meta.mode}${meta.channel === "rc" ? ", RC channel" : ""})`);
    for (const entry of checks.entries) {
      console.log(`  [${entry.status}] ${entry.id} ${entry.name}${entry.detail ? ` — ${entry.detail}` : ""}`);
      if (entry.status === "FAIL" && entry.fix) console.log(`         fix: ${entry.fix}`);
    }
    console.log(
      `verify-release-integrity: ${summary.result}` +
        ` (${summary.passed} passed, ${summary.failed} failed, ${summary.skipped} skipped)`,
    );
  }
  process.exit(failed.length > 0 ? 1 : 0);
}

// Only execute the CLI when this file is the entry point; the pure check
// helpers above are imported and unit-tested by verify-release-integrity.test.mjs.
const isEntrypoint = process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;
if (isEntrypoint) {
  main().catch((error) => {
    console.error(`verify-release-integrity: could not verify: ${error.message}`);
    process.exit(2);
  });
}
