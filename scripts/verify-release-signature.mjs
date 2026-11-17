#!/usr/bin/env node
/**
 * Fail-closed verification of the updater signature against the trust root
 * embedded in this source tree — CLI entry point.
 *
 * The release workflow calls this file without arguments. Paths then come from
 * INSTALLER_PATH and SIG_PATH. Optional long-form arguments exist for local
 * verification and tests. Every missing, malformed, legacy, mismatched, or
 * unverifiable input exits nonzero; there is no SKIP result.
 *
 * The implementation lives in ./verify-release-signature-lib.mjs, which must
 * stay shebang-free: vite's transform pipeline hoists imports above a leading
 * hashbang, producing an invalid token mid-module, so vitest cannot import a
 * shebang-bearing module.
 */

import { pathToFileURL } from "node:url";

import { main } from "./verify-release-signature-lib.mjs";

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await main();
}
