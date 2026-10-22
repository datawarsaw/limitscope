//! Native observation of display-topology changes for the floating bar.
//!
//! The bar's placement depends on the monitor topology, but the windowing
//! layer that owns the webview exposes no event for it. A monitor that is
//! plugged or unplugged *elsewhere* never moves or rescales the bar, so no
//! window event fires: `onScaleChanged` only reports the bar's own DPI
//! change, and a focus change is not a topology signal. Windows does
//! broadcast WM_DISPLAYCHANGE, WM_DEVICECHANGE, WM_SETTINGCHANGE and
//! WM_POWERBROADCAST to every top-level window, however. This module
//! observes those on the floating window and reports them, so the webview —
//! which owns the placement — can reconcile it.
//!
//! Observation only: no placement policy lives here, and nothing is
//! written to the window.

use serde::Serialize;

pub const EVENT: &str = "floating://monitor-topology";

/// The kind of topology change the platform reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    /// WM_DISPLAYCHANGE: the display resolution or topology changed.
    Display,
    /// WM_DEVICECHANGE / DBT_DEVNODES_CHANGED: a display device was
    /// attached or removed.
    Device,
    /// WM_SETTINGCHANGE / SPI_SETWORKAREA: a work area changed (a taskbar
    /// moved or resized).
    WorkArea,
    /// WM_POWERBROADCAST: the system resumed from sleep, so the displays
    /// are about to come back.
    Resume,
}

/// One observed topology change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TopologyEvent {
    pub change: Change,
}

#[cfg(windows)]
fn classify(message: u32, wparam: usize) -> Option<Change> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    match message {
        WM_DISPLAYCHANGE => Some(Change::Display),
        WM_DEVICECHANGE => (wparam as u32 == DBT_DEVNODES_CHANGED).then_some(Change::Device),
        WM_SETTINGCHANGE => {
            (wparam as u32 == SPI_SETWORKAREA as u32).then_some(Change::WorkArea)
        }
        WM_POWERBROADCAST => match wparam as u32 {
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(Change::Resume),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(not(windows))]
fn classify(_message: u32, _wparam: usize) -> Option<Change> {
    None
}

/// Starts observing the floating window's display-topology messages.
/// Returns a generation that identifies this observation.
#[tauri::command]
pub async fn attach_floating_monitor_observer(window: tauri::WebviewWindow) -> Result<u32, String> {
    if window.label() != "floating-quota" {
        return Err("Monitor topology observation belongs to floating-quota".into());
    }
    #[cfg(windows)]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let target = window.clone();
        window
            .run_on_main_thread(move || {
                let _ = tx.send(native::attach(target));
            })
            .map_err(|error| error.to_string())?;
        rx.await.map_err(|error| error.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = &window;
        Ok(0)
    }
}

/// Stops the observation that `generation` identifies. A stale generation
/// is ignored, so a late detach cannot silence a newer observation.
#[tauri::command]
pub async fn detach_floating_monitor_observer(
    window: tauri::WebviewWindow,
    generation: u32,
) -> Result<(), String> {
    if window.label() != "floating-quota" {
        return Err("Monitor topology observation belongs to floating-quota".into());
    }
    #[cfg(windows)]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        window
            .run_on_main_thread(move || {
                native::detach(generation);
                let _ = tx.send(());
            })
            .map_err(|error| error.to_string())?;
        rx.await.map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = &window;
        let _ = generation;
        Ok(())
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        cell::RefCell,
        sync::atomic::{AtomicU32, Ordering},
    };
    use tauri::Emitter;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    };

    /// Distinct from the drag lifecycle's subclass id, so the two
    /// observations on the same window never remove each other.
    const SUBCLASS: usize = 0x4c534d54; // LimitScope monitor topology.
    static GENERATION: AtomicU32 = AtomicU32::new(1);
    thread_local! { static INSTALLED: RefCell<Option<Installation>> = const { RefCell::new(None) }; }

    struct Installation {
        window: Option<tauri::WebviewWindow>,
        hwnd: HWND,
        generation: u32,
    }

    pub fn attach(window: tauri::WebviewWindow) -> Result<u32, String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as HWND;
        install(hwnd, Some(window))
    }

    fn install(hwnd: HWND, window: Option<tauri::WebviewWindow>) -> Result<u32, String> {
        let thread = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
        if thread == 0 || thread != unsafe { GetCurrentThreadId() } {
            return Err("Monitor topology observation must install on its window thread".into());
        }
        detach_current();
        let generation = GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| "Monitor topology generation exhausted".to_string())?;
        if unsafe { SetWindowSubclass(hwnd, Some(window_message), SUBCLASS, 0) } == 0 {
            return Err(format!(
                "Monitor topology subclass: {}",
                std::io::Error::last_os_error()
            ));
        }
        INSTALLED.with(|slot| {
            *slot.borrow_mut() = Some(Installation {
                window,
                hwnd,
                generation,
            })
        });
        Ok(generation)
    }

    pub fn detach(generation: u32) {
        let current = INSTALLED.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|state| state.generation == generation)
        });
        if current {
            detach_current();
        }
    }

    fn detach_current() {
        let old = INSTALLED.with(|slot| slot.borrow_mut().take());
        if let Some(old) = old {
            unsafe {
                RemoveWindowSubclass(old.hwnd, Some(window_message), SUBCLASS);
            }
        }
    }

    unsafe extern "system" fn window_message(
        hwnd: HWND,
        message: u32,
        wp: WPARAM,
        lp: LPARAM,
        _: usize,
        _: usize,
    ) -> LRESULT {
        if let Some(change) = classify(message, wp) {
            INSTALLED.with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut() {
                    if let Some(state) = slot.as_mut().filter(|state| state.hwnd == hwnd) {
                        if let Some(window) = &state.window {
                            let _ = window.emit(EVENT, TopologyEvent { change });
                        }
                    }
                }
            });
        }
        if message == WM_NCDESTROY {
            detach_current();
        }
        DefSubclassProc(hwnd, message, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[cfg(windows)]
    #[test]
    fn classifies_the_messages_windows_broadcasts_on_a_topology_change() {
        assert_eq!(classify(WM_DISPLAYCHANGE, 0), Some(Change::Display));
        assert_eq!(
            classify(WM_DEVICECHANGE, DBT_DEVNODES_CHANGED as usize),
            Some(Change::Device)
        );
        assert_eq!(
            classify(WM_SETTINGCHANGE, SPI_SETWORKAREA as usize),
            Some(Change::WorkArea)
        );
        assert_eq!(
            classify(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize),
            Some(Change::Resume)
        );
        assert_eq!(
            classify(WM_POWERBROADCAST, PBT_APMRESUMESUSPEND as usize),
            Some(Change::Resume)
        );
    }

    #[cfg(windows)]
    #[test]
    fn ignores_messages_that_are_not_topology_signals() {
        // A device change that is not about display nodes (a USB key) and a
        // settings change that is not about the work area are both noise.
        assert_eq!(classify(WM_DEVICECHANGE, 0x8000), None);
        assert_eq!(classify(WM_SETTINGCHANGE, 0x0002), None);
        assert_eq!(classify(WM_POWERBROADCAST, PBT_APMSUSPEND as usize), None);
        assert_eq!(classify(WM_MOVE, 0), None);
    }

    #[test]
    fn the_wire_contract_names_the_four_changes() {
        assert_eq!(
            serde_json::to_value(TopologyEvent { change: Change::Display }).unwrap(),
            serde_json::json!({ "change": "display" })
        );
        assert_eq!(
            serde_json::to_value(TopologyEvent { change: Change::Device }).unwrap(),
            serde_json::json!({ "change": "device" })
        );
        assert_eq!(
            serde_json::to_value(TopologyEvent { change: Change::WorkArea }).unwrap(),
            serde_json::json!({ "change": "workarea" })
        );
        assert_eq!(
            serde_json::to_value(TopologyEvent { change: Change::Resume }).unwrap(),
            serde_json::json!({ "change": "resume" })
        );
    }

    #[test]
    fn the_event_name_is_the_one_the_webview_listens_for() {
        assert_eq!(EVENT, "floating://monitor-topology");
    }
}

