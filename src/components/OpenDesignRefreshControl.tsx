import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** The shared runtime publishes the result to both windows. No mount read. */
export function OpenDesignRefreshControl() {
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  async function refresh() {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError(false);
    try {
      await invoke("refresh_opendesign_usage");
    } catch {
      setError(true);
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }
  return (
    <div className="opendesign-control opendesign-note">
      <p>Manual refresh only. Open Plan &amp; balance in Chrome and sign in, then Refresh. No automatic browser reads.</p>
      <button type="button" className="opendesign-refresh" aria-label="Refresh OpenDesign usage" aria-disabled={busy || undefined} aria-busy={busy || undefined} onClick={() => void refresh()}>
        {busy ? "Refreshing…" : "Refresh"}
      </button>
      {error ? <p role="alert">Couldn't refresh OpenDesign. The last observation is unchanged.</p> : null}
    </div>
  );
}
