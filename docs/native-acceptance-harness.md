# LimitScope v0.6 Native Acceptance Harness

## 1. Overview & Purpose

The Native Acceptance Harness is a reusable Windows-native verification framework for **LimitScope**.
Its primary goal is to eliminate repeated manual smoke testing before the v0.6 release by automating native lifecycle, process, window, system tray, and persistence validations that standard unit tests cannot prove.

### Key Principles
- **No Visual Screenshot Automation**: Pixel comparisons across diverse Windows display scalings, HDR modes, and GPU compositors are inherently fragile. This harness replaces them with deterministic Windows OS observations.
- **Deterministic Observations**:
  - Process lifecycle and single-instance PID tracking via Win32.
  - Native window hierarchy enumeration (`EnumWindows`, window classes, styles, and geometry).
  - System tray icon discovery and Win32 popup menu (`#32768`) command interrogation via Windows UI Automation.
  - Window extended styles (`WS_EX_TOPMOST`) for pin/always-on-top assertions.
  - Safe, non-destructive user profile isolation with guaranteed automatic backup and restore.

---

## 2. Target Acceptance Flows (18 Automated Checks)

The harness validates 18 target lifecycle flows across four test suites:

| Flow | Test Identifier | Suite | Assertion & Verification Mechanism |
|---|---|---|---|
| **01** | `app-launch-success` | Smoke | Starts target binary, monitors process creation, verifies path matching and PID liveness after startup settle window. |
| **02** | `single-instance-count` | Smoke | Scans all running processes; proves exactly one instance of the application binary is running. |
| **03** | `second-launch-reuses-instance` | SingleInstance | Spawns a secondary process; verifies secondary launcher exits within settle window and original instance remains active. |
| **04** | `tray-icon-exists` | Smoke | Interrogates Windows taskbar (`Shell_TrayWnd`) and notification overflow flyout (`TopLevelWindowForOverflowXamlIsland`) for the named tray button. |
| **05** | `tray-commands-available` | Smoke | Right-clicks the tray icon, inspects the Win32 popup menu (`#32768`), and proves the existence of `Open`, `Show floating quota bar`, `Refresh`, and `Quit`. |
| **06** | `main-window-show-from-tray` | Smoke | Dispatches `WM_CLOSE` to main window, confirms window hides to tray while process remains alive, and restores visibility via tray command. |
| **07** | `floating-bar-show` | Floating | Invokes tray show command, verifies floating bar Tauri window (`WS_POPUP`, 520x64 default) is created and visible on desktop. |
| **08** | `floating-bar-hide` | Floating | Triggers floating bar hide action (via context menu or tray command); verifies window visibility transitions to hidden. |
| **09** | `floating-hide-not-disable` | Floating | Confirms product semantics: hiding the bar leaves the feature enabled, so the tray menu command remains ready to show it again. |
| **10** | `floating-bar-disabled-prevents-show` | Floating | Verifies disabled feature contract: when `floatingBarEnabled = false`, tray show action is disabled or prevented from exposing the bar. |
| **11** | `floating-bar-re-enable-restores` | Floating | Re-enables the floating bar via tray command, verifying visibility is restored and saved geometry is not lost. |
| **12** | `floating-position-persists` | Floating | Relocates floating bar via Win32 `SetWindowPos` to test coordinates (220, 140); verifies updated outer position. |
| **13** | `floating-pin-persists` | Floating | Inspects window extended style flags (`GetWindowLongW(GWL_EXSTYLE)`) to verify the pin preference (`WS_EX_TOPMOST`). |
| **14** | `refresh-no-duplicate-windows` | Floating | Triggers runtime Refresh via tray menu; verifies total native window count remains constant (no duplicate floating windows created). |
| **15** | `settings-no-duplicate-windows` | Floating | Cycles settings drawer state; verifies no duplicate native top-level windows or dialog handles are leaked. |
| **16** | `preferences-persist-restart` | Persistence | Verifies persisted settings and floating preference structures against test fixtures across process restart. |
| **17** | `quit-removes-process-cleanly` | Smoke | Triggers `Quit` command through the tray menu; monitors process termination; proves 0 zombie processes remain. |
| **18** | `relaunch-after-quit-succeeds` | Smoke | Relaunches application following clean quit; verifies new PID starts and stays responsive. |

---

## 3. What is Automated vs What Remains Manual

### Fully Automated by the Harness
- **Process and instance management**: Launch, PID binding, single-instance enforcement, rapid consecutive launch resilience, and clean termination.
- **Native window verification**: Enumeration of main window and floating bar window handles, visibility status, coordinates, dimensions, and window styles (`WS_EX_TOPMOST`).
- **System tray automation**: Opening the overflow flyout, discovering the LimitScope button, opening the popup menu, inspecting menu item names and enabled states, and activating menu items.
- **Floating bar lifecycle**: Showing, hiding, moving, and verifying window count stability during refresh cycles.
- **Non-destructive profile isolation**: Automatic backup of `%LOCALAPPDATA%\com.ratelimits.desktop` and `%APPDATA%\com.ratelimits.desktop` before test execution, and guaranteed restoration in a `finally` block upon completion.

### Manual / Human Checkpoints (Minimal)
- **Visual Brand Aesthetics**: Verifying the crispness of the Dock Tick logo on high-DPI monitors, theme rendering aesthetics (Graphite, Glass, OLED), and reset countdown pacing in the UI.
- **Live Provider Credentials**: Probing live network endpoints for live quota providers (Codex, Z.ai, OpenCode Go, Antigravity, Grok) with authenticated credentials.

---

## 4. Prerequisites

1. **Operating System**: Windows 10 or Windows 11 (64-bit).
2. **Shell**: Windows PowerShell 5.1 or PowerShell Core (pwsh 7+).
3. **Target Binary**: A built `LimitScope.exe` in `src-tauri/target/release` or an installed binary in `%LOCALAPPDATA%\Rate Limits\LimitScope.exe`.
4. **Permissions**: Standard user permissions (Administrator elevation is **not** required).
5. **UI Automation**: Built into Windows (`UIAutomationClient.dll`, `UIAutomationTypes.dll`).

---

## 5. How to Run

The primary entry point is:
```powershell
scripts/native-acceptance.ps1
```

### Run the Full Acceptance Suite (All 18 Flows)
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\native-acceptance.ps1 -Full -StopExisting
```

### Run Smoke & Core Lifecycle Only (Flows 1, 2, 4, 5, 6, 16, 17, 18)
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\native-acceptance.ps1 -Smoke -StopExisting
```

### Run Floating Bar Lifecycle Suite Only (Flows 7-15)
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\native-acceptance.ps1 -Floating -StopExisting
```

### Run Single-Instance Guardrails Only (Flow 3 + Rapid Launch Stress)
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\native-acceptance.ps1 -SingleInstance -StopExisting
```

### Testing an Installed Binary or Alternate Executable Path
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\native-acceptance.ps1 `
  -ExePath "C:\Program Files\LimitScope\LimitScope.exe" `
  -AppVersion "0.5.0" `
  -Full -StopExisting
```

---

## 6. Output Artifacts

Every run generates both human-readable console output and machine-readable artifacts in `artifacts/`:

### 1. Machine-Readable Result: `artifacts/native-acceptance-result.json`
```json
{
  "timestamp": "2026-09-30T02:25:17+02:00",
  "binary": "C:\\AI\\Token_Monitor\\src-tauri\\target\\release\\LimitScope.exe",
  "version": "0.5.0",
  "summary": {
    "total": 18,
    "pass": 18,
    "fail": 0,
    "humanRequired": 0
  },
  "tests": [
    {
      "id": "app-launch-success",
      "flow": 1,
      "name": "Application launches successfully",
      "durationMs": 4197,
      "status": "PASS",
      "detail": "Process PID 72896 running from C:\\AI\\Token_Monitor\\src-tauri\\target\\release\\LimitScope.exe"
    }
  ]
}
```

### 2. Detailed Run Log: `artifacts/native-acceptance-<timestamp>.log`
Contains timestamped diagnostic logs for each flow execution, window measurements, menu dump trees, and cleanup actions.

---

## 7. Expected Duration

- **Smoke Suite**: ~15–20 seconds
- **SingleInstance Suite**: ~15–20 seconds
- **Floating Bar Suite**: ~20–30 seconds
- **Full Suite**: ~40–55 seconds

---

## 8. Failure Interpretation & Troubleshooting

| Failure Symptom | Likely Cause | Resolution / Next Step |
|---|---|---|
| `Process exited prematurely during startup settle window` | Stale binary, missing webview2 runtime, or unhandled panics on startup. | Inspect `src-tauri` panic logs or run binary from console to check stdout. |
| `Single-instance breach` | `tauri-plugin-single-instance` did not bind to local socket or socket was held by orphaned process. | Ensure no zombie `LimitScope.exe` processes remain (`-StopExisting` flag). |
| `Tray button not found in Shell_TrayWnd or overflow area` | Tray icon failed to register with Windows Explorer within startup window. | Verify default window icon is bundled in `tauri.conf.json`; retry with higher `-StartupWaitSeconds`. |
| `Tray floating bar menu item not observed` | Tray menu schema drift or custom labels in experimental branch. | Compare tray items with documented names (`Show floating quota bar` / `Hide floating quota bar`). |
| `Window count changed ... on Refresh` | Memory leak or re-creation of webview window rather than webview reload. | Review Tauri window creation in `main.rs` and ensure single window ownership. |
