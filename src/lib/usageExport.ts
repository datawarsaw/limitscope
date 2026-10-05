import { invoke } from "@tauri-apps/api/core";

export type UsageExportFormat = "csv" | "json";

export type UsageExportResult = {
  /**
   * "saved" — the file was written atomically.
   * "confirm-overwrite" — the destination exists; the app must re-invoke with
   * an explicit replace confirmation before Rust will touch it.
   */
  status: "saved" | "confirm-overwrite";
  fileName?: string;
};

export type UsageExportDestination = {
  directory: string;
  fileName: string;
  confirmOverwrite: boolean;
};

/**
 * Narrow frontend adapter for the Rust-owned usage history export.
 * The schema, serialization, destination validation, atomic write, and
 * sanitization all stay in Rust; this only forwards the user-chosen
 * destination and the explicit overwrite confirmation. The native Save As
 * dialog is deliberately not part of this path: it cannot settle on
 * destinations it refuses (read-only files), which used to leave the
 * export pending forever.
 */
export function exportUsageHistory(
  format: UsageExportFormat,
  destination: UsageExportDestination,
): Promise<UsageExportResult> {
  return invoke("export_usage_history", {
    format,
    directory: destination.directory,
    fileName: destination.fileName,
    confirmOverwrite: destination.confirmOverwrite,
  });
}

/**
 * Native folder picker for the export destination. Resolves null when the
 * user cancels, so every outcome settles.
 */
export function pickUsageExportDirectory(): Promise<string | null> {
  return invoke("pick_usage_export_directory");
}
