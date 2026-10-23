import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AppErrorBoundary } from "./AppErrorBoundary";
import App from "./App";
import { FloatingQuotaWindow } from "./components/FloatingQuotaWindow";
import { LocateRippleSurface } from "./components/LocateRippleSurface";
import { isTauriRuntime } from "./lib/floatingWindowChrome";
import { LOCATE_RIPPLE_WINDOW_PREFIX } from "./lib/locateRipple";
import { applyDevFixtureTheme } from "./dev/usageFixtures";
import "./styles.css";
import "./floating.css";

// Dev-only visual-acceptance hooks (`?theme=…`); inert in production builds.
if (import.meta.env.DEV && typeof window !== "undefined") {
  applyDevFixtureTheme(window.location.search);
}

function currentWindowLabel(): "main" | "floating-quota" {
  if (!isTauriRuntime()) return "main";
  return getCurrentWindow().label === "floating-quota" ? "floating-quota" : "main";
}

// The locate ripple's transient overlay window carries a generation-suffixed
// label (locate-ripple-N); any label under that prefix renders the wave pool.
function isRippleWindow(): boolean {
  if (isTauriRuntime()) {
    return getCurrentWindow().label.startsWith(LOCATE_RIPPLE_WINDOW_PREFIX);
  }
  return (
    import.meta.env.DEV &&
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("surface") ===
      "locate-ripple"
  );
}

// Dev-only visual-acceptance entry (`?surface=floating`); production builds
// evaluate this branch away and always use the native Tauri window label.
const windowLabel =
  import.meta.env.DEV &&
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("surface") === "floating"
    ? "floating-quota"
    : currentWindowLabel();
const rippleSurface = isRippleWindow();
if (windowLabel === "floating-quota" || rippleSurface) {
  document.documentElement.style.background = "transparent";
  document.body.style.background = "transparent";
}
if (windowLabel === "floating-quota") {
  document.documentElement.dataset.window = "floating";
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AppErrorBoundary>
      {rippleSurface ? (
        <LocateRippleSurface />
      ) : windowLabel === "floating-quota" ? (
        <FloatingQuotaWindow />
      ) : (
        <App />
      )}
    </AppErrorBoundary>
  </StrictMode>,
);
