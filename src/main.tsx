import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AppErrorBoundary } from "./AppErrorBoundary";
import App from "./App";
import { FloatingQuotaWindow } from "./components/FloatingQuotaWindow";
import { isTauriRuntime } from "./lib/floatingWindowChrome";
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

// Dev-only visual-acceptance entry (`?surface=floating`); production builds
// evaluate this branch away and always use the native Tauri window label.
const windowLabel =
  import.meta.env.DEV &&
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("surface") === "floating"
    ? "floating-quota"
    : currentWindowLabel();
if (windowLabel === "floating-quota") {
  document.documentElement.dataset.window = "floating";
  document.documentElement.style.background = "transparent";
  document.body.style.background = "transparent";
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AppErrorBoundary>
      {windowLabel === "floating-quota" ? <FloatingQuotaWindow /> : <App />}
    </AppErrorBoundary>
  </StrictMode>,
);
