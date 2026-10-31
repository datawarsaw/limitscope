import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * Structural guardrails for IPC window authorization (P2).
 *
 * Tauri only enforces the capability ACL for application commands once the
 * app defines its own permission manifest in src-tauri/permissions. These
 * tests pin that manifest to the source tree:
 *
 * - every capability grant resolves to a command that is actually registered,
 * - the floating bar keeps exactly its read/refresh/lifecycle contract
 *   (without an attach-time refresh-interval push, and with the locate-ripple
 *   overlay creation permission),
 * - every other registered command — history clears, exports, settings
 *   writes, account writes, and the legacy provider fetch commands — is
 *   denied to the floating window,
 * - the main dashboard keeps the full shared-runtime contract and is denied
 *   the floating-only window lifecycle commands.
 */

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const MAIN_RS = join(ROOT, "src-tauri", "src", "main.rs");
const PERMISSIONS_DIR = join(ROOT, "src-tauri", "permissions");
const CAPABILITIES_DIR = join(ROOT, "src-tauri", "capabilities");

/**
 * Commands the floating window actually reaches, derived from its component
 * tree (FloatingQuotaWindow -> useProviderUsage/useSettings,
 * floatingWindowChrome, GrokBotSection, OpenDesignRefreshControl). It mounts
 * useProviderUsage without onAttach, so the interval belongs to the dashboard.
 */
const FLOATING_COMMANDS = new Set([
  "get_runtime_snapshot",
  "request_refresh",
  "request_refresh_on_reconnect",
  "request_wallet_refresh",
  "get_operational_settings",
  "get_codex_accounts",
  "get_usage_intelligence",
  "open_main_window",
  "attach_floating_drag_lifecycle",
  "detach_floating_drag_lifecycle",
  "restore_floating_drag_origin",
  "attach_floating_monitor_observer",
  "detach_floating_monitor_observer",
  "refresh_grok_bot_usage",
  "refresh_opendesign_usage",
]);

/** The dashboard contract: every shared command except the floating-only ones. */
const MAIN_COMMANDS = new Set([
  "get_runtime_snapshot",
  "set_refresh_interval",
  "set_quota_notifications_enabled",
  "set_wallet_monitoring",
  "set_codex_accounts",
  "get_codex_accounts",
  "get_operational_settings",
  "request_refresh",
  "request_refresh_on_reconnect",
  "request_wallet_refresh",
  "get_history",
  "get_history_range",
  "get_usage_analytics",
  "clear_history",
  "import_legacy_history",
  "set_usage_intelligence_enabled",
  "get_usage_intelligence",
  "clear_usage_intelligence",
  "clear_provider_cache",
  "export_diagnostics",
  "pick_usage_export_directory",
  "export_usage_history",
  "refresh_grok_bot_usage",
  "refresh_opendesign_usage",
]);

/**
 * Registered but deliberately unreachable from any window: the pre-runtime
 * provider fetch commands. TS never invokes them (the runtime owns fetching);
 * the app ACL manifest now denies them everywhere instead of leaving them
 * callable from both webviews.
 */
const LEGACY_PROVIDER_FETCHES = [
  "get_codex_usage",
  "get_opencode_go_usage",
  "get_zai_usage",
  "get_antigravity_usage",
  "get_grok_usage",
];

/** High-impact commands the floating bar must never reach. */
const FLOATING_DENIED_SAMPLE = [
  "set_refresh_interval",
  "set_quota_notifications_enabled",
  "set_wallet_monitoring",
  "set_codex_accounts",
  "set_usage_intelligence_enabled",
  "clear_usage_intelligence",
  "clear_history",
  "import_legacy_history",
  "clear_provider_cache",
  "export_diagnostics",
  "pick_usage_export_directory",
  "export_usage_history",
  ...LEGACY_PROVIDER_FETCHES,
];

function registeredCommands(): Set<string> {
  const source = readFileSync(MAIN_RS, "utf8");
  const match = /invoke_handler\(tauri::generate_handler!\[([\s\S]*?)\]\)/.exec(source);
  if (!match) throw new Error("could not find the invoke handler in main.rs");
  return new Set(
    match[1]
      .split(/[\s,]+/)
      .map((token) => token.trim())
      .filter(Boolean)
      .map((path) => path.split("::").pop()!),
  );
}

type AppPermission = { identifier: string; commands: string[] };

function parsePermissions(source: string): AppPermission[] {
  const permissions: AppPermission[] = [];
  for (const block of source.matchAll(/\[\[permission\]\]([\s\S]*?)(?=\[\[permission\]\]|$)/g)) {
    const identifier = /^identifier\s*=\s*"([^"]+)"/m.exec(block[1])?.[1];
    const allow = /commands\.allow\s*=\s*\[([^\]]*)\]/.exec(block[1])?.[1] ?? "";
    const commands = [...allow.matchAll(/"([^"]+)"/g)].map((entry) => entry[1]);
    if (!identifier || commands.length === 0) {
      throw new Error("permission block is missing an identifier or allowed commands");
    }
    permissions.push({ identifier, commands });
  }
  return permissions;
}

function filesUnder(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) found.push(...filesUnder(path));
    else if (entry.name.endsWith(".toml")) found.push(path);
  }
  return found;
}

function appPermissions(): Map<string, string[]> {
  const map = new Map<string, string[]>();
  for (const path of filesUnder(PERMISSIONS_DIR)) {
    for (const permission of parsePermissions(readFileSync(path, "utf8"))) {
      if (map.has(permission.identifier)) {
        throw new Error(`duplicate permission identifier: ${permission.identifier}`);
      }
      map.set(permission.identifier, permission.commands);
    }
  }
  return map;
}

type Capability = { identifier: string; windows: string[]; permissions: string[] };

function capabilities(): Capability[] {
  return readdirSync(CAPABILITIES_DIR)
    .filter((name) => name.endsWith(".json"))
    .map((name) => JSON.parse(readFileSync(join(CAPABILITIES_DIR, name), "utf8")));
}

/** Resolves a capability's app-permission grants to concrete command names. */
function resolvedCommands(capability: Capability, permissions: Map<string, string[]>): Set<string> {
  const resolved = new Set<string>();
  for (const reference of capability.permissions) {
    // Prefixed references are core/plugin permissions; they do not govern
    // application commands.
    if (reference.includes(":")) continue;
    for (const command of permissions.get(reference) ?? []) resolved.add(command);
  }
  return resolved;
}

function invokedCommands(source: string): Set<string> {
  return new Set(
    [...source.matchAll(/invoke(?:<[^>]*>)?\(\s*["']([^"']+)["']/g)].map((m) => m[1]),
  );
}

describe("IPC window authorization", () => {
  const permissions = appPermissions();
  const caps = capabilities();
  const byWindow = (label: string) =>
    caps.filter((capability) => capability.windows.includes(label));

  const floatingGranted = new Set(
    byWindow("floating-quota").flatMap((capability) => [
      ...resolvedCommands(capability, permissions),
    ]),
  );
  const mainGranted = new Set(
    byWindow("main").flatMap((capability) => [...resolvedCommands(capability, permissions)]),
  );

  it("the app permission manifest exists", () => {
    // Without at least one application permission, Tauri skips the ACL for
    // app commands entirely and both windows can invoke all of them.
    expect(permissions.size).toBeGreaterThan(0);
  });

  it("grants only registered commands and leaves only legacy fetches unreachable", () => {
    const registered = registeredCommands();
    const granted = new Set([...floatingGranted, ...mainGranted]);
    expect([...granted].filter((command) => !registered.has(command))).toEqual([]);
    expect(
      [...registered].filter(
        (command) => !granted.has(command) && !LEGACY_PROVIDER_FETCHES.includes(command),
      ),
    ).toEqual([]);
    for (const command of LEGACY_PROVIDER_FETCHES) {
      expect(registered.has(command)).toBe(true);
      expect(granted.has(command)).toBe(false);
    }
  });

  it("every permission reference resolves and every permission is used", () => {
    for (const capability of caps) {
      for (const reference of capability.permissions) {
        if (reference.includes(":")) continue;
        expect(permissions.has(reference), `${capability.identifier} -> ${reference}`).toBe(true);
      }
    }
    const referenced = new Set(caps.flatMap((capability) => capability.permissions));
    for (const identifier of permissions.keys()) {
      expect(referenced.has(identifier), identifier).toBe(true);
    }
  });

  it("the floating window keeps exactly its read/refresh/lifecycle contract", () => {
    expect([...floatingGranted].sort()).toEqual([...FLOATING_COMMANDS].sort());
  });

  it("the main window keeps the dashboard contract and no floating lifecycle", () => {
    expect([...mainGranted].sort()).toEqual([...MAIN_COMMANDS].sort());
    for (const command of [
      "open_main_window",
      "attach_floating_drag_lifecycle",
      "detach_floating_drag_lifecycle",
      "restore_floating_drag_origin",
      "attach_floating_monitor_observer",
      "detach_floating_monitor_observer",
    ]) {
      expect(mainGranted.has(command)).toBe(false);
    }
  });

  it("denies every non-granted command to the floating window", () => {
    const registered = registeredCommands();
    for (const command of registered) {
      expect(floatingGranted.has(command)).toBe(FLOATING_COMMANDS.has(command));
    }
    // The high-impact denials, spelled out for review:
    for (const command of FLOATING_DENIED_SAMPLE) {
      expect(floatingGranted.has(command)).toBe(false);
    }
  });

  it("preserves the floating window's own IPC paths", () => {
    const chrome = invokedCommands(
      readFileSync(join(ROOT, "src", "lib", "floatingWindowChrome.ts"), "utf8"),
    );
    for (const command of chrome) {
      expect(FLOATING_COMMANDS.has(command), command).toBe(true);
    }
    // Shared hooks the floating window mounts: snapshot pull and the three
    // Rust-owned reads. The interval hand-off belongs exclusively to the main
    // dashboard's settings queue, so the floating view stays read-only.
    const usageHook = readFileSync(join(ROOT, "src", "hooks", "useProviderUsage.ts"), "utf8");
    expect(usageHook).toContain('invoke<RuntimeSnapshot>("get_runtime_snapshot")');
    expect(usageHook).not.toContain('invoke("set_refresh_interval"');
    const floatingView = readFileSync(
      join(ROOT, "src", "components", "FloatingQuotaWindow.tsx"),
      "utf8",
    );
    expect(floatingView).toContain("} = useProviderUsage();");
    expect(floatingView).not.toContain("set_refresh_interval");
    const settingsHook = readFileSync(join(ROOT, "src", "hooks", "useSettings.ts"), "utf8");
    expect(settingsHook).toContain('invoke<void>("set_refresh_interval"');
    expect(settingsHook).toContain('invoke<OperationalSettings>("get_operational_settings")');
    expect(settingsHook).toContain('invoke<CodexAccountSettings[]>("get_codex_accounts")');
    expect(settingsHook).toContain('invoke<{ enabled: boolean }>("get_usage_intelligence"');
  });

  it("keeps the locate-ripple overlay creation permission paired with its use", () => {
    const locator = readFileSync(join(ROOT, "src", "lib", "locateRipple.ts"), "utf8");
    expect(locator).toContain("new WebviewWindow(");
    for (const capability of byWindow("floating-quota")) {
      expect(capability.permissions).toContain("core:webview:allow-create-webview-window");
    }
  });
});
