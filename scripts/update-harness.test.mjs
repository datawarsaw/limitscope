import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import {
  HarnessError,
  buildSigningInvocation,
  createSecretRedactor,
  formatTauriFailure,
  keysReport,
  main,
  resolveKeyConfig,
  resolveKeyPassword,
} from "./update-harness-lib.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SCRIPT = path.join(REPO_ROOT, "scripts", "update-harness.mjs");
const STATE_DIR = path.join(REPO_ROOT, "state", "update-harness");

// Synthetic credentials only: every "secret" in this suite is an invented
// string, and every key fixture is a plain-text file with no key structure.
// No real key material, password, signing operation or Tauri CLI execution
// is involved — the runner is always injected (or the command fails before
// any child process would start).
const SYNTHETIC_PASSWORD = "synthetic-harness-passphrase-4f7b2d91";
const SYNTHETIC_README_PASSWORD = "synthetic-readme-password-11aa22bb";
const SYNTHETIC_KEY_BODY = "SYNTHETIC FIXTURE - NOT A KEY - DO NOT USE";

let workspace;
let home;
let keyCfg;

function captureIo() {
  const lines = [];
  return {
    lines,
    log: (...parts) => lines.push(parts.map(String).join(" ")),
    error: (...parts) => lines.push(parts.map(String).join(" ")),
  };
}

function printed(io) {
  return io.lines.join("\n");
}

function withoutPub(text) {
  // The public-key path legitimately contains the private-key path as a
  // substring (...\\limitscope.key.pub); compare with the .pub line removed.
  return text.split(keyCfg.pub).join("");
}

async function checkReportFor(homeDir) {
  const io = captureIo();
  const code = await main(["check"], {}, { io, home: homeDir });
  expect(code).toBe(1);
  return printed(io);
}

beforeAll(() => {
  workspace = mkdtempSync(path.join(tmpdir(), "limitscope-harness-tests-"));
  home = path.join(workspace, "home");
  keyCfg = resolveKeyConfig(home);
  mkdirSync(path.dirname(keyCfg.file), { recursive: true });
  writeFileSync(keyCfg.file, SYNTHETIC_KEY_BODY);
  writeFileSync(keyCfg.pub, "SYNTHETIC FIXTURE - NOT A PUBLIC KEY");
  // A README carrying a plaintext password, mirroring the legacy key-directory
  // convention: the harness must never consult it again.
  writeFileSync(
    path.join(path.dirname(keyCfg.file), "README.txt"),
    `Password: ${SYNTHETIC_README_PASSWORD}\n-----\nsynthetic notes\n`,
  );
});

afterAll(() => {
  if (workspace) rmSync(workspace, { recursive: true, force: true });
});

describe("secret redactor", () => {
  it("removes registered secrets from arbitrary text, case-insensitively", () => {
    const redactor = createSecretRedactor();
    redactor.register(SYNTHETIC_PASSWORD);
    redactor.register("C:\\Users\\op\\.tauri\\synthetic-updater\\synthetic.key");
    const redacted = redactor.redact(
      [
        "tauri signer sign -f c:\\users\\op\\.tauri\\SYNTHETIC-UPDATER\\synthetic.key",
        `-p ${SYNTHETIC_PASSWORD.toUpperCase()}`,
      ].join(" "),
    );
    expect(redacted).not.toMatch(/synthetic\.key/);
    expect(redacted).not.toMatch(/synthetic-harness-passphrase/i);
    expect(redacted).toContain("[redacted]");
  });

  it("masks the full private-key path before its directory prefix", () => {
    const redactor = createSecretRedactor();
    const dir = "C:\\Users\\op\\.tauri\\synthetic-updater";
    const file = `${dir}\\synthetic.key`;
    redactor.register(file);
    redactor.register(dir);
    const redacted = redactor.redact(`failed to read ${file}`);
    expect(redacted).not.toContain("synthetic-updater");
    expect(redacted).not.toContain("synthetic.key");
  });

  it("handles separator variants of registered paths", () => {
    const redactor = createSecretRedactor();
    redactor.register("C:\\Users\\op\\.tauri\\synthetic-updater\\synthetic.key");
    redactor.register("C:/Users/op/.tauri/synthetic-updater/synthetic.key");
    expect(
      redactor.redact("cannot open C:/Users/op/.tauri/synthetic-updater/synthetic.key"),
    ).not.toContain("synthetic-updater");
    expect(
      redactor.redact("cannot open C:\\Users\\op\\.tauri\\synthetic-updater\\synthetic.key"),
    ).not.toContain("synthetic-updater");
  });

  it("ignores empty values so output is not clobbered", () => {
    const redactor = createSecretRedactor();
    redactor.register("");
    expect(redactor.redact("plain output")).toBe("plain output");
  });
});

describe("key password resolution", () => {
  it("reads the password from the environment", () => {
    expect(resolveKeyPassword({ TAURI_SIGNING_PRIVATE_KEY_PASSWORD: SYNTHETIC_PASSWORD })).toBe(
      SYNTHETIC_PASSWORD,
    );
  });

  it("fails closed without an environment password and never points at on-disk storage", () => {
    let error;
    try {
      resolveKeyPassword({});
    } catch (caught) {
      error = caught;
    }
    expect(error).toBeInstanceOf(HarnessError);
    expect(error.message).toMatch(/TAURI_SIGNING_PRIVATE_KEY_PASSWORD/);
    expect(error.message.toLowerCase()).not.toContain("readme");
    expect(error.message).not.toContain(home);
  });
});

describe("key-directory README is no longer consulted", () => {
  it("does not resolve the password from a plaintext README next to the key", () => {
    let error;
    try {
      resolveKeyPassword({});
    } catch (caught) {
      error = caught;
    }
    expect(error).toBeInstanceOf(HarnessError);
    expect(error.message).not.toContain(SYNTHETIC_README_PASSWORD);
  });

  it("fails a sign request without disclosing the README path or its value", async () => {
    const io = captureIo();
    const code = await main(["sign", "installer.exe"], {}, { io, home });
    expect(code).toBe(1);
    const shown = printed(io);
    expect(shown).toMatch(/TAURI_SIGNING_PRIVATE_KEY_PASSWORD/);
    expect(shown).not.toContain(SYNTHETIC_README_PASSWORD);
    expect(shown.toLowerCase()).not.toContain("readme");
  });
});

describe("key configuration", () => {
  it("resolves candidates under the injected home directory", () => {
    expect(keyCfg.file).toBe(path.join(home, ".tauri", "limitscope-updater", "limitscope.key"));
    expect(keyCfg.pub).toBe(path.join(home, ".tauri", "limitscope-updater", "limitscope.key.pub"));
  });

  it("falls back to the first candidate when no key directory exists", () => {
    const empty = path.join(workspace, "no-keys-home");
    mkdirSync(empty, { recursive: true });
    expect(resolveKeyConfig(empty).file).toBe(
      path.join(empty, ".tauri", "limitscope-updater", "limitscope.key"),
    );
  });

  it("selects the rate-limits candidate when it exists", () => {
    const altHome = path.join(workspace, "alt-home");
    const altDir = path.join(altHome, ".tauri", "rate-limits-updater");
    mkdirSync(altDir, { recursive: true });
    // Plain-text fixture; not a key file in any structural sense.
    writeFileSync(path.join(altDir, "rate-limits.key"), SYNTHETIC_KEY_BODY);
    expect(resolveKeyConfig(altHome).file).toBe(path.join(altDir, "rate-limits.key"));
  });
});

describe("signing invocation", () => {
  it("keeps the password out of argv and passes it through the environment", () => {
    const invocation = buildSigningInvocation({
      keyFile: keyCfg.file,
      password: SYNTHETIC_PASSWORD,
      target: "installer.exe",
      baseEnv: { PATH: "C:\\tools" },
    });
    expect(invocation.args).toEqual(["signer", "sign", "-f", keyCfg.file, "installer.exe"]);
    expect(invocation.args.join(" ")).not.toContain(SYNTHETIC_PASSWORD);
    expect(invocation.args.join(" ")).not.toMatch(/-p /);
    expect(invocation.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD).toBe(SYNTHETIC_PASSWORD);
    expect(invocation.env.PATH).toBe("C:\\tools");
  });
});

describe("failed sign diagnostics are redacted", () => {
  it("strips the password and the private-key path from child output", () => {
    const redactor = createSecretRedactor();
    redactor.register(SYNTHETIC_PASSWORD);
    redactor.register(keyCfg.file);
    redactor.register(keyCfg.dir);
    const message = formatTauriFailure(
      {
        status: 1,
        stdout: `tauri signer sign -f ${keyCfg.file} -p ${SYNTHETIC_PASSWORD} installer.exe`,
        stderr: `error: failed to open ${keyCfg.file}: access denied`,
      },
      redactor,
    );
    expect(message).toMatch(/tauri signer sign failed/);
    expect(message).not.toContain(SYNTHETIC_PASSWORD);
    expect(message).not.toContain(keyCfg.file);
    expect(message).not.toContain(keyCfg.dir);
  });

  it("reports spawn-level failures without leaking secrets", () => {
    const redactor = createSecretRedactor();
    redactor.register(SYNTHETIC_PASSWORD);
    const message = formatTauriFailure({ error: new Error("spawn ENOENT") }, redactor);
    expect(message).toMatch(/spawn ENOENT/);
    expect(message).not.toContain(SYNTHETIC_PASSWORD);
  });

  it("routes the sign failure path through the redactor end to end", async () => {
    const io = captureIo();
    const spawn = () => ({
      status: 1,
      stdout: `signer invoked with -p ${SYNTHETIC_PASSWORD}`,
      // Forward-slash variant: main registers both separator spellings.
      stderr: `failed to read ${keyCfg.file.replaceAll("\\", "/")}`,
    });
    const code = await main(
      ["sign", "installer.exe"],
      { TAURI_SIGNING_PRIVATE_KEY_PASSWORD: SYNTHETIC_PASSWORD },
      { io, spawn, home },
    );
    expect(code).toBe(1);
    const shown = printed(io);
    expect(shown).toMatch(/tauri signer sign failed/);
    expect(shown).not.toContain(SYNTHETIC_PASSWORD);
    expect(shown).not.toContain("limitscope-updater");
  });
});

describe("successful command output is redacted", () => {
  it("never echoes the password even when it appears in user input", async () => {
    const io = captureIo();
    const spawn = () => ({ status: 0, stdout: "", stderr: "" });
    const code = await main(
      ["sign", `installer-${SYNTHETIC_PASSWORD}.exe`],
      { TAURI_SIGNING_PRIVATE_KEY_PASSWORD: SYNTHETIC_PASSWORD },
      { io, spawn, home },
    );
    expect(code).toBe(0);
    expect(printed(io)).toMatch(/signed:/);
    expect(printed(io)).not.toContain(SYNTHETIC_PASSWORD);
  });

  it("reports the password source without the password value", async () => {
    const io = captureIo();
    const code = await main(
      ["keys"],
      { TAURI_SIGNING_PRIVATE_KEY_PASSWORD: SYNTHETIC_PASSWORD },
      { io, home },
    );
    expect(code).toBe(0);
    const shown = printed(io);
    expect(shown).toContain("private key present: yes");
    expect(shown).toContain("password source: environment");
    expect(shown).toContain(keyCfg.pub);
    expect(shown).not.toContain(SYNTHETIC_PASSWORD);
    expect(withoutPub(shown)).not.toContain(keyCfg.file);
  });

  it("withholds the private-key path from the keys report", () => {
    const report = keysReport(keyCfg, {
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: SYNTHETIC_PASSWORD,
    }).join("\n");
    expect(report).toContain(keyCfg.pub);
    expect(report).not.toContain(SYNTHETIC_PASSWORD);
    expect(withoutPub(report)).not.toContain(keyCfg.file);
  });

  it("stages a prepared installer without leaking the password", async () => {
    // Build a fake installer for the synthetic version so stageInstaller runs
    // to completion; the injected runner mimics the signer contract by
    // writing an empty .sig next to the target.
    const targetRoot = path.join(REPO_ROOT, "src-tauri", "target");
    const nsisDir = path.join(targetRoot, "debug", "bundle", "nsis");
    const installerName = "LimitScope_9.9.9-synthetic_x64-setup.exe";
    const targetExisted = existsSync(targetRoot);
    const feedExisted = existsSync(path.join(STATE_DIR, "latest.json"));
    mkdirSync(nsisDir, { recursive: true });
    const installerFixture = path.join(nsisDir, installerName);
    writeFileSync(installerFixture, "SYNTHETIC INSTALLER FIXTURE");

    const io = captureIo();
    const spawn = (_command, args) => {
      writeFileSync(`${args[args.length - 1]}.sig`, "");
      return { status: 0, stdout: "", stderr: "" };
    };
    try {
      const code = await main(
        ["prepare", "--version", "9.9.9-synthetic", "--notes", "synthetic notes"],
        { TAURI_SIGNING_PRIVATE_KEY_PASSWORD: SYNTHETIC_PASSWORD },
        { io, spawn, home },
      );
      expect(code).toBe(0);
      expect(printed(io)).toMatch(/staged:/);
      expect(printed(io)).not.toContain(SYNTHETIC_PASSWORD);

      const feed = JSON.parse(readFileSync(path.join(STATE_DIR, "latest.json"), "utf8"));
      expect(feed.version).toBe("9.9.9-synthetic");
      expect(feed.platforms["windows-x86_64"].signature).toBe("");
      expect(feed.notes).toBe("synthetic notes");
    } finally {
      // Remove only artifacts this test created; never pre-existing state.
      rmSync(path.join(STATE_DIR, installerName), { force: true });
      rmSync(path.join(STATE_DIR, `${installerName}.sig`), { force: true });
      if (!feedExisted) rmSync(path.join(STATE_DIR, "latest.json"), { force: true });
      if (targetExisted) rmSync(installerFixture, { force: true });
      else rmSync(targetRoot, { recursive: true, force: true });
    }
  });
});

describe("check command diagnostics", () => {
  it("lists a withheld descriptor instead of the private-key path", async () => {
    const report = await checkReportFor(home);
    expect(report).toMatch(/harness incomplete/);
    expect(report).not.toContain(keyCfg.file);
    expect(report).not.toMatch(/\.tauri/);
  });

  it("withholds the private-key path when the key is missing entirely", async () => {
    const emptyHome = path.join(workspace, "empty-home");
    mkdirSync(emptyHome, { recursive: true });
    const report = await checkReportFor(emptyHome);
    expect(report).toMatch(/signing private key \(path withheld\)/);
    expect(report).not.toContain(emptyHome);
    expect(report).not.toMatch(/\.tauri/);
  });
});

describe("prepare fails closed before any signing", () => {
  it("fails when no installer was built and never spawns the signer", async () => {
    const io = captureIo();
    const spawn = () => {
      throw new Error("the signer must not be spawned");
    };
    const code = await main(["prepare", "--version", "0.0.0-no-installer"], {}, { io, spawn, home });
    expect(code).toBe(1);
    expect(printed(io)).toMatch(/no NSIS installer/);
  });
});

describe("CLI subprocess with an isolated synthetic home", () => {
  function runCli(commandArgs, extraEnv = {}) {
    return spawnSync(process.execPath, [SCRIPT, ...commandArgs], {
      cwd: REPO_ROOT,
      encoding: "utf8",
      env: {
        ...process.env,
        USERPROFILE: home,
        HOME: home,
        // Never let an operator's real password enter the test process.
        TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "",
        ...extraEnv,
      },
      timeout: 120_000,
      windowsHide: true,
    });
  }

  it("exits nonzero for sign without an environment password and ignores the README fallback", () => {
    const result = runCli(["sign", "installer.exe"]);
    expect(result.status).toBe(1);
    const shown = `${result.stdout ?? ""}${result.stderr ?? ""}`;
    expect(shown).toMatch(/TAURI_SIGNING_PRIVATE_KEY_PASSWORD/);
    expect(shown).not.toContain(SYNTHETIC_README_PASSWORD);
    expect(shown.toLowerCase()).not.toContain("readme");
  });

  it("prints key presence and the public-key location without the private-key path", () => {
    const result = runCli(["keys"], { TAURI_SIGNING_PRIVATE_KEY_PASSWORD: SYNTHETIC_PASSWORD });
    expect(result.status).toBe(0);
    const shown = result.stdout ?? "";
    expect(shown).toContain("private key present: yes");
    expect(shown).toContain("password source: environment");
    expect(shown).toContain(keyCfg.pub);
    expect(shown).not.toContain(SYNTHETIC_PASSWORD);
    expect(withoutPub(shown)).not.toContain(keyCfg.file);
  });

  it("reports an incomplete harness without the private-key path", () => {
    const emptyHome = path.join(workspace, "cli-empty-home");
    mkdirSync(emptyHome, { recursive: true });
    const result = runCli(["check"], { USERPROFILE: emptyHome, HOME: emptyHome });
    expect(result.status).toBe(1);
    const shown = `${result.stdout ?? ""}${result.stderr ?? ""}`;
    expect(shown).toMatch(/harness incomplete/);
    expect(shown).not.toContain(keyCfg.file);
    expect(shown).not.toContain(SYNTHETIC_PASSWORD);
  });

  it("keeps usage output free of secrets", () => {
    const noArgs = runCli([]);
    expect(noArgs.status).toBe(0);
    const unknown = runCli(["not-a-command"]);
    expect(unknown.status).toBe(1);
    const shown = `${noArgs.stdout ?? ""}${unknown.stdout ?? ""}${unknown.stderr ?? ""}`;
    expect(shown).not.toContain(SYNTHETIC_PASSWORD);
    expect(shown).not.toContain(keyCfg.file);
  });
});
