// Offline source contract only: this test never invokes NSIS or reads the registry.
import { readFileSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const config = JSON.parse(read("src-tauri/tauri.conf.json"));
const hooks = read("src-tauri/installer-hooks.nsh");
const main = read("src-tauri/src/main.rs");

test("installer retains application identity and configured updater key", () => {
  assert.equal(config.identifier, "com.ratelimits.desktop");
  assert.equal(config.productName, "LimitScope");
  assert.equal(config.mainBinaryName, "LimitScope");
  assert.equal(config.bundle.windows.nsis.installerHooks, "./installer-hooks.nsh");
  assert.equal(config.plugins.updater.pubkey, "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDAyNUZGMzZERTNGRjQ0RUIKUldUclJQL2piZk5mQWdhQ0FtNVNUSHRoNURJNER6UDVQQmZMeHk4NS9MYjlRYkFyMG0vcHFFOVoK");
});

test("legacy autostart transition is conditional and points to the branded binary", () => {
  const pre = hooks.match(/!macro NSIS_HOOK_PREINSTALL\b([\s\S]*?)!macroend/)[1];
  const post = hooks.match(/!macro NSIS_HOOK_POSTINSTALL\b([\s\S]*?)!macroend/)[1];
  const legacyName = ["Rate", "Limits"].join(" ");
  assert.ok(hooks.includes(`!define LIMITSCOPE_OLD_RUN_NAME "${legacyName}"`));
  assert.match(hooks, /!define LIMITSCOPE_RUN_NAME "LimitScope"/);
  assert.match(pre, /ReadRegStr \$R7 HKCU "\$\{LIMITSCOPE_RUN_KEY\}" "\$\{LIMITSCOPE_OLD_RUN_NAME\}"/);
  assert.match(post, /\$\{If\} \$R7 != ""[\s\S]*DeleteRegValue HKCU "\$\{LIMITSCOPE_RUN_KEY\}" "\$\{LIMITSCOPE_OLD_RUN_NAME\}"[\s\S]*WriteRegStr HKCU "\$\{LIMITSCOPE_RUN_KEY\}" "\$\{LIMITSCOPE_RUN_NAME\}" '\"\$INSTDIR\\LimitScope\.exe\" --hidden'[\s\S]*\$\{EndIf\}/);
  assert.match(main, /const AUTOSTART_ARG: &str = "--hidden";/);
  assert.match(main, /\.app_name\("LimitScope"\)\s*\.arg\(AUTOSTART_ARG\)/);
});

// Deliberately limited synthetic interpreter of the actual POSTINSTALL source.
// Only Run-value reads/writes/deletes and If/OrIf/EndIf are supported; unknown
// syntax fails closed. This is not NSIS execution or Windows registry evidence.
const runKey = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const legacyName = "Rate Limits";
const currentName = "LimitScope";
const pre = hooks.match(/!macro NSIS_HOOK_PREINSTALL\b([\s\S]*?)!macroend/)[1];
const post = hooks.match(/!macro NSIS_HOOK_POSTINSTALL\b([\s\S]*?)!macroend/)[1];
const instructions = (source) => source.split(/\r?\n/)
  .map((line) => line.trim()).filter((line) => line && !line.startsWith(";"));
const definitions = Object.fromEntries(
  [...hooks.matchAll(/^!define (\w+) "([^"]*)"/gm)].map((match) => match.slice(1)),
);

function syntheticPost(values, { installDir = "C:\\Synthetic Install\\LimitScope", removeLegacy = false } = {}) {
  const registry = new Map(Object.entries(values));
  const registers = { $INSTDIR: installDir };
  const frames = [];
  const active = () => frames.every((frame) => frame.enabled);
  const expand = (value) => value
    .replace(/\$\{(\w+)\}/g, (_, name) => {
      assert.ok(Object.hasOwn(definitions, name), `unknown define: ${name}`);
      return definitions[name];
    })
    .replace(/\$(?:INSTDIR|R\d+)\b/g, (name) => {
      assert.ok(Object.hasOwn(registers, name), `uninitialized register: ${name}`);
      return registers[name];
    });

  function registryInstruction(line) {
    const tokens = line.match(/"[^"]*"|'[^']*'|\S+/g);
    const [command, ...args] = tokens.map((token) => /^["']/.test(token) ? token.slice(1, -1) : token);
    assert.ok(["ReadRegStr", "WriteRegStr", "DeleteRegValue"].includes(command), `unsupported instruction: ${line}`);
    assert.equal(args.length, command === "DeleteRegValue" ? 3 : 4);
    const [root, key, name] = (command === "ReadRegStr" ? args.slice(1) : args).map(expand);
    assert.equal(root, "HKCU");
    assert.equal(key, runKey);
    if (!active()) return;
    if (command === "ReadRegStr") registers[args[0]] = registry.get(name) ?? "";
    if (command === "DeleteRegValue") registry.delete(name);
    if (command === "WriteRegStr") registry.set(name, expand(args[3]));
  }

  // Execute only the real PREINSTALL capture instruction, then optionally
  // model the legacy uninstaller removing its own value. No uninstall is run.
  const capture = instructions(pre)[0];
  assert.match(capture, /^ReadRegStr \$R7 HKCU "\$\{LIMITSCOPE_RUN_KEY\}" "\$\{LIMITSCOPE_OLD_RUN_NAME\}"$/);
  assert.ok(pre.indexOf(capture) < pre.indexOf("ExecWait"));
  registryInstruction(capture);
  if (removeLegacy) registry.delete(legacyName);

  for (const line of instructions(post)) {
    const condition = /^\$\{(If|OrIf)\} (\$R\d+) != ""$/.exec(line);
    if (condition) {
      const [, kind, register] = condition;
      const enabled = expand(register) !== "";
      if (kind === "If") frames.push({ enabled, bodyStarted: false });
      else {
        assert.ok(frames.length, "OrIf without If");
        const frame = frames.at(-1);
        assert.equal(frame.bodyStarted, false, "OrIf after conditional body");
        frame.enabled ||= enabled;
      }
    } else if (line === "${EndIf}") {
      assert.ok(frames.length, "EndIf without If");
      frames.pop();
    } else {
      for (const frame of frames) frame.bodyStarted = true;
      registryInstruction(line);
    }
  }
  assert.equal(frames.length, 0, "unclosed If");
  return registry;
}

const unrelated = { "Other App": '"C:\\Other\\app.exe"', "Rate Limits Backup": "preserve" };
const stale = '"C:\\Obsolete\\LimitScope.exe" --hidden';
const legacy = '"C:\\Legacy\\rate-limits.exe" --hidden';
for (const [name, values, enabled, options = {}] of [
  ["fresh/no keys", {}, false],
  ["legacy only", { [legacyName]: legacy }, true],
  ["legacy capture survives simulated legacy removal", { [legacyName]: legacy }, true, { removeLegacy: true }],
  ["stale same-brand key without legacy", { [currentName]: stale }, true],
  ["both keys", { [legacyName]: legacy, [currentName]: stale }, true],
  ["empty historical value is preserved during same-brand repair", { [legacyName]: "", [currentName]: stale }, true],
  ["disabled/opted-out (Run values absent)", {}, false],
  ["empty values do not enable autostart", { [legacyName]: "", [currentName]: "" }, false],
  ["same-brand target follows selected install directory", { [currentName]: stale }, true, { installDir: "D:\\Selected Path\\Rate Limits" }],
]) {
  test(`synthetic POSTINSTALL control flow: ${name}`, () => {
    const initial = { ...unrelated, ...values };
    const expected = new Map(Object.entries(initial));
    if (enabled) {
      if (values[legacyName]) expected.delete(legacyName);
      expected.set(currentName, `"${options.installDir ?? "C:\\Synthetic Install\\LimitScope"}\\${config.mainBinaryName}.exe" --hidden`);
    }
    const actual = syntheticPost(initial, options);
    assert.deepEqual(actual, expected);
    assert.deepEqual(syntheticPost(Object.fromEntries(actual), options), expected, "reconciliation is idempotent");
  });
}

test("static scope: no custom uninstall or rollback hook is introduced", () => {
  assert.doesNotMatch(hooks, /!macro NSIS_HOOK_(?:PRE|POST)UNINSTALL\b/);
  // Rollback to a historical installer and Tauri's generated uninstaller are
  // outside this parser; this assertion does not establish their behavior.
});
