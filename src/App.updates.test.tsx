/**
 * Pins the presentation of every updater phase in the Settings Updates
 * block. The state machine itself lives in `updater.ts` / `useUpdater.ts`;
 * this file guards what the user actually sees for each state, including
 * the two hard requirements from the update-flow brief: a visible manual
 * check entry point and error text that renders the safe message.
 *
 * Rendered with `react-dom/server` (same approach as the other hook tests:
 * no DOM environment, no new dependency).
 */
import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { UpdatesSection } from "./App";
import type { ReleaseNote, SafeUpdateError } from "./lib/updater";

const noop = () => {};
const ERROR: SafeUpdateError = {
  message: "Could not reach the update server. Check your connection.",
  detail: "error sending request: connection refused",
};
const UPDATE: ReleaseNote = {
  version: "0.6.0",
  notes: "Passive upgrade flow and update signing.",
};

/**
 * `renderToString` inserts `<!-- -->` markers between dynamic text nodes and
 * escapes apostrophes; flatten both so assertions read like the visible text.
 */
function text(html: string): string {
  return html.replace(/<!-- -->/g, "").replace(/&#x27;/g, "'");
}

function render(
  phase: Parameters<typeof UpdatesSection>[0]["phase"],
  overrides?: {
    update?: ReleaseNote | null;
    error?: SafeUpdateError | null;
    currentVersion?: string | null;
  },
): string {
  return renderToString(
    createElement(UpdatesSection, {
      phase,
      currentVersion:
        overrides?.currentVersion === undefined
          ? "0.5.0"
          : overrides.currentVersion,
      update: overrides?.update === undefined ? null : overrides.update,
      error: overrides?.error === undefined ? null : overrides.error,
      onCheck: noop,
      onInstall: noop,
      onDismiss: noop,
    }),
  );
}

describe("UpdatesSection", () => {
  it("idle shows the current version and the manual check button", () => {
    const html = text(render("idle"));
    expect(html).toContain("Current version");
    expect(html).toContain("0.5.0");
    expect(html).toContain("Check for updates");
    expect(html).not.toContain("disabled");
  });

  it("shows a dash when the version is unknown (plain-browser dev)", () => {
    const html = text(render("idle", { currentVersion: null }));
    expect(html).toContain("—");
  });

  it("checking disables the button and says so", () => {
    const html = text(render("checking"));
    expect(html).toContain("Checking…");
    expect(html).toContain("disabled");
    expect(html).not.toContain(">Check for updates<");
  });

  it("up to date confirms it and keeps the check button", () => {
    const html = text(render("upToDate"));
    expect(html).toContain("You're up to date.");
    expect(html).toContain("Check for updates");
  });

  it("update available shows version, notes and both actions", () => {
    const html = text(render("available", { update: UPDATE }));
    expect(html).toContain("Update available: LimitScope 0.6.0");
    expect(html).toContain("Passive upgrade flow and update signing.");
    expect(html).toContain("Update now");
    expect(html).toContain("Later");
    expect(html).not.toContain("Check for updates");
  });

  it("update available without notes renders no empty note", () => {
    const html = text(render("available", {
      update: { version: "0.6.0" },
    }));
    expect(html).toContain("Update available: LimitScope 0.6.0");
    expect(html).toContain("Update now");
  });

  it("downloading shows progress and no competing button", () => {
    const html = text(render("downloading"));
    expect(html).toContain("Downloading update…");
    expect(html).not.toContain("<button");
  });

  it("installing tells the user the app will restart", () => {
    const html = text(render("installing"));
    expect(html).toContain("Installing update — LimitScope will restart");
    expect(html).not.toContain("<button");
  });

  it("error renders the safe message with the raw detail as tooltip", () => {
    const html = text(render("error", { error: ERROR }));
    expect(html).toContain('role="alert"');
    expect(html).toContain(ERROR.message);
    expect(html).toContain(`title="${ERROR.detail}"`);
    expect(html).not.toContain("connection refused</span>");
  });

  it("every phase exposes the group for the settings landmark", () => {
    for (const phase of [
      "idle",
      "checking",
      "upToDate",
      "available",
      "downloading",
      "installing",
      "error",
    ] as const) {
      expect(render(phase)).toContain('aria-label="Updates"');
    }
  });
});
