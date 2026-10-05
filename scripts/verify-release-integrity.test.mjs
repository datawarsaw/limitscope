// Unit tests for scripts/verify-release-integrity.mjs.
//
// All cases are offline: the pure check logic is exercised against
// fixtures/release-integrity/ (structurally valid, explicitly non-cryptographic
// minisign fixture material — no real key, no network, no git).

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  classifyVersionChannel,
  decodeConfigPubkey,
  evaluateRcInvocation,
  evaluateReleaseFlags,
  evaluateStableFeedIsolation,
  evaluateTagIdentity,
  extractProviderStatus,
  findDevUrlLeaks,
  keyIdCommentForm,  parseCargoLockVersion,
  parseCargoPackageVersion,
  parsePkgLockVersions,
  parsePubKey,
  parseSig,
  scanFeedForPrivateUrls,
  sigMatchesFeed,
  validateAssetSet,
  validateFeed,
  validateUpdaterContract,
  validateVersionSurfaces,
} from "./verify-release-integrity.mjs";

const fixtureDir = fileURLToPath(new URL("../fixtures/release-integrity/", import.meta.url));
const readFixture = (name) => readFileSync(path.join(fixtureDir, name), "utf8");
const readJsonFixture = (name) => JSON.parse(readFixture(name));

describe("minisign structure parsing", () => {
  it("parses the fixture public key and signature with matching key ids", () => {
    const pub = parsePubKey(readFixture("pubkey-fixture.pub"));
    const sig = parseSig(readFixture("sig-good.minisign"));
    expect(pub.algorithm).toBe("Ed");
    expect(sig.algorithm).toBe("ED");
    expect(sig.keyId).toBe(pub.keyId);
  });

  it("rejects a signature whose key differs from the fixture trust root", () => {
    const pub = parsePubKey(readFixture("pubkey-fixture.pub"));
    const other = parseSig(readFixture("sig-other.minisign"));
    expect(other.keyId).not.toBe(pub.keyId);
  });

  it("accepts the Tauri .sig armor form (base64 of the minisign text)", () => {
    const armored = readFixture("sig-good.armored.sig");
    expect(armored.trim()).not.toMatch(/^untrusted comment:/);
    expect(parseSig(armored)).toEqual(parseSig(readFixture("sig-good.minisign")));
  });

  it("rejects legacy or unknown signature algorithms", () => {
    const legacy = "untrusted comment: x\n" + Buffer.concat([Buffer.from("Ed"), Buffer.alloc(8), Buffer.alloc(64)]).toString("base64");
    expect(() => parseSig(legacy)).toThrow(/unexpected signature algorithm/);
  });

  it("reports key ids in the minisign comment form (little-endian)", () => {
    expect(keyIdCommentForm("0A0B0C0D0E0F0102")).toBe("02010F0E0D0C0B0A");
  });

  it("decodes the tauri.conf.json pubkey (base64 of the whole key file)", () => {
    const fileText = readFixture("pubkey-fixture.pub");
    const wrapped = Buffer.from(fileText).toString("base64");
    const pub = decodeConfigPubkey(wrapped);
    expect(pub.algorithm).toBe("Ed");
  });
});

describe("version surfaces", () => {
  const pkgLockText = JSON.stringify({
    name: "rate-limits",
    version: "9.9.9",
    lockfileVersion: 3,
    packages: { "": { name: "rate-limits", version: "9.9.9" } },
  });
  const good = {
    pkg: { version: "9.9.9" },
    pkgLock: pkgLockText,
    conf: { version: "9.9.9", productName: "LimitScope", mainBinaryName: "LimitScope" },
    cargoToml: '[package]\nname = "rate-limits"\nversion = "9.9.9"\n',
    cargoLock: '[[package]]\nname = "rate-limits"\nversion = "9.9.9"\n',
  };

  it("accepts agreeing surfaces (all five files, six version fields)", () => {
    expect(validateVersionSurfaces(good, "9.9.9")).toEqual([]);
  });

  it("fails on a single drifted surface", () => {
    const drifted = { ...good, cargoLock: '[[package]]\nname = "rate-limits"\nversion = "9.9.8"\n' };
    expect(validateVersionSurfaces(drifted, "9.9.9")).toHaveLength(1);
  });

  it("fails when the package-lock root version drifts from package.json", () => {
    const lock = JSON.parse(pkgLockText);
    lock.version = "9.9.8";
    const issues = validateVersionSurfaces({ ...good, pkgLock: JSON.stringify(lock) }, "9.9.9");
    expect(issues).toHaveLength(1);
    expect(issues[0]).toContain("package-lock.json root version");
  });

  it("fails when the package-lock packages[\"\"] version drifts", () => {
    const lock = JSON.parse(pkgLockText);
    lock.packages[""].version = "9.9.8";
    const issues = validateVersionSurfaces({ ...good, pkgLock: JSON.stringify(lock) }, "9.9.9");
    expect(issues).toHaveLength(1);
    expect(issues[0]).toContain('packages[""] version');
  });

  it("fails closed when package-lock.json is absent or unparseable", () => {
    for (const pkgLock of ["", "{not json"]) {
      const issues = validateVersionSurfaces({ ...good, pkgLock }, "9.9.9");
      expect(issues).toHaveLength(2);
      expect(issues[0]).toContain("package-lock.json root version");
      expect(issues[1]).toContain('packages[""] version');
    }
  });

  it("fails when productName/mainBinaryName leave the installer-name contract", () => {
    const renamed = { ...good, conf: { ...good.conf, productName: "SomethingElse" } };
    const issues = validateVersionSurfaces(renamed, "9.9.9");
    expect(issues.some((issue) => issue.includes("productName"))).toBe(true);
  });

  it("parses Cargo.toml, Cargo.lock and package-lock.json the way the release gate does", () => {
    expect(parseCargoPackageVersion(good.cargoToml)).toBe("9.9.9");
    expect(parseCargoLockVersion(good.cargoLock)).toBe("9.9.9");
    expect(parseCargoLockVersion("no match here")).toBeUndefined();
    expect(parsePkgLockVersions(pkgLockText)).toEqual({ root: "9.9.9", packages: "9.9.9" });
    expect(parsePkgLockVersions("nope")).toEqual({ root: undefined, packages: undefined });
  });
});

describe("updater contract and dev-URL leaks", () => {
  const base = {
    identifier: "com.ratelimits.desktop",
    build: { devUrl: "http://localhost:1420", frontendDist: "../dist" },
    app: { security: { csp: "default-src 'self'; connect-src 'self' ipc: http://ipc.localhost" } },
    plugins: {
      updater: {
        pubkey: Buffer.from(readFixture("pubkey-fixture.pub")).toString("base64"),
        endpoints: ["https://github.com/datawarsaw/limitscope-releases/releases/latest/download/latest.json"],
      },
    },
  };

  it("accepts the production contract (fixture key id pinned)", () => {
    const fixtureKeyId = keyIdCommentForm(parsePubKey(readFixture("pubkey-fixture.pub")).keyId);
    const { issues, pubkey } = validateUpdaterContract(base, { pinnedKeyId: fixtureKeyId });
    expect(issues).toEqual([]);
    expect(findDevUrlLeaks(base)).toEqual([]);
    expect(pubkey.keyId).toBe(parsePubKey(readFixture("pubkey-fixture.pub")).keyId);
  });

  it("fails closed when the trust-root key id is not the pinned one", () => {
    const { issues } = validateUpdaterContract(base, { pinnedKeyId: "0000000000000000" });
    expect(issues.some((issue) => issue.includes("key id") && issue.includes("managed key rotation"))).toBe(true);
  });

  it("pins the production trust-root key id", () => {
    const drifted = { ...base, plugins: { ...base.plugins, updater: { ...base.plugins.updater, pubkey: Buffer.from(readFixture("sig-other.minisign")).toString("base64") } } };
    const { issues } = validateUpdaterContract(drifted);
    // sig-other is a signature frame, not a pubkey — it must be rejected outright.
    expect(issues[0]).toMatch(/does not decode to a minisign public key|key id/);
  });

  it("fails when the updater endpoint is not exactly the production feed", () => {
    const local = { ...base, plugins: { ...base.plugins, updater: { ...base.plugins.updater, endpoints: ["http://127.0.0.1:3000/latest.json"] } } };
    const { issues } = validateUpdaterContract(local);
    expect(issues.some((issue) => issue.includes("updater endpoints"))).toBe(true);
  });

  it("allows devUrl only under build.devUrl", () => {
    const leaked = { ...base, app: { security: { csp: "connect-src http://localhost:1420 ipc:" } } };
    const leaks = findDevUrlLeaks(leaked);
    expect(leaks).toHaveLength(1);
    expect(leaks[0].path).toBe("app.security.csp");
  });

  it("requires the production IPC custom protocol in the CSP", () => {
    const noIpc = { ...base, app: { security: { csp: "default-src 'self'; connect-src 'self' https://api.example.com" } } };
    const leaks = findDevUrlLeaks(noIpc);
    expect(leaks.some((leak) => leak.reason.includes("IPC custom protocol"))).toBe(true);
  });
});

describe("provider registration", () => {
  const registered = ["codex", "opencode_go", "zai", "antigravity", "grok"]
    .flatMap((provider) => [`mod ${provider};`, `${provider}::get_${provider}_usage,`])
    .join("\n");

  it("accepts the five canonical providers", () => {
    const status = extractProviderStatus(registered);
    expect(status.missingMods).toEqual([]);
    expect(status.missingHandlers).toEqual([]);
    expect(status.claudeRegistered).toBe(false);
  });

  it("flags a missing provider registration", () => {
    const withoutGrok = registered.replace(/mod grok;\n/, "").replace(/grok::get_grok_usage,/, "");
    const status = extractProviderStatus(withoutGrok);
    expect(status.missingMods).toContain("grok");
    expect(status.missingHandlers).toContain("grok");
  });

  it("flags the forbidden experimental Claude registration", () => {
    const withClaude = registered + "\nmod claude;\nclaude::get_claude_usage,";
    expect(extractProviderStatus(withClaude).claudeRegistered).toBe(true);
  });
});

describe("feed contract (fixtures)", () => {
  const version = "9.9.9";
  const installerName = `LimitScope_${version}_x64-setup.exe`;

  it("accepts the correct metadata fixture", () => {
    const feed = readJsonFixture("latest-good.json");
    expect(validateFeed(feed, { version, installerName })).toEqual([]);
    expect(scanFeedForPrivateUrls(feed)).toEqual([]);
    expect(sigMatchesFeed(feed, readFixture("sig-good.minisign"))).toBe(true);
  });

  it("fails the wrong-version fixture", () => {
    const issues = validateFeed(readJsonFixture("latest-wrong-version.json"), { version, installerName });
    expect(issues.some((issue) => issue.includes("version"))).toBe(true);
  });

  it("fails the wrong-artifact-URL fixture", () => {
    const issues = validateFeed(readJsonFixture("latest-wrong-url.json"), { version, installerName });
    expect(issues.some((issue) => issue.includes("url"))).toBe(true);
  });

  it("fails the private-URL-leak fixture with a path-only finding", () => {
    const findings = scanFeedForPrivateUrls(readJsonFixture("latest-private-url.json"));
    expect(findings.length).toBeGreaterThan(0);
    for (const finding of findings) {
      expect(finding.path).toBe("platforms.windows-x86_64.url");
      expect(JSON.stringify(finding)).not.toContain("127.0.0.1");
    }
  });

  it("detects the mismatched-signature fixture", () => {
    const feed = readJsonFixture("latest-signature-mismatch.json");
    expect(sigMatchesFeed(feed, readFixture("sig-good.minisign"))).toBe(false);
    expect(sigMatchesFeed(feed, readFixture("sig-other.minisign"))).toBe(true);
  });

  it("matches the feed signature against the armored .sig fixture", () => {
    const feed = readJsonFixture("latest-good.json");
    expect(sigMatchesFeed(feed, readFixture("sig-good.armored.sig"))).toBe(true);
  });

  it("matches the feed signature even when the .sig file has CRLF endings (autocrlf checkout)", () => {
    const feed = readJsonFixture("latest-good.json");
    const crlfSig = readFixture("sig-good.minisign").replace(/\r\n/g, "\n").replace(/\n/g, "\r\n");
    expect(sigMatchesFeed(feed, crlfSig)).toBe(true);
    expect(parseSig(crlfSig)).toEqual(parseSig(readFixture("sig-good.minisign")));
  });

  it("flags plaintext and file URLs and PAT shapes anywhere in the feed", () => {
    // Assembled at runtime so no literal credential shape lands in this file
    // (src/updaterSecurity.test.ts scans scripts/ for exactly those shapes).
    const fakePat = ["ghp_", "abcdefghij", "klmnopqrst", "uvwx"].join("");
    const findings = scanFeedForPrivateUrls({ notes: `grab it from file://C:/x or ${fakePat}` });
    expect(findings.map((finding) => finding.finding)).toEqual(expect.arrayContaining(["file URL", "GitHub PAT shape"]));
  });
});

describe("release asset set (fixtures)", () => {
  it("accepts the exact three-asset set", () => {
    expect(validateAssetSet(readJsonFixture("assets-good.json").map((asset) => asset.name), "9.9.9")).toEqual([]);
  });

  it("fails the missing-signature fixture and any unexpected asset", () => {
    const missing = validateAssetSet(readJsonFixture("assets-missing-sig.json").map((asset) => asset.name), "9.9.9");
    expect(missing).toEqual([`missing release asset: LimitScope_9.9.9_x64-setup.exe.sig`]);
    expect(validateAssetSet(["latest.json", "extra.bin"], "9.9.9").some((issue) => issue.includes("unexpected"))).toBe(true);
  });
});

describe("tag identity (fixture)", () => {
  it("fails closed on the wrong-tag-target fixture", () => {
    const fixture = readJsonFixture("tag-target-wrong.json");
    const result = evaluateTagIdentity(fixture);
    expect(result.status).toBe("FAIL");
    expect(result.detail).toContain("not the expected product SHA");
  });

  it("accepts an annotated tag that dereferences to the product SHA", () => {
    const sha = "c".repeat(40);
    expect(evaluateTagIdentity({ tagExists: true, tagType: "tag", tagCommit: sha, expectedSha: sha, requireTag: true }).status).toBe("PASS");
  });

  it("fails on a lightweight tag and on a missing required tag", () => {
    expect(evaluateTagIdentity({ tagExists: true, tagType: "commit", tagCommit: "c".repeat(40), expectedSha: "c".repeat(40) }).status).toBe("FAIL");
    expect(evaluateTagIdentity({ tagExists: false, requireTag: true }).status).toBe("FAIL");
    expect(evaluateTagIdentity({ tagExists: false, requireTag: false }).status).toBe("SKIP");
  });
});

describe("RC channel derivation (version shape)", () => {
  it("derives the channel strictly from the version shape", () => {
    expect(classifyVersionChannel("0.8.7")).toBe("stable");
    expect(classifyVersionChannel("0.8.7-rc.1")).toBe("rc");
    expect(classifyVersionChannel("0.8.7-rc.12")).toBe("rc");
  });

  it("rejects malformed, non-RC prerelease and build-metadata shapes (fail closed)", () => {
    for (const malformed of ["0.8.7rc1", "0.8.7-rc", "0.8.7-rc.x", "0.8.7-beta.1", "0.8.7-rc.1+build", "v0.8.7-rc.1", "0.8", ""]) {
      expect(classifyVersionChannel(malformed)).toBeNull();
      expect(classifyVersionChannel(malformed)).not.toBe("rc");
    }
  });

  it("requires --rc for an RC version in stable mode (RC verification is opt-in)", () => {
    const issues = evaluateRcInvocation({ version: "0.8.7-rc.1", rcMode: false });
    expect(issues).toHaveLength(1);
    expect(issues[0]).toContain("--rc");
  });

  it("requires --stable-feed-version in RC mode and rejects non-stable feed versions", () => {
    const missing = evaluateRcInvocation({ version: "0.8.7-rc.1", rcMode: true });
    expect(missing.some((issue) => issue.includes("--stable-feed-version"))).toBe(true);
    const malformed = evaluateRcInvocation({ version: "0.8.7-rc.1", rcMode: true, stableFeedVersion: "0.8.7-rc.1" });
    expect(malformed.some((issue) => issue.includes("stable x.y.z"))).toBe(true);
  });

  it("keeps the stable path unchanged: plain versions and no stray RC options", () => {
    expect(evaluateRcInvocation({ version: "0.8.7", rcMode: false })).toEqual([]);
    const stray = evaluateRcInvocation({ version: "0.8.7", rcMode: false, stableFeedVersion: "0.8.6" });
    expect(stray).toHaveLength(1);
    expect(stray[0]).toContain("only meaningful with --rc");
  });

  it("rejects malformed versions in both modes (never reinterpreted)", () => {
    expect(evaluateRcInvocation({ version: "0.8.7rc1", rcMode: false })).toHaveLength(1);
    expect(evaluateRcInvocation({ version: "0.8.7-rc.x", rcMode: true, stableFeedVersion: "0.8.6" })).toHaveLength(1);
  });

  it("accepts the intended RC contract end-to-end", () => {
    expect(evaluateRcInvocation({ version: "0.8.7-rc.1", rcMode: true, stableFeedVersion: "0.8.6" })).toEqual([]);
  });

  it("carries the RC shape through the canonical checks (S3 surfaces, R4 feed URL)", () => {
    const pkgLockText = JSON.stringify({
      version: "0.8.7-rc.1",
      packages: { "": { version: "0.8.7-rc.1" } },
    });
    const surfaces = {
      pkg: { version: "0.8.7-rc.1" },
      pkgLock: pkgLockText,
      conf: { version: "0.8.7-rc.1", productName: "LimitScope", mainBinaryName: "LimitScope" },
      cargoToml: '[package]\nname = "rate-limits"\nversion = "0.8.7-rc.1"\n',
      cargoLock: '[[package]]\nname = "rate-limits"\nversion = "0.8.7-rc.1"\n',
    };
    expect(validateVersionSurfaces(surfaces, "0.8.7-rc.1")).toEqual([]);
    const feed = { version: "0.8.7-rc.1", platforms: { "windows-x86_64": { signature: "s", url: "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.7-rc.1/LimitScope_0.8.7-rc.1_x64-setup.exe" } } };
    expect(validateFeed(feed, { version: "0.8.7-rc.1", installerName: "LimitScope_0.8.7-rc.1_x64-setup.exe" })).toEqual([]);
  });
});

describe("RC release flags (R1 semantics per channel)", () => {
  it("accepts a stable release in stable mode", () => {
    const result = evaluateReleaseFlags({ isDraft: false, isPrerelease: false }, { rcMode: false });
    expect(result.status).toBe("PASS");
  });

  it("rejects a prerelease in stable mode (unchanged stable contract)", () => {
    const result = evaluateReleaseFlags({ isDraft: false, isPrerelease: true }, { rcMode: false });
    expect(result.status).toBe("FAIL");
    expect(result.detail).toContain("prerelease");
  });

  it("rejects a draft in stable mode", () => {
    expect(evaluateReleaseFlags({ isDraft: true, isPrerelease: false }, { rcMode: false }).status).toBe("FAIL");
  });

  it("accepts a published prerelease in RC mode", () => {
    const result = evaluateReleaseFlags({ isDraft: false, isPrerelease: true }, { rcMode: true });
    expect(result.status).toBe("PASS");
    expect(result.detail).toContain("releases/latest");
  });

  it("rejects a full release passed off as an RC (prerelease flag absent)", () => {
    const result = evaluateReleaseFlags({ isDraft: false, isPrerelease: false }, { rcMode: true });
    expect(result.status).toBe("FAIL");
    expect(result.detail).toContain("not a prerelease");
  });

  it("rejects a draft RC", () => {
    const result = evaluateReleaseFlags({ isDraft: true, isPrerelease: true }, { rcMode: true });
    expect(result.status).toBe("FAIL");
    expect(result.detail).toContain("draft");
  });
});

describe("stable-feed isolation (RC-mode R8)", () => {
  const base = {
    rcTag: "v0.8.7-rc.1",
    stableVersion: "0.8.6",
  };
  const stableFeed = (version) => JSON.stringify({
    version,
    platforms: { "windows-x86_64": { signature: "s", url: `https://github.com/datawarsaw/limitscope-releases/releases/download/v${version}/LimitScope_${version}_x64-setup.exe` } },
  });

  it("passes when the stable feed still serves the stable release", () => {
    const issues = evaluateStableFeedIsolation({
      ...base,
      stableReleaseFeedText: stableFeed("0.8.6"),
      redirectLocation: "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.6/latest.json",
      liveFeedText: stableFeed("0.8.6"),
    });
    expect(issues).toEqual([]);
  });

  it("rejects the RC being served by releases/latest (redirect target is the RC tag)", () => {
    const issues = evaluateStableFeedIsolation({
      ...base,
      redirectLocation: "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.7-rc.1/latest.json",
      liveFeedText: stableFeed("0.8.7-rc.1"),
    });
    expect(issues.some((issue) => issue.includes("redirects to the RC tag"))).toBe(true);
    expect(issues.some((issue) => issue.includes("expected the stable release 0.8.6"))).toBe(true);
  });

  it("rejects an unexpected stable-feed version change (stable channel moved)", () => {
    const issues = evaluateStableFeedIsolation({
      ...base,
      redirectLocation: "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.8/latest.json",
      liveFeedText: stableFeed("0.8.8"),
    });
    expect(issues.some((issue) => issue.includes("stable channel has moved"))).toBe(true);
  });

  it("rejects feed drift between the live endpoint and the stable release bytes", () => {
    const issues = evaluateStableFeedIsolation({
      ...base,
      stableReleaseFeedText: stableFeed("0.8.6"),
      redirectLocation: "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.6/latest.json",
      liveFeedText: `${stableFeed("0.8.6")}\n`,
    });
    expect(issues.some((issue) => issue.includes("bytes differ"))).toBe(true);
  });

  it("rejects a missing redirect and a feed that points at RC assets", () => {
    const noRedirect = evaluateStableFeedIsolation({ ...base, redirectLocation: null, liveFeedText: stableFeed("0.8.6") });
    expect(noRedirect.some((issue) => issue.includes("did not redirect"))).toBe(true);
    const rcAsset = evaluateStableFeedIsolation({
      ...base,
      redirectLocation: "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.6/latest.json",
      liveFeedText: JSON.stringify({ version: "0.8.6", platforms: { "windows-x86_64": { signature: "s", url: "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.7-rc.1/LimitScope_0.8.7-rc.1_x64-setup.exe" } } }),
    });
    expect(rcAsset.some((issue) => issue.includes("RC release asset"))).toBe(true);
  });

  it("rejects a non-JSON live stable feed", () => {
    const issues = evaluateStableFeedIsolation({
      ...base,
      redirectLocation: "https://github.com/datawarsaw/limitscope-releases/releases/download/v0.8.6/latest.json",
      liveFeedText: "not json",
    });
    expect(issues.some((issue) => issue.includes("not valid JSON"))).toBe(true);
  });
});
