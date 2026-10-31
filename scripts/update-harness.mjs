#!/usr/bin/env node
/**
 * Local updater test harness for LimitScope — CLI entry point.
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
 * never touches the production feed URL. Secrets never reach output: the
 * signing password is read from the TAURI_SIGNING_PRIVATE_KEY_PASSWORD
 * environment variable only (there is no on-disk fallback), it is passed to
 * the Tauri CLI through the child environment instead of a command-line
 * argument, and every error and diagnostic line is redacted for the
 * password, the private-key path and the key directory. Diagnostics never
 * print the private-key path; the `keys` command prints the public-key
 * location instead.
 *
 * Usage:
 *   node scripts/update-harness.mjs <command> [options]
 *
 * Commands:
 *   keys                       Print the public-key path and key presence
 *                              (never the private-key path).
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
 *
 * The implementation lives in ./update-harness-lib.mjs, which must stay
 * shebang-free: vite's transform pipeline hoists imports above a leading
 * hashbang, producing an invalid token mid-module, so vitest cannot import
 * a shebang-bearing module (this also breaks the pre-existing
 * verify-release-signature.test.mjs collection).
 */

import { pathToFileURL } from "node:url";

import { main } from "./update-harness-lib.mjs";

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await main(process.argv.slice(2), process.env);
}
