/**
 * Decides whether a CI run must re-run the full `npm run tauri build` NSIS
 * bundling verification, based on the paths changed by the run.
 *
 * Why this exists: the ordinary CI path used to bundle an unsigned NSIS
 * installer on every pull request and every push to `main`, even though the
 * installer is a build artifact nobody consumes from ordinary CI (releases are
 * built, signed and published by .github/workflows/release.yml). Bundling is
 * the slowest CI stage (a full release-profile LTO Rust build plus the NSIS
 * package). It now runs only when a bundling-specific input changes, while
 * every ordinary gate (`npm ci`, `cargo test --locked`, `npm test`,
 * `npm run build`, `npm run scan:secrets`) keeps running unconditionally.
 *
 * The effective bundle inputs are the Tauri NSIS inputs: the bundler config
 * (`tauri.conf.json`), the NSIS script inputs it points at (`installerHooks`,
 * `customLanguageFiles`), the Rust crate inputs that produce the bundled
 * binary, and the bundling toolchain. The Tauri CLI downloads NSIS at build
 * time, so no vendored NSIS sources exist in this repository; the file set is
 * asserted against the tracked tree by scripts/ci-bundling-gate.test.mjs so it
 * fails loudly if bundling inputs move or a new one appears.
 *
 * Usage:
 *   node scripts/ci-bundling-gate.mjs --paths-file <file>
 *   git diff --name-only <base>...HEAD | node scripts/ci-bundling-gate.mjs
 *   node scripts/ci-bundling-gate.mjs --paths <path> [<path> ...]
 *
 * Prints `true` (run bundling verification) or `false` (skip it) to stdout.
 *
 * No shebang: this module is imported by scripts/ci-bundling-gate.test.mjs. A
 * `#!` first line ended by CRLF (the Windows runner's default checkout) breaks
 * the Vite ESM transform with "SyntaxError: Invalid or unexpected token", so
 * the module stays shebang-free like scripts/verify-release-integrity.mjs. It is
 * invoked as `node scripts/ci-bundling-gate.mjs`.
 */

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** Exact files whose change selects the native bundle (NSIS installer). */
export const BUNDLING_INPUTS = [
  "src-tauri/tauri.conf.json",
  "src-tauri/installer-hooks.nsh",
  "src-tauri/installer/English-custom.nsh",
  "src-tauri/Cargo.toml",
  "src-tauri/Cargo.lock",
  "src-tauri/build.rs",
  "package.json",
  "package-lock.json",
];

/** Directory prefixes whose change selects the native bundle. */
export const BUNDLING_INPUT_PREFIXES = [
  "src-tauri/src/",
  "src-tauri/icons/",
  "src-tauri/capabilities/",
  ".github/workflows/ci.yml",
];

/** Normalize a repo-relative path to forward slashes, without a leading `./`. */
export function normalizePath(raw) {
  return raw
    .trim()
    .replace(/\\/g, "/")
    .replace(/^\.\//, "");
}

/** True when a changed path is a bundling-specific input. */
export function isBundlingInput(raw) {
  if (!raw) return false;
  const rel = normalizePath(raw);
  if (rel === "") return false;
  if (BUNDLING_INPUTS.includes(rel)) return true;
  return BUNDLING_INPUT_PREFIXES.some(
    (prefix) => rel === prefix || rel.startsWith(prefix),
  );
}

/** True when any changed path is a bundling-specific input. */
export function anyBundlingInput(rawPaths) {
  return rawPaths.some((raw) => isBundlingInput(raw));
}

/** Split newline-separated `git diff --name-only` output into paths. */
export function parsePaths(text) {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/**
 * Assert that every configured bundling input exists in the tracked tree. The
 * test suite runs this against the working tree; a rename that is not mirrored
 * here would otherwise silently stop triggering bundling verification. Paths
 * are existence-probed with a fresh read so the check is meaningful on disk.
 */
export function missingBundlingInputs(root = repoRoot) {
  const missing = [];
  for (const rel of BUNDLING_INPUTS) {
    try {
      readFileSync(path.join(root, rel));
    } catch {
      missing.push(rel);
    }
  }
  return missing;
}

function readStdin() {
  try {
    return readFileSync(0, "utf8");
  } catch {
    return "";
  }
}

function collectPaths(argv) {
  const pathsFileIndex = argv.indexOf("--paths-file");
  if (pathsFileIndex !== -1) {
    const file = argv[pathsFileIndex + 1];
    if (!file) throw new Error("--paths-file requires a path argument");
    return parsePaths(readFileSync(file, "utf8"));
  }
  const pathsIndex = argv.indexOf("--paths");
  if (pathsIndex !== -1) {
    return argv.slice(pathsIndex + 1).flatMap(parsePaths);
  }
  return parsePaths(readStdin());
}

function main() {
  const changed = collectPaths(process.argv.slice(2));
  const runBundling = anyBundlingInput(changed);
  console.log(runBundling ? "true" : "false");
}

// Only run the CLI when invoked directly; the pure helpers above are imported
// and unit-tested by ci-bundling-gate.test.mjs.
const isEntrypoint =
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;
if (isEntrypoint) {
  try {
    main();
  } catch (error) {
    console.error(`ci-bundling-gate: ${error.message}`);
    process.exit(2);
  }
}
