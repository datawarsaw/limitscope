import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const release = readFileSync(new URL("../.github/workflows/release.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
const ci = readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
const validation = release.split("  validation:\n")[1].split("\n  windows:\n")[0];
const signing = release.split("\n  windows:\n")[1];

test("failed validation blocks the independent signing job", () => {
  assert.match(signing, /\n    needs: validation\n/);
  assert.doesNotMatch(signing, /\b(always|failure|cancelled)\s*\(/);
  assert.match(validation, /runs-on: windows-latest/);
  assert.match(signing, /runs-on: windows-latest/);
});
test("validation cannot supply workspace, artifacts, caches or secrets to signing", () => {
  assert.doesNotMatch(validation, /secrets\.|upload-artifact|actions\/cache|rust-cache|cache:\s*npm/);
  assert.doesNotMatch(signing, /download-artifact|actions\/cache|rust-cache|cache:\s*npm|needs\.validation\.outputs/);
  for (const job of [validation, signing]) {
    assert.match(job, /ref: \$\{\{ github\.sha \}\}/);
    assert.match(job, /persist-credentials: false/);
    assert.match(job, /npm ci --strict-allow-scripts/);
    assert.match(job, /node-version: 24\.19\.0/);
    assert.match(job, /npm@12\.0\.2/);
  }
});
test("signing key is scoped to signing and signature check precedes publication", () => {
  assert.equal((release.match(/secrets\.TAURI_SIGNING_PRIVATE_KEY\b/g) ?? []).length, 1);
  assert.equal((release.match(/secrets\.TAURI_SIGNING_PRIVATE_KEY_PASSWORD\b/g) ?? []).length, 1);
  assert.ok(signing.indexOf("Build unsigned NSIS") < signing.indexOf("Sign exact installer"));
  assert.ok(signing.indexOf("Verify updater signature before publication") < signing.indexOf("Stage complete draft"));
  assert.match(signing, /tauri build -- --no-sign -- --locked/);
  assert.match(ci, /tauri build -- -- --locked/);
  assert.doesNotMatch(signing, /npm test/);
});
