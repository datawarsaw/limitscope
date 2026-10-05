import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import {
  exportUsageHistory,
  pickUsageExportDirectory,
} from "./usageExport";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn().mockImplementation((_cmd: string, args: { format: string }) => {
    return Promise.resolve({
      status: "saved",
      fileName: `limitscope-usage-20260930T120000Z.${args.format}`,
    });
  }),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

describe("usage export adapter", () => {
  it("forwards the destination and overwrite confirmation for csv", async () => {
    const result = await exportUsageHistory("csv", {
      directory: "C:\\Exports",
      fileName: "limitscope-usage-20260930T120000Z.csv",
      confirmOverwrite: false,
    });
    expect(mocks.invoke).toHaveBeenCalledWith("export_usage_history", {
      format: "csv",
      directory: "C:\\Exports",
      fileName: "limitscope-usage-20260930T120000Z.csv",
      confirmOverwrite: false,
    });
    expect(result.status).toBe("saved");
    expect(result.fileName).toBe("limitscope-usage-20260930T120000Z.csv");
  });

  it("forwards an explicit replace confirmation for json", async () => {
    mocks.invoke.mockResolvedValueOnce({ status: "saved" });
    const result = await exportUsageHistory("json", {
      directory: "C:\\Exports",
      fileName: "existing.json",
      confirmOverwrite: true,
    });
    expect(mocks.invoke).toHaveBeenCalledWith("export_usage_history", {
      format: "json",
      directory: "C:\\Exports",
      fileName: "existing.json",
      confirmOverwrite: true,
    });
    expect(result.status).toBe("saved");
  });

  it("surfaces the Rust confirm-overwrite receipt untouched", async () => {
    mocks.invoke.mockResolvedValueOnce({
      status: "confirm-overwrite",
      fileName: "existing.csv",
    });
    const result = await exportUsageHistory("csv", {
      directory: "C:\\Exports",
      fileName: "existing.csv",
      confirmOverwrite: false,
    });
    expect(result.status).toBe("confirm-overwrite");
    expect(result.fileName).toBe("existing.csv");
  });

  it("invokes the always-settling folder picker", async () => {
    mocks.invoke.mockResolvedValueOnce("C:\\Exports");
    await expect(pickUsageExportDirectory()).resolves.toBe("C:\\Exports");
    expect(mocks.invoke).toHaveBeenCalledWith("pick_usage_export_directory");

    mocks.invoke.mockResolvedValueOnce(null);
    await expect(pickUsageExportDirectory()).resolves.toBeNull();
  });

  it("never reconstructs history or payloads in the frontend", () => {
    const source = readFileSync(
      fileURLToPath(new URL("./usageExport.ts", import.meta.url)),
      "utf8",
    );
    expect(source).not.toMatch(/schemaVersion|gapSemantics|rangeCovered/i);
    expect(source).not.toMatch(/JSON\.stringify|toCsv|escape/i);
  });

  it("validates golden fixture equivalence between CSV and JSON", () => {
    const repoRoot = join(import.meta.dirname, "../..");
    const csvContent = readFileSync(
      join(repoRoot, "fixtures/usage-export/golden.csv"),
      "utf8",
    );
    const jsonContent = readFileSync(
      join(repoRoot, "fixtures/usage-export/golden.json"),
      "utf8",
    );

    const json = JSON.parse(jsonContent);
    expect(json.schemaVersion).toBe(1);
    expect(json.kind).toBe("limitscope-usage-export");
    expect(json.rows.length).toBeGreaterThan(0);

    // Simple RFC 4180 CSV parser for equivalence check
    const lines = csvContent.trimEnd().split("\r\n");
    const header = lines[0].split(",");
    expect(header).toEqual([
      "providerId",
      "providerName",
      "account",
      "windowLabel",
      "usedPercent",
      "observedAt",
      "resetAt",
      "resolution",
    ]);

    const dataLines = lines.slice(1);
    expect(dataLines.length).toBe(json.rows.length);

    for (let i = 0; i < dataLines.length; i++) {
      const jsonRow = json.rows[i];
      const rawLine = dataLines[i];
      // Check that raw line contains the providerId and observedAt
      expect(rawLine).toContain(jsonRow.providerId);
      expect(rawLine).toContain(jsonRow.observedAt);
      expect(rawLine).toContain(jsonRow.resolution);
      if (jsonRow.providerName) {
        expect(rawLine).toContain(jsonRow.providerName);
      }
      if (jsonRow.account) {
        expect(rawLine).toContain(jsonRow.account);
      }
      if (jsonRow.resetAt) {
        expect(rawLine).toContain(jsonRow.resetAt);
      }
    }
  });
});
