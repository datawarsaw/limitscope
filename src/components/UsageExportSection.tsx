import { useEffect, useRef, useState } from "react";
import type {
  UsageExportFormat,
  UsageExportResult,
} from "../lib/usageExport";

/**
 * The "Usage data" settings row: the Export CSV / Export JSON triggers plus
 * the in-place destination chooser that replaced the native Save As dialog.
 *
 * The chooser follows the LocalDataSection pattern — an in-place
 * confirmation instead of a modal, focus moved into it and back to the
 * trigger that opened it, and every outcome reported in words. It exists
 * because the native save dialog cannot settle on destinations it refuses
 * (a read-only file leaves it open forever); here the destination is chosen
 * in two always-settling steps (folder picker, file name) and an existing
 * file is replaced only after an explicit in-app confirmation.
 *
 * Outcome text (saved / failure) stays with the app: the section calls
 * `onExport` and renders nothing about results, so the drawer's existing
 * role="status" / role="alert" lines remain the single outcome surface.
 */
export type UsageExportRequest = {
  format: UsageExportFormat;
  directory: string;
  fileName: string;
  confirmOverwrite: boolean;
};

export type UsageExportSectionProps = {
  onPickDirectory: () => Promise<string | null>;
  onExport: (request: UsageExportRequest) => Promise<UsageExportResult>;
  /** The format whose invoke is in flight, or null when idle. */
  exporting: UsageExportFormat | null;
  onOutcomeDismiss: () => void;
  onSaved: (fileName?: string) => void;
};

const EXPORT_EXTENSION: Record<UsageExportFormat, string> = {
  csv: "csv",
  json: "json",
};

function defaultFileName(format: UsageExportFormat): string {
  const stamp = new Date()
    .toISOString()
    .replace(/[-:]/g, "")
    .replace(/\.\d{3}Z$/, "Z");
  return `limitscope-usage-${stamp}.${EXPORT_EXTENSION[format]}`;
}

export function UsageExportSection({
  onPickDirectory,
  onExport,
  exporting,
  onOutcomeDismiss,
  onSaved,
}: UsageExportSectionProps) {
  const [choosing, setChoosing] = useState<UsageExportFormat | null>(null);
  const [directory, setDirectory] = useState<string | null>(null);
  const [fileName, setFileName] = useState<string>("");
  const [picking, setPicking] = useState(false);
  // An existing destination was reported by Rust and not yet answered.
  const [awaitingReplace, setAwaitingReplace] = useState(false);
  const folderButtonRef = useRef<HTMLButtonElement>(null);
  const replaceButtonRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (choosing !== null && !awaitingReplace) folderButtonRef.current?.focus();
  }, [choosing, awaitingReplace]);

  useEffect(() => {
    if (awaitingReplace) replaceButtonRef.current?.focus();
  }, [awaitingReplace]);

  const closeChooser = () => {
    setChoosing(null);
    setDirectory(null);
    setAwaitingReplace(false);
    triggerRef.current?.focus();
  };

  const openChooser = (format: UsageExportFormat) => {
    onOutcomeDismiss();
    setChoosing(format);
    setDirectory(null);
    setFileName(defaultFileName(format));
    setAwaitingReplace(false);
  };

  const pickDirectory = async () => {
    setPicking(true);
    try {
      const picked = await onPickDirectory();
      if (picked) {
        setDirectory(picked);
        setAwaitingReplace(false);
      }
    } finally {
      setPicking(false);
      folderButtonRef.current?.focus();
    }
  };

  const save = async (confirmOverwrite: boolean) => {
    if (choosing === null || directory === null) return;
    const format = choosing;
    try {
      const result = await onExport({
        format,
        directory,
        fileName,
        confirmOverwrite,
      });
      if (result.status === "confirm-overwrite") {
        setAwaitingReplace(true);
        return;
      }
      onSaved(result.fileName);
      closeChooser();
    } catch {
      // The app reports the sanitized failure through its role="alert"
      // line; the chooser stays open so the destination can be corrected.
      setAwaitingReplace(false);
    }
  };

  const cancelReplace = () => {
    // Declining the replace is a plain cancel: no outcome, no error.
    onOutcomeDismiss();
    closeChooser();
  };

  return (
    <div className="setting-row utility-action">
      <span className="setting-copy">
        <span className="setting-label">Usage data</span>
        <span className="setting-note">Retained observation history</span>
      </span>
      {choosing === null ? (
        <div className="usage-export-actions">
          <button
            type="button"
            className="clear-history-btn"
            ref={triggerRef}
            onClick={() => openChooser("csv")}
            disabled={exporting !== null}
          >
            {exporting === "csv" ? "Exporting..." : "Export CSV"}
          </button>
          <button
            type="button"
            className="clear-history-btn"
            onClick={() => openChooser("json")}
            disabled={exporting !== null}
          >
            {exporting === "json" ? "Exporting..." : "Export JSON"}
          </button>
        </div>
      ) : (
        <div
          className="usage-export-panel"
          role="group"
          aria-label={`Export usage data (${choosing.toUpperCase()})`}
        >
          {awaitingReplace ? (
            <>
              <p className="setting-note">
                {fileName} already exists in the chosen folder.
              </p>
              <div className="local-data-confirm-actions">
                <button
                  type="button"
                  className="local-data-cancel-btn"
                  onClick={cancelReplace}
                  disabled={exporting !== null}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="local-data-confirm-btn"
                  ref={replaceButtonRef}
                  onClick={() => void save(true)}
                  disabled={exporting !== null}
                >
                  {exporting === choosing ? "Replacing\u2026" : "Replace"}
                </button>
              </div>
            </>
          ) : (
            <>
              <button
                type="button"
                className="clear-history-btn"
                ref={folderButtonRef}
                onClick={() => void pickDirectory()}
                disabled={picking || exporting !== null}
              >
                {picking ? "Choosing\u2026" : "Choose folder\u2026"}
              </button>
              <span className="usage-export-folder" title={directory ?? ""}>
                {directory ?? "No folder chosen"}
              </span>
              <label className="usage-export-name-row">
                <span className="setting-note">File name</span>
                <input
                  type="text"
                  className="usage-export-name-input"
                  value={fileName}
                  onChange={(event) => setFileName(event.target.value)}
                  disabled={picking || exporting !== null}
                  spellCheck={false}
                />
              </label>
              <div className="local-data-confirm-actions">
                <button
                  type="button"
                  className="local-data-cancel-btn"
                  onClick={() => {
                    onOutcomeDismiss();
                    closeChooser();
                  }}
                  disabled={exporting !== null}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="local-data-confirm-btn usage-export-save-btn"
                  onClick={() => void save(false)}
                  disabled={
                    directory === null ||
                    fileName.trim() === "" ||
                    picking ||
                    exporting !== null
                  }
                  title={
                    directory === null
                      ? "Choose a folder first"
                      : undefined
                  }
                >
                  {exporting === choosing ? "Saving\u2026" : "Save"}
                </button>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
