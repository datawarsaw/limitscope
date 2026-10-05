import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { SETTINGS_STORAGE_KEY } from "./lib/settings";

/**
 * Brand guardrail for the Rate Limits → LimitScope identity migration.
 *
 * It distinguishes three classes of strings:
 * - current product identity ("Rate Limits" as the app's own name) — not
 *   allowed on any user-facing surface of this branch;
 * - technical phrases ("rate limit", "rate limited") — unaffected, never
 *   matched (the guard scans for the exact product-name form only);
 * - historical records (docs/, state/, and README's previous-name note) —
 *   deliberately outside the scan, because they document the past.
 */

const ROOT = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");

function listFiles(dir: string, extensions: readonly string[]): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      if (
        entry === "node_modules" ||
        entry === "target" ||
        entry === "dist" ||
        entry === "icons" ||
        entry.startsWith(".")
      ) {
        continue;
      }
      out.push(...listFiles(full, extensions));
    } else if (extensions.some((ext) => entry.endsWith(ext))) {
      out.push(full);
    }
  }
  return out;
}

function read(relative: string): string {
  return readFileSync(join(ROOT, relative), "utf8");
}

describe("LimitScope brand identity", () => {
  it("names the product LimitScope in the Tauri metadata and window titles", () => {
    const conf = JSON.parse(read("src-tauri/tauri.conf.json"));
    expect(conf.productName).toBe("LimitScope");
    // The shipped binary carries the brand (it is what Task Manager and the
    // autostart Run entry show), instead of the cargo crate name.
    expect(conf.mainBinaryName).toBe("LimitScope");
    expect(conf.app.windows).toHaveLength(2);
    for (const window of conf.app.windows) {
      expect(window.title).toBe("LimitScope");
    }
  });

  it("preserves the internal app identifier (settings/history continuity)", () => {
    const conf = JSON.parse(read("src-tauri/tauri.conf.json"));
    // The identifier keys the app-data directory (quota history, notification
    // state) and the WebView2 profile (settings in localStorage). Renaming it
    // would orphan all of it; the brand migration deliberately keeps it.
    expect(conf.identifier).toBe("com.ratelimits.desktop");
    // The settings storage key is part of the same continuity contract.
    expect(SETTINGS_STORAGE_KEY).toBe("rate-limits.settings.v1");
  });

  it("wires the upgrade-migration installer hooks", () => {
    const conf = JSON.parse(read("src-tauri/tauri.conf.json"));
    const hookPath = conf.bundle.windows.nsis.installerHooks;
    expect(hookPath).toBe("./installer-hooks.nsh");
    const hooks = read(join("src-tauri", hookPath));
    // Detects the pre-rename install and upgrades it in place…
    expect(hooks).toContain("Uninstall\\Rate Limits");
    expect(hooks).toContain("StrCpy $INSTDIR $R0");
    // …and migrates the autostart Run entry to the new product name.
    expect(hooks).toContain('"Rate Limits"');
    expect(hooks).toContain('WriteRegStr HKCU "${LIMITSCOPE_RUN_KEY}" "${LIMITSCOPE_RUN_NAME}"');
  });

  it("carries the product identity in the tray and autostart registration", () => {
    const main = read("src-tauri/src/main.rs");
    expect(main).toContain('.tooltip("LimitScope")');
    expect(main).toContain('.app_name("LimitScope")');
  });

  it("keeps settings/history/autostart reconciliation behavior unchanged", () => {
    // The reconciliation contract (OS state authoritative at launch) is what
    // makes the Run-key migration safe; this pins the imports it relies on.
    const useSettings = read("src/hooks/useSettings.ts");
    expect(useSettings).toContain("isEnabled()");
    expect(useSettings).toContain("disable, enable, isEnabled");
  });

  it("leaves no stale current-brand 'Rate Limits' strings in code or config", () => {
    const stale: string[] = [];
    for (const file of listFiles(ROOT, [".ts", ".tsx", ".rs", ".html", ".json", ".nsh", ".mjs", ".toml"])) {
      const relative = file.slice(ROOT.length).replaceAll("\\", "/");
      // Docs/state are historical records; generated platform icons are
      // binary. The guardrail itself and the installer's migration logic are
      // the two places the old name must remain as a literal. Everything
      // else is current product surface.
      if (
        relative.startsWith("docs/") ||
        relative.startsWith("state/") ||
        relative.startsWith("artifacts/") ||
        relative.startsWith("src-tauri/target/") ||
        relative === "src/brand.test.ts" ||
        relative === "src-tauri/installer-hooks.nsh"
      ) {
        continue;
      }
      if (relative === "package-lock.json") continue;
      const text = read(relative);
      if (/Rate Limits/i.test(text)) stale.push(relative);
    }
    expect(stale).toEqual([]);
  });

  it("documents the rename in current product-facing docs without rewriting history", () => {
    const readme = read("README.md");
    expect(readme.startsWith("# LimitScope")).toBe(true);
    expect(readme).toContain("working name **Rate Limits**");
    // Historical release records keep their original identity (the internal
    // delivery ledger is excluded from the public tree; the public release
    // notes carry the original name instead).
    const releaseNotes = read("docs/release-notes-v0.2.0.md");
    expect(releaseNotes).toContain("Rate Limits");
  });
});
