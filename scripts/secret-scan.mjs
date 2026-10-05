#!/usr/bin/env node
/**
 * Secret scan over everything the app ships or builds:
 *
 *   - dist/ (built frontend)
 *   - src-tauri/tauri.conf.json (updater configuration)
 *   - src-tauri/target/<profile>/bundle (NSIS installers, updater signatures)
 *
 * It looks for credential shapes (GitHub PATs, PEM headers, the Tauri
 * minisign private-key envelope) and for the actual local signing key
 * contents. Reports file + offset only — never prints the matched content.
 *
 * Usage: node scripts/secret-scan.mjs [--target <path>]... (default: all)
 */

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const argv = process.argv.slice(2);
const explicitTargets = [];
for (let i = 0; i < argv.length; i += 2) {
  if (argv[i] === "--target") explicitTargets.push(path.resolve(argv[i + 1]));
}

function collectTargets() {
  if (explicitTargets.length > 0) return explicitTargets;
  const targets = [path.join(repoRoot, "dist"), path.join(repoRoot, "src-tauri", "tauri.conf.json")];
  for (const profile of ["release", "debug"]) {
    const bundle = path.join(repoRoot, "src-tauri", "target", profile, "bundle");
    if (existsSync(bundle)) targets.push(bundle);
  }
  return targets.filter((target) => existsSync(target));
}

const PATTERNS = [
  ["classic GitHub PAT", /ghp_[A-Za-z0-9]{20,}/],
  ["fine-grained GitHub PAT", /github_pat_[A-Za-z0-9_]{20,}/],
  ["OAuth token shape", /gho_[A-Za-z0-9]{20,}/],
  ["PEM private key header", /-----BEGIN [A-Z ]*PRIVATE KEY-----/],
  ["Tauri minisign private-key envelope", /untrusted comment: rsign encrypted secret key/],
];

/** The real private key bytes from external key locations: strongest possible check. */
function privateKeyNeedles() {
  const needles = [];
  const candidateFiles = [
    path.join(os.homedir(), ".tauri", "limitscope-updater", "limitscope.key"),
    path.join(os.homedir(), ".tauri", "rate-limits-updater", "rate-limits.key"),
  ];
  for (const file of candidateFiles) {
    if (existsSync(file)) {
      try {
        needles.push(readFileSync(file));
      } catch {}
    }
  }
  return needles;
}

function* walkFiles(dir) {
  if (statSync(dir).isFile()) {
    yield dir;
    return;
  }
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    if (statSync(full).isDirectory()) yield* walkFiles(full);
    else yield full;
  }
}

function scanFile(file, needles) {
  const findings = [];
  let buffer;
  try {
    buffer = readFileSync(file);
  } catch {
    return findings;
  }
  const text = buffer.toString("latin1");
  for (const [name, pattern] of PATTERNS) {
    const at = text.search(pattern);
    if (at !== -1) findings.push({ name, at });
  }
  for (const needle of needles) {
    if (buffer.includes(needle)) {
      findings.push({ name: "signing private key contents", at: 0 });
    }
  }
  return findings;
}

const targets = collectTargets();
if (targets.length === 0) {
  console.log("secret-scan: no build outputs found to scan (dist/, bundle/) — nothing to do");
  process.exit(0);
}

const needles = privateKeyNeedles();
let scanned = 0;
const hits = [];
for (const target of targets) {
  for (const file of walkFiles(target)) {
    scanned += 1;
    for (const finding of scanFile(file, needles)) {
      hits.push(`${finding.name} @ ${file} (offset ${finding.at})`);
    }
  }
}

console.log(`secret-scan: scanned ${scanned} file(s) under:`);
for (const target of targets) console.log(`  ${target}`);

if (hits.length > 0) {
  console.error("secret-scan: FAIL");
  for (const hit of hits) console.error(`  ${hit}`);
  process.exit(1);
}
console.log("secret-scan: PASS (no credential-shaped material found)");
