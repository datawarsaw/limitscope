#!/usr/bin/env node
/**
 * Local updater test harness for LimitScope.
 *
 * Provides the primitives for a fully local, signed update loop without any
 * production hosting:
 *
 *   installed 0.5.90 (built with the overlay endpoint)
 *     -> serves latest.json advertising 0.5.91 from http://127.0.0.1:8397
 *     -> app "Check for updates" sees 0.5.91
 *     -> "Update now" downloads the signed NSIS installer
 *     -> minisign signature verifies against the pubkey in tauri.conf.json
 *     -> passive install, app restarts, reports 0.5.91
 *
 * Everything lives under `state/update-harness/` (gitignored). The harness
 * never touches the production feed URL and never prints private key
 * material or the key password.
 *
 * Usage:
 *   node scripts/update-harness.mjs <command> [options]
 *
 * Commands:
 *   keys                       Print the keypair paths (never contents).
 *   sign <file>                Sign a file; writes <file>.sig next to it.
 *   prepare --version V        Locate the built installer for version V,
 *                              sign it, and stage installer + latest.json
 *                              into the harness directory.
 *   metadata --version V       (Re)write latest.json only; --notes "text".
 *   serve [--port P]           Serve the harness directory (default :8397).
 *   overlay --version V        Write the build overlay that pins the app
 *                              version to V and points the updater at the
 *                              local feed; prints the tauri build command.
 *   check                      Verify the staged harness is complete.
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

function resolveKeyConfig() {
  const candidates = [
    {
      dir: path.join(os.homedir(), ".tauri", "limitscope-updater"),
      file: path.join(os.homedir(), ".tauri", "limitscope-updater", "limitscope.key"),
      pub: path.join(os.homedir(), ".tauri", "limitscope-updater", "limitscope.key.pub"),
      readme: path.join(os.homedir(), ".tauri", "limitscope-updater", "README.txt"),
    },
    {
      dir: path.join(os.homedir(), ".tauri", "rate-limits-updater"),
      file: path.join(os.homedir(), ".tauri", "rate-limits-updater", "rate-limits.key"),
      pub: path.join(os.homedir(), ".tauri", "rate-limits-updater", "rate-limits.key.pub"),
      readme: path.join(os.homedir(), ".tauri", "rate-limits-updater", "README.txt"),
    },
  ];
  for (const c of candidates) {
    if (existsSync(c.file)) return c;
  }
  return candidates[0];
}

const KEY_CFG = resolveKeyConfig();
const KEY_DIR = KEY_CFG.dir;
const KEY_FILE = KEY_CFG.file;

const args = process.argv.slice(2);
const command = args[0];

function flagValue(name) {
  const index = args.indexOf(name);
  return index === -1 ? undefined : args[index + 1];
}

function die(message) {
  console.error(`error: ${message}`);
  process.exit(1);
}

/** Resolves the key password without ever printing it. */
function keyPassword() {
  if (process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
    return process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
  }
  if (existsSync(KEY_CFG.readme)) {
    const lines = readFileSync(KEY_CFG.readme, "utf8").split(/\r?\n/);
    for (let i = 0; i < lines.length; i++) {
      if (lines[i].startsWith("Password:")) {
        const next = lines[i + 1]?.trim();
        if (next && !next.startsWith("-")) return next;
        const inline = lines[i].slice("Password:".length).trim();
        if (inline) return inline;
      }
    }
  }
  die(`no key password found; set TAURI_SIGNING_PRIVATE_KEY_PASSWORD or inspect ${KEY_CFG.readme}`);
}

function runTauri(cliArgs) {
  const result = spawnSync(process.execPath, [tauriCli, ...cliArgs], {
    stdio: ["ignore", "pipe", "pipe"],
    encoding: "utf8",
    cwd: repoRoot,
  });
  if (result.status !== 0) {
    process.stderr.write(result.stdout ?? "");
    process.stderr.write(result.stderr ?? "");
    die(`tauri ${cliArgs.join(" ")} failed`);
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
    die(
      `no NSIS installer for version ${version} under src-tauri/target/*/bundle/nsis — ` +
        "build one first (see docs/updater-production.md)",
    );
  }
  return hits[0];
}

function stageInstaller(version, notes) {
  const installer = findInstaller(version);
  mkdirSync(HARNESS_DIR, { recursive: true });
  const staged = path.join(HARNESS_DIR, path.basename(installer));
  copyFileSync(installer, staged);
  runTauri([
    "signer",
    "sign",
    "-f",
    KEY_FILE,
    "-p",
    keyPassword(),
    staged,
  ]);
  const sigFile = `${staged}.sig`;
  if (!existsSync(sigFile)) die(`signature was not written to ${sigFile}`);
  const signature = readFileSync(sigFile, "utf8").trim();
  const encoded = encodeURI(path.basename(installer));
  const feed = {
    version,
    pub_date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
    notes: notes ?? `Local harness build ${version}.`,
    platforms: {
      "windows-x86_64": {
        signature,
        url: `http://127.0.0.1:${flagValue("--port") ?? DEFAULT_PORT}/${encoded}`,
      },
    },
  };
  writeFileSync(path.join(HARNESS_DIR, "latest.json"), `${JSON.stringify(feed, null, 2)}\n`);
  console.log(`staged: ${path.basename(installer)} (+ .sig)`);
  console.log(`feed:   ${path.join(HARNESS_DIR, "latest.json")} -> version ${version}`);
}

function serve(port) {
  if (!existsSync(path.join(HARNESS_DIR, "latest.json"))) {
    die(`no feed staged yet — run: node scripts/update-harness.mjs prepare --version <V>`);
  }
  const server = createServer((request, response) => {
    const name = decodeURIComponent(new URL(request.url, "http://x").pathname.slice(1));
    const file = path.join(HARNESS_DIR, path.basename(name));
    if (!existsSync(file)) {
      console.log(`MISS  ${request.url}`);
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
    console.log(`SENT  ${request.url} (${body.length} bytes)`);
  });
  server.listen(port, "127.0.0.1", () => {
    console.log(`update feed: http://127.0.0.1:${port}/latest.json`);
    console.log("serve dir:", HARNESS_DIR);
    console.log("press Ctrl+C to stop");
  });
}

function writeOverlay(version, port) {
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
  const productName = flagValue("--product-name");
  const identifier = flagValue("--identifier");
  if (productName) overlay.productName = productName;
  if (identifier) overlay.identifier = identifier;

  const target = path.join(HARNESS_DIR, "tauri.overlay.json");
  writeFileSync(target, `${JSON.stringify(overlay, null, 2)}\n`);
  console.log(`wrote:  ${target}`);
  console.log("build with:");
  console.log(
    `  npm run tauri build -- --debug --config "state\\update-harness\\tauri.overlay.json"`,
  );
}

function check() {
  const missing = [];
  if (!existsSync(KEY_FILE)) missing.push(KEY_FILE);
  if (!existsSync(path.join(HARNESS_DIR, "latest.json"))) {
    missing.push("state/update-harness/latest.json");
  }
  if (missing.length > 0) {
    console.error("harness incomplete; missing:");
    for (const item of missing) console.error(`  ${item}`);
    process.exit(1);
  }
  const feed = JSON.parse(readFileSync(path.join(HARNESS_DIR, "latest.json"), "utf8"));
  console.log("harness OK:");
  console.log(`  version:   ${feed.version}`);
  console.log(`  asset url: ${feed.platforms["windows-x86_64"].url}`);
}

switch (command) {
  case "keys":
    console.log("key dir: ", KEY_DIR);
    console.log("key file:", KEY_FILE);
    console.log("exists:  ", existsSync(KEY_FILE));
    break;
  case "sign": {
    const target = args[1];
    if (!target) die("usage: node scripts/update-harness.mjs sign <file>");
    runTauri(["signer", "sign", "-f", KEY_FILE, "-p", keyPassword(), target]);
    console.log(`signed: ${target}.sig`);
    break;
  }
  case "prepare": {
    const version = flagValue("--version");
    if (!version) die("missing --version <V>");
    stageInstaller(version, flagValue("--notes"));
    break;
  }
  case "metadata": {
    const version = flagValue("--version");
    if (!version) die("missing --version <V>");
    stageInstaller(version, flagValue("--notes"));
    break;
  }
  case "serve":
    serve(Number(flagValue("--port") ?? DEFAULT_PORT));
    break;
  case "overlay": {
    const version = flagValue("--version");
    if (!version) die("missing --version <V>");
    writeOverlay(version, Number(flagValue("--port") ?? DEFAULT_PORT));
    break;
  }
  case "check":
    check();
    break;
  default:
    console.log("usage: node scripts/update-harness.mjs <keys|sign|prepare|serve|overlay|check>");
    process.exit(command ? 1 : 0);
}
