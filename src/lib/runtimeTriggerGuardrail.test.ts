import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * Structural guardrails for the v0.5 runtime resilience triggers. The Rust
 * runtime is the single refresh owner and the single owner of Retry-After
 * cooldowns; this scan pins that ownership to the source tree itself:
 *
 * - the webviews may only ever invoke the shared runtime/history/window
 *   commands plus the native drag-lifecycle attach/detach/restore trio —
 *   no provider fetch command exists in TS;
 * - the reconnect trigger only calls the shared `request_refresh_on_reconnect`
 *   command (Rust coalesces it; JS never fans out refreshes itself);
 * - no cooldown/Retry-After logic is duplicated in TS: cooldown state and
 *   cadence eligibility live in `src-tauri/src/runtime.rs` alone.
 */

const SRC_ROOT = fileURLToPath(new URL("..", import.meta.url));

const ALLOWED_COMMANDS = new Set([
  // snapshot + settings + refresh (shared runtime consumer contract)
  "get_runtime_snapshot",
  "set_refresh_interval",
  "set_quota_notifications_enabled",
  "request_refresh",
  "request_refresh_on_reconnect",
  // history is read from Rust only
  "get_history",
  "get_history_range",
  "get_usage_analytics",
  "clear_history",
  "import_legacy_history",
  // v0.7 local data: the provider cache clear takes no arguments at all, so
  // no path can cross this boundary.
  "clear_provider_cache",
  // Rust constructs, redacts, and writes the support bundle.
  "export_diagnostics",
  // Rust exports retained usage history to CSV or JSON; the folder picker
  // feeds the destination chooser that replaced the native save dialog.
  "pick_usage_export_directory",
  "export_usage_history",
  // window management
  "open_main_window",
  // Native drag lifecycle (Human-Accepted Slice A): attach/detach/restore
  // the Rust-owned window drag observer. Not a fetch and not a refresh.
  "attach_floating_drag_lifecycle",
  "detach_floating_drag_lifecycle",
  "restore_floating_drag_origin",
]);

function tsSources(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      found.push(...tsSources(path));
    } else if (/\.(ts|tsx)$/.test(entry.name) && !/\.test\.(ts|tsx)$/.test(entry.name)) {
      found.push(path);
    }
  }
  return found;
}

function invokedCommands(source: string): string[] {
  return [...source.matchAll(/invoke(?:<[^>]*>)?\(\s*["']([^"']+)["']/g)].map(
    (match) => match[1],
  );
}

/** Drops comments so the policy scan reads code, not prose. */
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:])\/\/[^\n]*/g, "$1");
}

describe("runtime trigger guardrails", () => {
  it("invokes only the shared runtime, history, and window commands", () => {
    const used = new Set<string>();
    for (const path of tsSources(SRC_ROOT)) {
      for (const command of invokedCommands(readFileSync(path, "utf8"))) {
        used.add(command);
      }
    }
    expect(
      [...used].filter((command) => !ALLOWED_COMMANDS.has(command)),
    ).toEqual([]);
  });

  it("never invokes a provider fetch command from TS", () => {
    for (const path of tsSources(SRC_ROOT)) {
      const source = readFileSync(path, "utf8");
      expect(source).not.toMatch(/invoke[^;]*["']get_(codex|zai|opencode_go|antigravity|grok)_usage["']/);
      expect(source).not.toMatch(/fetch\(/);
    }
  });

  it("the reconnect trigger only calls the shared runtime command", () => {
    const hook = readFileSync(
      join(SRC_ROOT, "hooks", "useProviderUsage.ts"),
      "utf8",
    );
    expect(hook).toContain('invoke("request_refresh_on_reconnect")');
    // Only the trigger effect may call the reconnect command, and that
    // effect must not fan out its own refreshes or snapshot pulls (the
    // manual `refresh` callback below it intentionally uses the plain
    // `request_refresh` command and is out of this slice).
    const start = hook.indexOf("Resilience trigger");
    const end = hook.indexOf("const refresh = useCallback");
    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    const triggerBlock = hook.slice(start, end);
    expect(triggerBlock).toContain('invoke("request_refresh_on_reconnect")');
    expect(triggerBlock).not.toContain('invoke("request_refresh")');
    expect(triggerBlock).not.toMatch(/invoke\("get_/);
  });

  it("keeps cooldown and Retry-After logic out of TS", () => {
    for (const path of tsSources(SRC_ROOT)) {
      const source = readFileSync(path, "utf8");
      expect(source, path).not.toMatch(/retry[-_ ]?after/i);
      expect(source, path).not.toMatch(/retryAfterMs/);
      // The v0.6 status contract ships the normalized health value as
      // read-only wire data: the `"cooldown"` union member and the
      // "Cooldown" display label are the only allowed spellings, and
      // comments are prose, not logic. Any other occurrence (a cooldown
      // variable, timer, or eligibility check) is policy leaking into TS
      // and must fail here.
      const code = stripComments(source)
        .replaceAll('"cooldown"', "")
        .replaceAll("'cooldown'", "")
        .replaceAll("Cooldown", "");
      expect(code, path).not.toMatch(/\bcooldown/i);
    }
  });
});
