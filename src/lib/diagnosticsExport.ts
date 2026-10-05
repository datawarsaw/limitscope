import { invoke } from "@tauri-apps/api/core";
import type { Settings } from "./settings";

export type DiagnosticsExportResult = {
  status: "saved" | "cancelled";
  fileName?: string;
};

/**
 * Narrow frontend adapter for the Rust-owned diagnostics command. The
 * bundle schema, runtime reconstruction, privacy filtering, save dialog, and file
 * write all stay in Rust; this only forwards explicitly safe preferences.
 */
export function exportDiagnostics(
  settings: Settings,
): Promise<DiagnosticsExportResult> {
  return invoke("export_diagnostics", {
    settings: {
      theme: settings.theme,
      launchAtStartup: settings.launchAtStartup,
      notificationEnabled: settings.quotaNotifications,
    },
  });
}
