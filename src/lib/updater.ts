/**
 * Pure updater logic: version rules and safe presentation of updater
 * results. No Tauri imports here so every rule stays unit-testable.
 *
 * Version rules mirror `tauri-plugin-updater` 2.x (`updater.rs`):
 * `release.version > current_version` (semver) is the only condition that
 * counts as an update. Equal and older versions are not offered, malformed
 * versions are rejected, so the app never downgrades by accident.
 */

export type UpdatePhase =
  | "idle"
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "installing"
  | "error";

export type SafeUpdateError = {
  /** Short, user-facing message with no raw internals. */
  message: string;
  /** Raw error string, safe for a tooltip — updater errors carry no secrets. */
  detail: string;
};

export type ReleaseNote = {
  version: string;
  notes?: string;
  date?: string;
};

type ParsedVersion = {
  major: number;
  minor: number;
  patch: number;
  prerelease: string[];
};

/**
 * Parses a version the way the Rust `semver` crate accepts the versions the
 * plugin compares: optional `v` prefix, `MAJOR.MINOR.PATCH` and an optional
 * `-prerelease`. Returns null for anything else (build metadata is ignored
 * in comparisons, same as semver precedence rules).
 */
export function parseVersion(raw: string | undefined | null): ParsedVersion | null {
  if (typeof raw !== "string") return null;
  const value = raw.trim().replace(/^v/, "");
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(value);
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] ? match[4].split(".") : [],
  };
}

function comparePrereleaseIdentifiers(a: string, b: string): number {
  const aNum = /^\d+$/.test(a);
  const bNum = /^\d+$/.test(b);
  if (aNum && bNum) {
    const aVal = Number(a);
    const bVal = Number(b);
    return aVal === bVal ? 0 : aVal < bVal ? -1 : 1;
  }
  // Numeric identifiers always sort below alphanumeric ones.
  if (aNum) return -1;
  if (bNum) return 1;
  return a === b ? 0 : a < b ? -1 : 1;
}

function compareParsed(a: ParsedVersion, b: ParsedVersion): number {
  if (a.major !== b.major) return a.major < b.major ? -1 : 1;
  if (a.minor !== b.minor) return a.minor < b.minor ? -1 : 1;
  if (a.patch !== b.patch) return a.patch < b.patch ? -1 : 1;
  // A release sorts above any of its prereleases.
  if (a.prerelease.length === 0 && b.prerelease.length === 0) return 0;
  if (a.prerelease.length === 0) return 1;
  if (b.prerelease.length === 0) return -1;
  const shared = Math.min(a.prerelease.length, b.prerelease.length);
  for (let i = 0; i < shared; i += 1) {
    const order = comparePrereleaseIdentifiers(a.prerelease[i], b.prerelease[i]);
    if (order !== 0) return order;
  }
  if (a.prerelease.length === b.prerelease.length) return 0;
  return a.prerelease.length < b.prerelease.length ? -1 : 1;
}

/** Full three-way comparison of two version strings; null for malformed input. */
export function compareVersions(
  a: string | undefined | null,
  b: string | undefined | null,
): number | null {
  const left = parseVersion(a);
  const right = parseVersion(b);
  if (!left || !right) return null;
  return compareParsed(left, right);
}

/**
 * Whether a remote release should be offered to the user. Mirrors the
 * plugin: strictly newer semver only — same version, downgrades and
 * malformed versions never produce an update prompt.
 */
export function isUpdateRelevant(
  remoteVersion: string | undefined | null,
  currentVersion: string | undefined | null,
): boolean {
  const order = compareVersions(remoteVersion, currentVersion);
  return order !== null && order > 0;
}

const MAX_NOTES_LENGTH = 280;

/**
 * Normalizes the release-notes text from the update feed into something
 * safe to render: strings only, trimmed, capped. Anything malformed
 * (missing, wrong type, whitespace) becomes "no notes".
 */
export function sanitizeReleaseNotes(notes: unknown): string | undefined {
  if (typeof notes !== "string") return undefined;
  const trimmed = notes.trim();
  if (!trimmed) return undefined;
  if (trimmed.length <= MAX_NOTES_LENGTH) return trimmed;
  return `${trimmed.slice(0, MAX_NOTES_LENGTH - 1).trimEnd()}…`;
}

/** Normalizes the feed's date string; non-strings and empty values are dropped. */
export function sanitizeReleaseDate(date: unknown): string | undefined {
  if (typeof date !== "string" || !date.trim()) return undefined;
  return date.trim();
}

/**
 * Maps a raw updater failure to a safe user-facing message. The updater
 * never handles credentials, so the raw string carries no secrets and is
 * kept as a secondary detail.
 */
export function formatUpdateError(error: unknown): SafeUpdateError {
  const detail = error instanceof Error ? error.message : String(error);
  const message = classifyUpdateError(detail);
  return { message, detail };
}

function classifyUpdateError(detail: string): string {
  const text = detail.toLowerCase();
  if (text.includes("insecure") || text.includes("transport protocol")) {
    return "The update feed is not using HTTPS, so the update was blocked.";
  }
  if (text.includes("signature")) {
    return "The update could not be verified, so it was not installed.";
  }
  if (text.includes("could not fetch a valid release json")) {
    return "The update feed could not be read. Try again later.";
  }
  if (
    /status code/.test(text) ||
    /\bhttp \d{3}\b/.test(text) ||
    text.includes("404") ||
    text.includes("403")
  ) {
    return "The update server returned an error. Try again later.";
  }
  if (
    text.includes("network") ||
    text.includes("timed out") ||
    text.includes("timeout") ||
    text.includes("connection") ||
    text.includes("dns error") ||
    text.includes("request") ||
    text.includes("channel closed")
  ) {
    return "Could not reach the update server. Check your connection.";
  }
  return "Something went wrong while checking for updates.";
}
