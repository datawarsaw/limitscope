import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { isRunningInTauri } from "../lib/runtime";
import {
  formatUpdateError,
  isUpdateRelevant,
  sanitizeReleaseDate,
  sanitizeReleaseNotes,
  type ReleaseNote,
  type SafeUpdateError,
  type UpdatePhase,
} from "../lib/updater";

/**
 * In-app update flow (manual check is the primary path; a silent startup
 * check only surfaces an "update available" indicator — never a modal and
 * never an automatic install).
 *
 * Installation follows the official Tauri updater flow: `downloadAndInstall`
 * verifies the minisign signature, then on Windows launches the NSIS
 * installer in passive mode and exits this process; the installer restarts
 * the app when done, so the promise below normally never resolves.
 */
export function useUpdater() {
  const [phase, setPhase] = useState<UpdatePhase>("idle");
  const [update, setUpdate] = useState<ReleaseNote | null>(null);
  const [error, setError] = useState<SafeUpdateError | null>(null);
  const [currentVersion, setCurrentVersion] = useState<string | null>(null);
  const updateRef = useRef<Update | null>(null);

  useEffect(() => {
    if (!isRunningInTauri()) return;
    let cancelled = false;
    getVersion()
      .then((version) => {
        if (!cancelled) setCurrentVersion(version);
      })
      .catch(() => {
        // The label stays "—" outside the desktop app; nothing to reconcile.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const releaseHeldUpdate = useCallback(() => {
    const held = updateRef.current;
    updateRef.current = null;
    if (held) {
      // Frees the Rust-side resource; download bytes are not held yet.
      void held.close().catch(() => {});
    }
  }, []);

  const runCheck = useCallback(
    async (source: "manual" | "startup") => {
      if (!isRunningInTauri()) {
        if (source === "manual") {
          setError({
            message: "Update checks are only available in the installed app.",
            detail: "not running inside Tauri",
          });
          setPhase("error");
        }
        return;
      }
      if (source === "manual") {
        setError(null);
        setPhase("checking");
      }
      try {
        const found = await check();
        if (updateRef.current && updateRef.current !== found) {
          releaseHeldUpdate();
        }
        if (found && isUpdateRelevant(found.version, found.currentVersion)) {
          updateRef.current = found;
          setUpdate({
            version: found.version,
            notes: sanitizeReleaseNotes(found.body),
            date: sanitizeReleaseDate(found.date),
          });
          setPhase("available");
        } else {
          // A returned-but-not-offered release (equal or older version)
          // still holds a Rust-side resource; close it explicitly.
          if (found) void found.close().catch(() => {});
          setUpdate(null);
          if (source === "manual") setPhase("upToDate");
        }
      } catch (caught) {
        releaseHeldUpdate();
        setUpdate(null);
        if (source === "manual") {
          setError(formatUpdateError(caught));
          setPhase("error");
        }
        // Startup failures stay silent: no banners, no error state.
      }
    },
    [releaseHeldUpdate],
  );

  // Silent startup check: surfaces the indicator only; failures are ignored.
  useEffect(() => {
    void runCheck("startup");
  }, [runCheck]);

  const checkForUpdates = useCallback(() => runCheck("manual"), [runCheck]);

  const dismissUpdate = useCallback(() => {
    releaseHeldUpdate();
    setUpdate(null);
    setError(null);
    setPhase("idle");
  }, [releaseHeldUpdate]);

  const installUpdate = useCallback(async () => {
    const held = updateRef.current;
    if (!held) return;
    setError(null);
    setPhase("downloading");
    try {
      // On Windows the process exits inside this call; the NSIS installer
      // (passive mode) restarts the app once the update is in place.
      await held.downloadAndInstall();
      setPhase("installing");
      await relaunch();
    } catch (caught) {
      setError(formatUpdateError(caught));
      setPhase("error");
    }
  }, []);

  return {
    phase,
    update,
    error,
    currentVersion,
    checkForUpdates,
    installUpdate,
    dismissUpdate,
  };
}
