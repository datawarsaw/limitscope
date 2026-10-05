# Glass theme & native window transparency — investigation

Status: **investigation only, no shell change shipped** (feature/themes-reset-countdown).

The Glass theme ships as a **frontend-only** surface treatment: translucent cards with
restrained backdrop blur, composited over the window's own opaque background gradient.
This document records what real desktop-level transparency (Mica/Acrylic behind the
whole window) would require in the Tauri shell, and why it was deferred.

## Current shell state

- `src-tauri/tauri.conf.json`: main window is opaque — `backgroundColor: "#161619"`,
  no `transparent`, no `windowEffects`. Created `visible: false`, shown by
  `show_main_window()` from setup, the tray, and the single-instance handler.
- Tray, close-to-tray (`CloseRequested → hide`), and single-instance focus are the
  paths a transparency change must not disturb.

## What native transparency would require

1. `tauri.conf.json` — main window:
   - `"transparent": true`
   - `"windowEffects": { "effects": ["mica"] }` (or `"acrylic"`; `mica` follows the
     system dark/light preference)
   - remove/adjust `backgroundColor` so it does not paint over the effect
2. Frontend: when the native effect is active, the Glass theme must set
   `html`/`body` backgrounds to transparent — otherwise the WebView paints an opaque
   rectangle and the effect is invisible. The CSS gradient shipped with Glass would
   become the **fallback layer** instead of the primary surface.
3. Runtime alternative (least risky, config-neutral): leave the window opaque by
   default and call `setEffects()/clearEffects()` on the `WebviewWindow` from the
   frontend only while theme = glass. Needs verification against the installed
   `@tauri-apps/api` version before prototyping.

## Known risks (why it is deferred)

- **Resize/drag flicker**: Mica/Acrylic backdrops are documented as having bad
  performance while resizing, and backdrop redraw flicker is a long-standing
  WinUI/WebView2-family issue.
- **Startup flash**: WebView2 initializes after the window shows; misconfigured
  transparency is a known source of white/black flashes. This app already mitigates
  the generic case (`visible: false` + show after setup), but the interplay with a
  transparent surface is untested.
- **OS/version dependence**: Mica requires Windows 11; on Windows 10 a
  `transparent: true` window without a working effect degrades to (at best) plain
  transparency and (at worst) an unreadable surface. The CSS gradient fallback
  remains mandatory either way.
- **WebView2 runtime dependence**: transparency behavior changed around WebView2
  Runtime 101+; behavior varies with the evergreen runtime installed on user
  machines.
- **Untested tray interactions**: show/hide from tray, close-to-tray, and the
  single-instance "focus existing window" path are exactly the flows where
  transparent-window repaint artifacts (stale frames, black flashes on `show()`)
  have been reported historically.

## Recommendation

Keep the window opaque for v0.3.0 and ship Glass as implemented (in-window
translucency). If native transparency is pursued later:

1. Prototype with the runtime `setEffects()` path behind the Glass theme only —
   no global config change, graphite/OLED users unaffected.
2. Test matrix before merging: tray show/hide, close-to-tray, single-instance
   focus, autostart-hidden launch, resize/drag, Windows 10 and 11, light/dark
   system mode.
3. Keep the CSS gradient fallback so Glass stays readable when effects are
   unavailable or disabled.

## References

- Tauri 2 window config (`transparent`, `windowEffects`):
  <https://v2.tauri.app/reference/config/>
- Practitioner write-up on Tauri v2 window transparency on Windows and the
  "invisible app" pitfall (dev.to, Mar 2026: "How I built an AI-powered Git
  context menu for Windows").
