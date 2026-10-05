import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Static guard for .github/workflows/release.yml (v0.8 pre-RC hardening).
 *
 * The release workflow is release tooling, so it has no runtime tests; these
 * deterministic assertions pin the invariants the v0.8 release depends on:
 * tag/ref/HEAD identity, version-surface agreement, exact artifact
 * selection, SHA-256/size traceability, atomic publication ordering
 * (latest.json LAST), and the updater key/endpoint contract. They also pin
 * the derived RC channel contract: the channel comes from the version shape
 * (no operator toggle), the RC companion release is created with --prerelease
 * from the initial creation call and asserted before any asset upload, and
 * the stable publication path is untouched (docs/updater-production.md
 * "Public RC channel").
 */

const ROOT = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const yml = readFileSync(join(ROOT, ".github", "workflows", "release.yml"), "utf8");

const stepNames = [...yml.matchAll(/- name: (.+)/g)].map((m) => m[1].trim());

function stepIndex(fragment: string): number {
  const index = stepNames.findIndex((name) =>
    name.toLowerCase().includes(fragment.toLowerCase()),
  );
  expect(index, `expected a workflow step matching "${fragment}"`).toBeGreaterThanOrEqual(0);
  return index;
}

describe("release workflow hardening", () => {
  it("requires the exact expected HEAD SHA and tag/ref/HEAD identity", () => {
    expect(yml).toMatch(/expected_head_sha:\s*\n\s*description: [^\n]+\n\s*required: true/);
    expect(stepIndex("verify tag/ref/HEAD identity")).toBeGreaterThanOrEqual(0);
    expect(yml).toContain("git rev-parse HEAD");
    expect(yml).toContain('[ "$ACTUAL_SHA" != "$GITHUB_SHA" ]');
    expect(yml).toContain('[ "$ACTUAL_SHA" != "$EXPECTED_HEAD_SHA" ]');
    expect(yml).toContain('git rev-parse "v$VERSION^{commit}"');
    expect(yml).toContain('[ "$TAG_SHA" != "$ACTUAL_SHA" ]');
  });

  it("verifies every version surface agrees before building", () => {
    expect(stepIndex("verify all version surfaces agree")).toBeGreaterThan(
      stepIndex("checkout repository"),
    );
    expect(yml).toContain("require('./src-tauri/tauri.conf.json').version");
    expect(yml).toContain("require('./package.json').version");
    expect(yml).toContain("src-tauri/Cargo.toml");
    expect(yml).toContain('name = "rate-limits"');
    expect(yml).toContain("src-tauri/Cargo.lock");
    // The installer-name contract pins the product identity.
    expect(yml).toContain("mainBinaryName");
    expect(yml).toContain('!= "LimitScope"');
    // The single-surface check it replaces must be gone.
    expect(yml).not.toContain("Verify version matches tauri.conf.json");
  });

  it("selects the exact installer/signature instead of glob or substring matching", () => {
    expect(yml).toContain("LimitScope_${VERSION}_x64-setup.exe");
    expect(yml).toContain("expected exactly one installer");
    // The bundle directory is scanned only to count/verify, never to select.
    expect(yml).toMatch(/-maxdepth 1 -type f -name '\*_x64-setup\.exe'/);
    expect(yml).not.toMatch(/ls\s+\S*bundle\/nsis\/\*_x64-setup\.exe/);
    expect(yml).not.toContain('f.includes(version)');
    // Publication and feed steps address the resolved exact paths.
    expect(yml).toContain('"$INSTALLER_PATH"');
    expect(yml).toContain('"$SIG_PATH"');
    expect(stepIndex("resolve exact release artifacts")).toBeGreaterThan(
      stepIndex("tauri build"),
    );
  });

  it("records SHA-256 and byte sizes for the release artifacts", () => {
    expect(stepIndex("record artifact SHA-256 and sizes")).toBeGreaterThan(
      stepIndex("resolve exact release artifacts"),
    );
    expect(yml).toContain('sha256sum "$INSTALLER_PATH"');
    expect(yml).toContain('sha256sum "$SIG_PATH"');
    expect(yml).toContain("release-artifacts-manifest.txt");
    expect(yml).toContain("GITHUB_STEP_SUMMARY");
    // The manifest travels with the run artifact for traceability.
    const uploadIndex = yml.indexOf("limitscope-signed-release");
    const manifestIndex = yml.lastIndexOf("release-artifacts-manifest.txt");
    expect(uploadIndex).toBeGreaterThan(0);
    expect(manifestIndex).toBeGreaterThan(uploadIndex);
  });

  it("keeps atomic publication ordering: latest.json goes last", () => {
    const publish = stepIndex("publish installer & signature");
    const scan = stepIndex("scan build outputs for secrets");
    const feed = stepIndex("generate update feed metadata");
    const uploadFeed = stepIndex("publish update feed (latest.json) LAST");
    const verifyFinal = stepIndex("verify final asset set and published feed");
    expect(scan).toBeGreaterThan(stepIndex("record artifact SHA-256 and sizes"));
    expect(feed).toBeGreaterThan(scan);
    expect(publish).toBeGreaterThan(feed);
    expect(uploadFeed).toBeGreaterThan(publish);
    expect(verifyFinal).toBeGreaterThan(uploadFeed);
    expect(yml.indexOf("gh release create")).toBeLessThan(yml.indexOf("gh release upload"));
    // latest.json is uploaded with --clobber in its own step, after the
    // exact-asset verification of the installer and signature.
    expect(yml).toMatch(/--clobber \\\r?\n\s+latest\.json/);
  });

  it("preserves the updater key, endpoint, and feed contract", () => {
    expect(yml).toContain("PUBLIC_FEED_REPO: datawarsaw/limitscope-releases");
    expect(yml).toContain('"windows-x86_64"');
    expect(yml).toContain('releases/download/v" + version + "/" + asset');
    expect(yml).toContain("${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}");
    expect(yml).toContain("${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}");
    expect(yml).toContain("${{ secrets.RELEASES_PAT }}");
    expect(yml).toContain('npx tauri signer sign --app-version "$VERSION"');
    // The feed carries only the Tauri-required metadata: no hashes or extra
    // platforms are smuggled into latest.json. (The SHA-256 manifest is
    // run-local traceability and must stay out of the published feed.)
    const feedScript = yml.slice(
      yml.indexOf("const feed = {"),
      yml.indexOf("fs.writeFileSync"),
    );
    expect(feedScript).not.toMatch(/sha256/i);
  });

  it("derives the RC channel from the version shape and fails closed on malformed versions", () => {
    const derive = stepIndex("derive release channel from version");
    expect(derive).toBeGreaterThan(stepIndex("checkout repository"));
    expect(derive).toBeLessThan(stepIndex("verify tag/ref/HEAD identity"));
    // Strict matchers only: x.y.z-rc.N -> RC, x.y.z -> stable, else fail.
    expect(yml).toContain('if [[ "$VERSION" =~ ^[0-9]+\\.[0-9]+\\.[0-9]+-rc\\.[0-9]+$ ]]; then');
    expect(yml).toContain('elif [[ "$VERSION" =~ ^[0-9]+\\.[0-9]+\\.[0-9]+$ ]]; then');
    expect(yml).toContain('echo "RELEASE_CHANNEL=rc" >> "$GITHUB_ENV"');
    expect(yml).toContain('echo "RELEASE_CHANNEL=stable" >> "$GITHUB_ENV"');
    expect(yml).toContain("is neither a stable x.y.z nor an RC x.y.z-rc.N version");
    expect(yml).not.toMatch(/prerelease[^\n]*[=:][^\n]*input/i); // no operator prerelease toggle
  });

  it("creates the RC as a prerelease atomically and asserts state before any asset upload", () => {
    const publishStep = yml.slice(yml.indexOf("Step 1 & 2"));
    const create = publishStep.indexOf("gh release create");
    const prerelease = publishStep.indexOf("--prerelease");
    const assertBeforeUpload = publishStep.indexOf("Asserting prerelease state before uploading any assets");
    const pin = publishStep.indexOf("gh api -X PATCH");
    const upload = publishStep.indexOf("gh release upload");
    expect(create).toBeGreaterThan(0);
    expect(prerelease).toBeGreaterThan(create);
    expect(prerelease).toBeLessThan(assertBeforeUpload);
    expect(assertBeforeUpload).toBeLessThan(pin);
    expect(pin).toBeLessThan(upload);
    // The assert helper rejects anything that is not a published prerelease.
    expect(yml).toContain("state.isPrerelease !== true || state.isDraft !== false");
    expect(yml.match(/assert_rc_prerelease_state\b/g)).toHaveLength(3); // definition + 2 calls
    // Second isolation lock: make_latest=false (drafts/prereleases cannot be latest).
    expect(yml).toContain("-f make_latest=false");
  });

  it("keeps the stable publication path unchanged and the RC feed release-scoped", () => {
    // The stable branch still creates the release WITH its two assets in one
    // atomic call, exactly as before the RC mode existed.
    const stableBranch = yml.slice(yml.indexOf("Stable channel: creating release"));
    expect(stableBranch).toContain("gh release create \"v$VERSION\"");
    expect(stableBranch).toContain('"$INSTALLER_PATH"');
    expect(stableBranch).toContain('"$SIG_PATH"');
    expect(stableBranch).not.toContain("--prerelease");
    // Publication ordering is channel-neutral: the RC's latest.json is an
    // asset of the RC release only (no global RC feed pointer exists).
    const step = yml.slice(yml.indexOf("Step 1 & 2"), yml.indexOf("Step 3 - Publish update feed"));
    expect(step).not.toContain("releases/latest");
    expect(yml).toContain("scripts/update-harness.mjs");
  });
});
