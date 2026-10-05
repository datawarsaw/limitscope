import { describe, expect, it } from "vitest";
import {
  compareVersions,
  formatUpdateError,
  isUpdateRelevant,
  parseVersion,
  sanitizeReleaseDate,
  sanitizeReleaseNotes,
} from "./updater";

describe("parseVersion", () => {
  it("parses plain, v-prefixed and prerelease versions", () => {
    expect(parseVersion("0.3.1")).toEqual({
      major: 0,
      minor: 3,
      patch: 1,
      prerelease: [],
    });
    expect(parseVersion("v0.4.0")).toEqual({
      major: 0,
      minor: 4,
      patch: 0,
      prerelease: [],
    });
    expect(parseVersion("0.4.0-beta.1")).toEqual({
      major: 0,
      minor: 4,
      patch: 0,
      prerelease: ["beta", "1"],
    });
  });

  it("rejects malformed versions", () => {
    expect(parseVersion("")).toBeNull();
    expect(parseVersion("0.3")).toBeNull();
    expect(parseVersion("zero.three.one")).toBeNull();
    expect(parseVersion("0.3.1.2")).toBeNull();
    expect(parseVersion("not-a-version")).toBeNull();
    expect(parseVersion(undefined)).toBeNull();
    expect(parseVersion(null)).toBeNull();
  });
});

describe("compareVersions", () => {
  it("orders numeric fields", () => {
    expect(compareVersions("0.3.1", "0.3.1")).toBe(0);
    expect(compareVersions("0.3.2", "0.3.1")).toBe(1);
    expect(compareVersions("0.3.1", "0.3.2")).toBe(-1);
    expect(compareVersions("0.4.0", "0.3.99")).toBe(1);
    expect(compareVersions("1.0.0", "0.99.99")).toBe(1);
    // Semver compares fields numerically, not lexically.
    expect(compareVersions("0.3.10", "0.3.9")).toBe(1);
  });

  it("ignores the v prefix", () => {
    expect(compareVersions("v0.4.0", "0.4.0")).toBe(0);
  });

  it("ranks a release above its prereleases and orders prerelease parts", () => {
    expect(compareVersions("0.4.0", "0.4.0-beta.1")).toBe(1);
    expect(compareVersions("0.4.0-beta.1", "0.4.0-beta.2")).toBe(-1);
    expect(compareVersions("0.4.0-beta.2", "0.4.0-beta.10")).toBe(-1);
    expect(compareVersions("0.4.0-alpha", "0.4.0-beta")).toBe(-1);
    expect(compareVersions("0.4.0-1", "0.4.0-alpha")).toBe(-1);
  });

  it("returns null when either side is malformed", () => {
    expect(compareVersions("garbage", "0.3.1")).toBeNull();
    expect(compareVersions("0.3.1", "garbage")).toBeNull();
  });
});

describe("isUpdateRelevant (mirrors tauri-plugin-updater rules)", () => {
  const current = "0.3.1";

  it("offers strictly newer versions", () => {
    expect(isUpdateRelevant("0.3.2", current)).toBe(true);
    expect(isUpdateRelevant("0.4.0", current)).toBe(true);
    expect(isUpdateRelevant("1.0.0", current)).toBe(true);
  });

  it("does not offer the same version", () => {
    expect(isUpdateRelevant("0.3.1", current)).toBe(false);
  });

  it("does not offer downgrades", () => {
    expect(isUpdateRelevant("0.3.0", current)).toBe(false);
    expect(isUpdateRelevant("0.2.9", current)).toBe(false);
  });

  it("rejects malformed remote versions instead of updating", () => {
    expect(isUpdateRelevant("", current)).toBe(false);
    expect(isUpdateRelevant("latest", current)).toBe(false);
    expect(isUpdateRelevant(undefined, current)).toBe(false);
  });

  it("rejects when the current version is unreadable", () => {
    expect(isUpdateRelevant("9.9.9", undefined)).toBe(false);
  });
});

describe("sanitizeReleaseNotes", () => {
  it("keeps a plain string", () => {
    expect(sanitizeReleaseNotes("Faster refresh and bug fixes.")).toBe(
      "Faster refresh and bug fixes.",
    );
  });

  it("trims whitespace", () => {
    expect(sanitizeReleaseNotes("  hello  ")).toBe("hello");
  });

  it("drops missing, non-string and empty notes", () => {
    expect(sanitizeReleaseNotes(undefined)).toBeUndefined();
    expect(sanitizeReleaseNotes(null)).toBeUndefined();
    expect(sanitizeReleaseNotes(42)).toBeUndefined();
    expect(sanitizeReleaseNotes({})).toBeUndefined();
    expect(sanitizeReleaseNotes("   ")).toBeUndefined();
    expect(sanitizeReleaseNotes("")).toBeUndefined();
  });

  it("caps very long notes with an ellipsis", () => {
    const long = "a".repeat(500);
    const capped = sanitizeReleaseNotes(long);
    expect(capped).toBeDefined();
    expect(capped!.length).toBeLessThanOrEqual(280);
    expect(capped!.endsWith("…")).toBe(true);
  });
});

describe("sanitizeReleaseDate", () => {
  it("keeps non-empty strings and drops everything else", () => {
    expect(sanitizeReleaseDate("2026-09-28T12:00:00Z")).toBe(
      "2026-09-28T12:00:00Z",
    );
    expect(sanitizeReleaseDate("  ")).toBeUndefined();
    expect(sanitizeReleaseDate(1234)).toBeUndefined();
    expect(sanitizeReleaseDate(undefined)).toBeUndefined();
  });
});

describe("formatUpdateError", () => {
  it("classifies signature failures as verification errors", () => {
    const result = formatUpdateError(
      new Error("signature verification failed: bad signature"),
    );
    expect(result.message).toBe(
      "The update could not be verified, so it was not installed.",
    );
    expect(result.detail).toContain("bad signature");
  });

  it("classifies unreachable feeds as network errors", () => {
    for (const raw of [
      "error sending request: connection refused",
      "operation timed out",
      "dns error: no record",
    ]) {
      expect(formatUpdateError(raw).message).toBe(
        "Could not reach the update server. Check your connection.",
      );
    }
  });

  it("classifies malformed or missing feed metadata", () => {
    expect(
      formatUpdateError("Could not fetch a valid release JSON from the remote")
        .message,
    ).toBe("The update feed could not be read. Try again later.");
  });

  it("classifies insecure transport protocols as blocked", () => {
    expect(formatUpdateError("InsecureTransportProtocol").message).toBe(
      "The update feed is not using HTTPS, so the update was blocked.",
    );
  });

  it("falls back to a generic message for unknown failures", () => {
    expect(formatUpdateError(new Error("something odd")).message).toBe(
      "Something went wrong while checking for updates.",
    );
  });

  it("keeps the raw detail but never invents content", () => {
    const result = formatUpdateError(new Error("HTTP 503"));
    expect(result.detail).toBe("HTTP 503");
    expect(result.message).toBe(
      "The update server returned an error. Try again later.",
    );
  });
});
