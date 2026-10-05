import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { exportDiagnostics } from "./diagnosticsExport";
import { DEFAULT_SETTINGS } from "./settings";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue({
    status: "saved",
    fileName: "runtime-diagnostics-20260929T120000Z.json",
  }),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

describe("diagnostics export adapter", () => {
  it("invokes the Rust command with only safe settings fields", async () => {
    const result = await exportDiagnostics({
      ...DEFAULT_SETTINGS,
      theme: "glass",
      launchAtStartup: true,
      quotaNotifications: true,
    });

    expect(mocks.invoke).toHaveBeenCalledWith("export_diagnostics", {
      settings: {
        theme: "glass",
        launchAtStartup: true,
        notificationEnabled: true,
      },
    });
    expect(result.fileName).toBe(
      "runtime-diagnostics-20260929T120000Z.json",
    );
  });

  it("never reconstructs diagnostics in the frontend", () => {
    const source = readFileSync(
      fileURLToPath(new URL("./diagnosticsExport.ts", import.meta.url)),
      "utf8",
    );
    expect(source).not.toMatch(/schemaVersion|generatedAt|providers|history/i);
    expect(source).not.toMatch(/JSON\.stringify|sanitize|redact/i);
  });
});
