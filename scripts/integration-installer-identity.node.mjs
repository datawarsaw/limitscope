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
