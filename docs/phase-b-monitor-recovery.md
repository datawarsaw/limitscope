# Phase B: Monitor Recovery & Locate Bar

LimitScope v0.8.15 baseline (`9ac6fe7`), branch `codex/monitor-recovery`.

## What this phase changes

The floating bar recovers its intended position after a display sleep/wake
and after a staggered multi-monitor reconnection, and the tray gains a
**Locate Floating Bar** action for a bar the user cannot find.

Nothing about the bar's size, its providers, or its wallet rendering is
touched: the recovery works on the *placement*, and the bar's measured
size is always read at runtime, never assumed.

## Root cause

The floating bar had **no display-topology event source at all**.

- `attachDockReanchor` watched `onScaleChanged` and `onFocusChanged`,
  and only for the *docked* placement. `onScaleChanged` reports the
  bar's *own* DPI change, so unplugging a *different* monitor produced no
  event at all. A focus change is not a topology signal.
- The persisted placement was a point (`x`/`y`) and a work-area-relative
  dock anchor. A point cannot say **which monitor** it belonged to, so a
  bar whose monitor went away had no way back when that monitor returned.
- The attach-time restore read the saved point and checked it against the
  monitors available *at that moment*. During a staggered reconnection
  that set is incomplete, so the check was made against a half-arrived
  topology.
- The saved placement was written back from live positions on several
  paths, so a temporary fallback position could overwrite the intended
  one before the real monitor came back.

The net effect: after a display sleep with three monitors, the bar could
be left on the wrong monitor, off-screen, or hidden, and nothing in the
codebase noticed or recovered.

## The mechanism

### 1. A native topology observer (`src-tauri/src/floating_monitor.rs`)

Windows broadcasts `WM_DISPLAYCHANGE`, `WM_DEVICECHANGE`,
`WM_SETTINGCHANGE` and `WM_POWERBROADCAST` to every top-level window.
None of them is exposed by the webview's window API, so a small Rust
window subclass observes them and emits
`floating://monitor-topology` with one of four changes: `display`,
`device`, `workarea`, `resume`.

The subclass id is distinct from the drag lifecycle's, so the two
observations on the same window never remove each other. The observer
writes nothing; it only reports.

### 2. A persisted monitor identity

`FloatingQuotaPrefs` gains a `monitor` field: the Windows device name
(`\\.\DISPLAY1`) plus the work-area geometry as a fallback. The
device name is the durable key — Windows renumbers display devices
freely, but a work-area coordinate is not stable across a reconnection.

The identity is written **only by user-intent paths** (a settled drag, a
dock, an undock). The automatic recovery path never writes it. That
separation is what guarantees a temporary fallback can never overwrite
the intended monitor.

### 3. A pure reconciler (`src/lib/floatingMonitorRecovery.ts`)

`reconcilePlacement` decides where the bar should be, given the
persisted intent, the monitors that are currently available, the window's
measured size and the preferred monitor identity. It returns a *decision*
(`none` / `move` / `unplaceable`) and never mutates anything.

The rules:

| Situation | Result |
|---|---|
| Saved point still overlaps a live work area | Stay there |
| Saved point is off every live work area, preferred monitor present | Move back to the saved point |
| Saved point is off every live work area, preferred monitor gone | Move to the nearest live monitor (a **fallback**, not persisted) |
| Docked, preferred monitor present | Re-derive the anchor on the preferred monitor |
| Docked, preferred monitor gone | Anchor on the monitor the window is on |
| No monitors at all | `unplaceable` — the tray locator is the way out |

The docked placement reuses the existing `clampDockAnchor` and
`clampWindowBoundsToWorkArea` helpers, so it can never disagree with
the existing anchoring path.

### 4. Bounded reconciliation (`attachMonitorRecovery`)

Three monitors reconnecting at slightly different moments produce a
burst of events. Reconciling on the first one would move the bar to a
fallback and then move it again.

The reconciler therefore waits for the topology to go quiet for
`TOPOLOGY_SETTLE_MS` (400 ms), bounded by `MAX_TOPOLOGY_WAIT_MS`
(3.2 s) so a flapping topology cannot strand the bar. This is
event-driven with one bounded timer — no polling, no background service.

An in-flight user drag owns the placement: recovery is suppressed for
the duration of the gesture, so it never fights an intentional drag.

### 5. The tray locator

The tray gains **Locate Floating Bar** (`tray://locate-floating`). It:

1. shows the existing floating window (no second window is ever
   created, and no restart is required),
2. runs the *same* reconciler the automatic recovery uses, so the tray
   action and the automatic path can never disagree,
3. pulses the bar with a brief outline (`fq-locate-pulse`, 3 × 400 ms,
   disabled under `prefers-reduced-motion`) so a bar that reappears on
   one of three monitors is findable.

## Files

| File | Change |
|---|---|
| `src-tauri/src/floating_monitor.rs` | New — the native topology observer |
| `src-tauri/src/main.rs` | Registers the two commands, adds the tray item |
| `src/lib/floatingMonitorRecovery.ts` | New — the pure reconciler |
| `src/lib/floatingMonitorIdentity.ts` | New — the persisted monitor identity |
| `src/lib/floatingWindowChrome.ts` | The recovery controller, the locator, the identity recording |
| `src/lib/floatingWindowPrefs.ts` | The `monitor` pref field |
| `src/components/FloatingQuotaWindow.tsx` | Attaches the observer, owns the pulse |
| `src/components/FloatingQuotaBar.tsx` | The `locatePulse` marker |
| `src/floating.css` | The pulse animation |

## Validation

- `src/lib/floatingMonitorRecovery.test.ts` — 28 pure tests: staggered
  three-monitor reconnection, temporary loss of the preferred monitor,
  its restoration, DPI/work-area changes, drag versus recovery, the
  settle window.
- `src/lib/floatingMonitorChrome.test.ts` — 12 integration tests: the
  observer wiring, the recovery effect on the live window, the
  "never overwrite the saved placement" rule, the drag suppression, the
  settle behaviour, and the tray locator.
- `src/lib/floatingMonitorIdentity.test.ts` — 14 tests on the identity
  parsing and the prefs round-trip.
- `src/floating.css.test.ts` — the locate pulse, including the
  reduced-motion ordering.
- `src-tauri/src/floating_monitor.rs` — 4 tests on the message
  classification and the wire contract.

Full suite: 1212 frontend tests, 901 Rust tests, typecheck and build
pass.

## Manual acceptance still required

The native Windows display events cannot be simulated. The following must
still be checked on a real three-monitor machine:

1. **Sleep/wake with all three monitors.** The bar returns to the
   monitor it was on, at the same place.
2. **Unplug the monitor the bar is on.** The bar moves to a live monitor
   and stays *findable*; unplugging it again is not an error.
3. **Replug it.** The bar returns to the original monitor and position.
4. **Staggered reconnection** (the monitor comes back a second or two
   after the others). The bar does not flicker between monitors.
5. **A display scale change** (Windows display settings → scale). The
   docked bar re-anchors proportionally.
6. **Locate Floating Bar** from the tray with the bar off-screen: the bar
   becomes visible, lands on a screen, and pulses.

