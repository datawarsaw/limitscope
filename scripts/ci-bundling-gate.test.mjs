// Unit tests for scripts/ci-bundling-gate.mjs and the CI wiring in
// .github/workflows/ci.yml.
//
// Two layers, both offline and deterministic:
//  1. the pure changed-path decision (which inputs select the NSIS bundle),
//     including a drift guard that every configured input still exists, and
//  2. the workflow wiring that turns `true`/`false` into a conditional job --
//     ordinary gates unconditional, the NSIS build gated, and changed-path
//     detection present for both pull requests and pushes to `main`.

import { existsSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  BUNDLING_INPUTS,
  BUNDLING_INPUT_PREFIXES,
  anyBundlingInput,
  isBundlingInput,
  missingBundlingInputs,
  normalizePath,
  parsePaths,
} from "./ci-bundling-gate.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const workflow = readFileSync(
  path.join(ROOT, ".github", "workflows", "ci.yml"),
  "utf8",
);

/** Split the workflow into top-level jobs keyed by their 2-space-indented id. */
function jobs(yml) {
  const part = yml.slice(yml.indexOf("\njobs:") + 1);
  const result = {};
  let current = null;
  for (const line of part.split(/\r?\n/)) {
    const key = /^  ([A-Za-z0-9_-]+):$/.exec(line);
    if (key) {
      current = key[1];
      result[current] = [];
    } else if (current) {
      result[current].push(line);
    }
  }
  return Object.fromEntries(
    Object.entries(result).map(([name, lines]) => [name, lines.join("\n")]),
  );
}

const ciJobs = jobs(workflow);

describe("bundling-input detection", () => {
  it("selects every exact bundling input", () => {
    for (const rel of BUNDLING_INPUTS) {
      expect(isBundlingInput(rel), `${rel} should select bundling`).toBe(true);
    }
  });

  it("selects prefix-owned bundling inputs", () => {
    const byPrefix = {
      "src-tauri/src/": "src-tauri/src/main.rs",
      "src-tauri/icons/": "src-tauri/icons/icon.ico",
      "src-tauri/capabilities/": "src-tauri/capabilities/default.json",
      ".github/workflows/ci.yml": ".github/workflows/ci.yml",
    };
    for (const prefix of BUNDLING_INPUT_PREFIXES) {
      const sample = byPrefix[prefix];
      expect(sample, `no sample exercises prefix ${prefix}`).toBeDefined();
      expect(isBundlingInput(sample), `${sample} should select bundling`).toBe(true);
    }
  });

  it("leaves non-bundling changes to the ordinary gates", () => {
    const unrelated = [
      "src/App.tsx",
      "src/lib/updater.ts",
      "docs/ci-and-release.md",
      "README.md",
      "scripts/secret-scan.mjs",
      ".github/workflows/release.yml",
      "src-tauri/.gitignore",
      "vite.config.ts",
    ];
    for (const rel of unrelated) {
      expect(isBundlingInput(rel), `${rel} should NOT select bundling`).toBe(false);
    }
  });

  it("normalizes Windows separators and ./ prefixes", () => {
    expect(normalizePath("./src/App.tsx")).toBe("src/App.tsx");
    expect(normalizePath("src-tauri\\src\\main.rs")).toBe("src-tauri/src/main.rs");
    expect(isBundlingInput("src-tauri\\Cargo.toml")).toBe(true);
    expect(isBundlingInput(".\\src-tauri\\tauri.conf.json")).toBe(true);
    expect(isBundlingInput("./src/App.tsx")).toBe(false);
  });

  it("aggregates a changed-path list and tolerates an empty one", () => {
    expect(anyBundlingInput(["src/App.tsx", "README.md"])).toBe(false);
    expect(anyBundlingInput(["src/App.tsx", "src-tauri/tauri.conf.json"])).toBe(true);
    expect(anyBundlingInput([])).toBe(false);
    expect(isBundlingInput("")).toBe(false);
  });

  it("parses git diff name-only output into clean paths", () => {
    expect(parsePaths("src/App.tsx\r\nsrc-tauri/Cargo.toml\n\n  \n")).toEqual([
      "src/App.tsx",
      "src-tauri/Cargo.toml",
    ]);
    expect(parsePaths("")).toEqual([]);
  });

  it("drift guard: every configured input still exists in the tree", () => {
    // A rename not mirrored in BUNDLING_INPUTS would silently stop triggering
    // the bundling verification, so the configuration is pinned to the tree.
    expect(missingBundlingInputs(ROOT)).toEqual([]);
    for (const prefix of BUNDLING_INPUT_PREFIXES) {
      const abs = path.join(ROOT, prefix);
      expect(existsSync(abs), `missing prefix ${prefix}`).toBe(true);
      if (prefix.endsWith("/")) {
        expect(statSync(abs).isDirectory(), `${prefix} should be a directory`).toBe(true);
      }
    }
  });
});

describe("CI wiring", () => {
  it("keeps every ordinary gate unconditional", () => {
    const windows = ciJobs.windows;
    expect(windows).toBeDefined();
    expect(windows).toContain("npm ci");
    expect(windows).toContain("cargo test --locked");
    expect(windows).toContain("npm test");
    expect(windows).toContain("npm run build");
    expect(windows).toContain("npm run scan:secrets");
    // No job-level `if:` on the always-on job: it must always report.
    expect(windows).not.toMatch(/^ {4}if:/m);
  });

  it("does not bundle on the ordinary path", () => {
    expect(ciJobs.windows).not.toContain("tauri build");
    // The bundling command runs exactly once (the header comment mentions it).
    expect(workflow.match(/^ {8}run: npm run tauri build -- -- --locked$/gm)).toHaveLength(1);
    expect(ciJobs["bundling-verification"]).toContain("npm run tauri build");
  });

  it("runs the bundling verification only when the detector says so", () => {
    const bundling = ciJobs["bundling-verification"];
    expect(bundling).toContain("needs: bundling-inputs");
    expect(bundling).toContain(
      "if: needs.bundling-inputs.outputs.run_bundling == 'true'",
    );
    // Job-level `if:` only: a step-level skip would leave the required check
    // pending, while a skipped job reports success.
    expect(bundling.match(/^ {4}if:/gm)).toHaveLength(1);
  });

  it("detects changed paths for both pull requests and pushes to main", () => {
    const inputs = ciJobs["bundling-inputs"];
    expect(inputs).toContain("scripts/ci-bundling-gate.mjs");
    expect(inputs).toContain("github.base_ref"); // pull_request base
    expect(inputs).toContain("github.event.before"); // push previous tip
    expect(inputs).toContain("fetch-depth: 0");
    // The provider is shared, so the gate cannot apply to pull requests only.
    expect(inputs.match(/ci-bundling-gate\.mjs/g)).toHaveLength(1);
    // The workflow is not skipped wholesale by path filtering (which would
    // leave any required check pending); gating happens per job instead.
    const onBlock = workflow.slice(workflow.indexOf("on:"), workflow.indexOf("concurrency:"));
    expect(onBlock).not.toMatch(/^ {4}paths(-ignore)?:/m);
  });

  it("keeps installer upload failure loud", () => {
    expect(workflow).toContain("if-no-files-found: error");
    expect(ciJobs["bundling-verification"]).toContain("upload-artifact");
  });

  it("leaves release publication out of CI", () => {
    expect(workflow).not.toContain("gh release");
    expect(workflow).not.toContain("TAURI_SIGNING_PRIVATE_KEY");
  });
});

describe("gate module shape", () => {
  it("stays shebang-free so a CRLF checkout can import it", () => {
    // This test file imports the gate module, and a `#!` first line ended by
    // CRLF (the Windows CI runner's default checkout) breaks the Vite ESM
    // transform with "SyntaxError: Invalid or unexpected token". The module is
    // only ever run as `node scripts/ci-bundling-gate.mjs`, so the shebang buys
    // nothing; verify-release-integrity.mjs, which is also imported by a test,
    // is shebang-free for the same reason.
    const source = readFileSync(
      path.join(ROOT, "scripts", "ci-bundling-gate.mjs"),
      "utf8",
    );
    expect(source.startsWith("#!")).toBe(false);
  });
});
