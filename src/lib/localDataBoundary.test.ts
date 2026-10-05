import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Structural regression guard for the v0.7 local data boundary.
 *
 * The product promise is that a clear can only ever reach a fixed,
 * application-owned target: no filesystem path crosses the IPC boundary, no
 * generic delete command exists, and no clear path names an external
 * credential location. Those are properties of the source tree, so they are
 * pinned here by reading it rather than by trusting a call site.
 *
 * Only code is scanned: the modules document the boundary in prose (naming
 * the credential stores they deliberately never touch), and comments are
 * stripped before every assertion.
 */

const REPO_ROOT = join(import.meta.dirname, "../..");
const RUST_MAIN = join(REPO_ROOT, "src-tauri/src/main.rs");
const RUST_LOCAL_DATA = join(REPO_ROOT, "src-tauri/src/local_data.rs");
const TS_LOCAL_DATA = join(REPO_ROOT, "src/lib/localData.ts");

/** The complete IPC surface. Growing it is a deliberate, reviewed act. */
const IPC_SURFACE = new Set([
  "codex::get_codex_usage",
  "opencode_go::get_opencode_go_usage",
  "zai::get_zai_usage",
  "antigravity::get_antigravity_usage",
  "grok::get_grok_usage",
  "open_main_window",
  "runtime::get_runtime_snapshot",
  "runtime::request_refresh",
  "runtime::request_refresh_on_reconnect",
  "runtime::set_refresh_interval",
  "runtime::set_quota_notifications_enabled",
  "history::get_history",
  "history::get_history_range",
  "history::clear_history",
  "history::import_legacy_history",
  "usage_analytics::get_usage_analytics",
  "local_data::clear_provider_cache",
  "diagnostics::export_diagnostics",
  "usage_export::pick_usage_export_directory",
  "usage_export::export_usage_history",
  // Native drag lifecycle (Human-Accepted Slice A): the Rust window subclass
  // owns the gesture; these commands attach, detach and restore it only.
  "floating_drag::attach_floating_drag_lifecycle",
  "floating_drag::detach_floating_drag_lifecycle",
  "floating_drag::restore_floating_drag_origin",
]);

/** Credential material LimitScope must never name as a target. */
const EXTERNAL_CREDENTIAL_MARKERS =
  /auth\.json|credentials\.json|\.codex|\.ssh|keychain|credential manager|windows credentials|%appdata%|appdata/i;

function stripRustComments(source: string): string {
  return source.replace(/\/\/[^\n]*/g, "").replace(/\/\*[\s\S]*?\*\//g, "");
}

function registeredCommands(): string[] {
  const source = readFileSync(RUST_MAIN, "utf8");
  const handler = /invoke_handler\(tauri::generate_handler!\[([\s\S]*?)\]\)/.exec(
    source,
  );
  expect(handler).not.toBeNull();
  return (handler as RegExpExecArray)[1]
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

describe("local data IPC boundary", () => {
  it("exposes exactly the pinned command surface and no generic delete", () => {
    const commands = registeredCommands();
    expect([...commands].sort()).toEqual([...IPC_SURFACE].sort());
    for (const command of commands) {
      expect(command, command).not.toMatch(
        /delete|unlink|trash|purge|wipe|remove_file|rmdir/i,
      );
    }
  });

  it("keeps every clear command free of caller-supplied paths", () => {
    const source = stripRustComments(readFileSync(RUST_LOCAL_DATA, "utf8"));
    const signatures = [...source.matchAll(/pub fn (clear_\w+)\(([^)]*)\)/g)];
    expect(signatures.length).toBeGreaterThan(0);
    for (const [, name, params] of signatures) {
      expect(params, name).not.toMatch(/PathBuf|Path|String|&str|Value/);
      expect(params, name).toMatch(/tauri::State<RuntimeHandle>/);
    }
  });

  it("names no file and no external credential location in its code", () => {
    const source = readFileSync(RUST_LOCAL_DATA, "utf8");
    const code = stripRustComments(
      source.slice(0, source.indexOf("#[cfg(test)]")),
    );
    expect(code).not.toMatch(/"[^"]*\.json"/);
    expect(code).not.toMatch(EXTERNAL_CREDENTIAL_MARKERS);
    // The store path is owned by the store the runtime already opens.
    expect(code).not.toMatch(/PathBuf::from|std::fs::remove_file|fs::remove_file/);
    // The only file names in the module are test fixtures proving that
    // credential-shaped siblings survive a cache clear.
    const tests = stripRustComments(source.slice(source.indexOf("#[cfg(test)]")));
    expect(tests).toMatch(/auth\.json/);
  });

  it("does document the credential boundary it enforces", () => {
    const docs = readFileSync(RUST_LOCAL_DATA, "utf8");
    expect(docs).toMatch(/auth\.json/);
    expect(docs).toMatch(/never deletes or mutates external authentication material/);
  });

  it("reaches Rust only through the fixed command table", () => {
    const source = stripTsComments(readFileSync(TS_LOCAL_DATA, "utf8"));
    const table = [...source.matchAll(/^\s{2}(\w+):\s*"([a-z_]+)",/gm)];
    expect(table.length).toBeGreaterThan(0);
    for (const [, key, command] of table) {
      expect(
        registeredCommands().some((entry) => entry.endsWith("::" + command)),
        key,
      ).toBe(true);
    }
    const calls = [
      ...source.matchAll(/invoke(?:<[^>]*>)?\(\s*([^)]*?)\s*\)/g),
    ];
    expect(calls.length).toBeGreaterThan(0);
    for (const [, args] of calls) {
      // A fixed command name, or a member of the fixed constant table - never
      // a computed, caller-supplied, or path-shaped argument.
      expect(args, args).toMatch(
        /^(["'][a-z_]+["']|LOCAL_DATA_COMMANDS\.\w+),?$/,
      );
    }
  });
});

function stripTsComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:])\/\/[^\n]*/g, "$1");
}
