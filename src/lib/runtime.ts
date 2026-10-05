/**
 * Runtime environment helpers shared by hooks that talk to Tauri commands.
 */

/**
 * True when the UI runs inside the Tauri WebView (the installed desktop app
 * or `tauri dev`); false in a plain browser (`npm run dev`), where plugin
 * commands are unavailable and callers must degrade gracefully.
 */
export function isRunningInTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
