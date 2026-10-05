//! Native evidence for the existing floating window's system move loop.
//! No docking policy lives here. The input observer is bound to the window's
//! own GUI thread, and is removed with its window subclass/subscription.

use serde::Serialize;

pub const EVENT: &str = "floating://drag-lifecycle";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    Release,
    Cancel,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Begin,
    Move,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DragEvent {
    pub generation: u32,
    pub gesture: u32,
    pub sequence: u32,
    pub phase: Phase,
    pub reason: Option<Reason>,
    pub position: Position,
}

struct Active {
    sequence: u32,
    position: Position,
    reason: Option<Reason>,
}

struct Lifecycle {
    generation: u32,
    gesture: u32,
    active: Option<Active>,
}

impl Lifecycle {
    fn allows_restore(&self, generation: u32, gesture: u32) -> bool {
        generation == self.generation
            && gesture > 0
            && gesture == self.gesture
            && self.active.is_none()
    }
    fn new(generation: u32) -> Self {
        Self {
            generation,
            gesture: 0,
            active: None,
        }
    }

    fn begin(&mut self, position: Position, pointer_held: bool) -> Option<DragEvent> {
        // System move loops also support keyboard movement. This contract is
        // specifically for a held left-button drag, not every window move.
        if !pointer_held || self.active.is_some() {
            return None;
        }
        self.gesture = self.gesture.checked_add(1)?;
        self.active = Some(Active {
            sequence: 1,
            position,
            reason: None,
        });
        Some(self.event(Phase::Begin, None, position, 1))
    }

    fn event(
        &self,
        phase: Phase,
        reason: Option<Reason>,
        position: Position,
        sequence: u32,
    ) -> DragEvent {
        DragEvent {
            generation: self.generation,
            gesture: self.gesture,
            sequence,
            phase,
            reason,
            position,
        }
    }

    fn moved(&mut self, position: Position) -> Option<DragEvent> {
        let active = self.active.as_mut()?;
        if active.position == position {
            return None;
        }
        active.position = position;
        active.sequence = active.sequence.checked_add(1)?;
        let sequence = active.sequence;
        Some(self.event(Phase::Move, None, position, sequence))
    }

    fn released(&mut self) {
        if let Some(active) = &mut self.active {
            if active.reason != Some(Reason::Cancel) {
                active.reason = Some(Reason::Release);
            }
        }
    }

    fn cancelled(&mut self) {
        if let Some(active) = &mut self.active {
            active.reason = Some(Reason::Cancel);
        }
    }

    fn end(&mut self, position: Position) -> Option<DragEvent> {
        let active = self.active.take()?;
        Some(self.event(
            Phase::End,
            Some(active.reason.unwrap_or(Reason::Unknown)),
            position,
            active.sequence.saturating_add(1),
        ))
    }
}

#[tauri::command]
pub async fn attach_floating_drag_lifecycle(window: tauri::WebviewWindow) -> Result<u32, String> {
    if window.label() != "floating-quota" {
        return Err("Floating drag lifecycle belongs to floating-quota".into());
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
        Ok(0)
    }
}

#[tauri::command]
pub async fn detach_floating_drag_lifecycle(
    window: tauri::WebviewWindow,
    generation: u32,
) -> Result<(), String> {
    if window.label() != "floating-quota" {
        return Err("Floating drag lifecycle belongs to floating-quota".into());
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
        let _ = generation;
        Ok(())
    }
}

/// A cancellation restore shares the frontend placement owner, but must also
/// reject an IPC request that reaches Windows after a newer native begin.
#[tauri::command]
pub async fn restore_floating_drag_origin(
    window: tauri::WebviewWindow,
    generation: u32,
    gesture: u32,
    x: i32,
    y: i32,
) -> Result<bool, String> {
    if window.label() != "floating-quota" {
        return Err("Floating drag lifecycle belongs to floating-quota".into());
    }
    #[cfg(windows)]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let target = window.clone();
        window
            .run_on_main_thread(move || {
                let result = target
                    .hwnd()
                    .map_err(|error| error.to_string())
                    .and_then(|hwnd| {
                        native::restore_position(
                            hwnd.0 as _,
                            generation,
                            gesture,
                            Position { x, y },
                        )
                    });
                let _ = tx.send(result);
            })
            .map_err(|error| error.to_string())?;
        rx.await.map_err(|error| error.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (generation, gesture, x, y);
        Ok(false)
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        cell::RefCell,
        fs::{File, OpenOptions},
        io::Write,
        ptr::null_mut,
        sync::atomic::{AtomicU32, Ordering},
    };
    use tauri::Emitter;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    };

    const SUBCLASS: usize = 0x4c534447; // LimitScope drag, distinct from Tao/Wry.
    static GENERATION: AtomicU32 = AtomicU32::new(1);
    thread_local! { static INSTALLED: RefCell<Option<Installation>> = const { RefCell::new(None) }; }

    struct Installation {
        window: Option<tauri::WebviewWindow>,
        hwnd: HWND,
        hook: HHOOK,
        lifecycle: Lifecycle,
        trace: Option<File>,
        #[cfg(test)]
        evidence: Option<std::sync::Arc<std::sync::Mutex<Vec<DragEvent>>>>,
    }

    impl Installation {
        fn publish(&mut self, event: DragEvent) {
            // Trace is opt-in, local and contains only lifecycle geometry,
            // never raw keyboard input or provider/user data.
            if let Some(file) = &mut self.trace {
                if let Ok(mut line) = serde_json::to_vec(&event) {
                    line.push(b'\n');
                    let _ = file.write_all(&line);
                }
            }
            #[cfg(test)]
            if let Some(evidence) = &self.evidence {
                if let Ok(mut evidence) = evidence.lock() {
                    evidence.push(event.clone());
                }
            }
            if let Some(window) = &self.window {
                let _ = window.emit(EVENT, event);
            }
        }
        fn cancel_and_end(&mut self) {
            self.lifecycle.cancelled();
            let position = position(self.hwnd)
                .or_else(|| self.lifecycle.active.as_ref().map(|active| active.position));
            if let Some(position) = position {
                if let Some(event) = self.lifecycle.end(position) {
                    self.publish(event);
                }
            }
        }
    }

    fn position(hwnd: HWND) -> Option<Position> {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return None;
        }
        Some(Position {
            x: rect.left,
            y: rect.top,
        })
    }

    pub fn attach(window: tauri::WebviewWindow) -> Result<u32, String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as HWND;
        install(hwnd, Some(window))
    }

    fn install(hwnd: HWND, window: Option<tauri::WebviewWindow>) -> Result<u32, String> {
        let thread = unsafe { GetWindowThreadProcessId(hwnd, null_mut()) };
        if thread == 0 || thread != unsafe { GetCurrentThreadId() } {
            return Err("Floating drag lifecycle must install on its window thread".into());
        }
        detach_current();
        let generation = GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| "Floating drag generation exhausted".to_string())?;
        let hook =
            unsafe { SetWindowsHookExW(WH_GETMESSAGE, Some(input_message), null_mut(), thread) };
        if hook.is_null() {
            return Err(format!(
                "Floating input observer: {}",
                std::io::Error::last_os_error()
            ));
        }
        if unsafe { SetWindowSubclass(hwnd, Some(window_message), SUBCLASS, 0) } == 0 {
            unsafe {
                UnhookWindowsHookEx(hook);
            }
            return Err(format!(
                "Floating drag subclass: {}",
                std::io::Error::last_os_error()
            ));
        }
        let trace = std::env::var_os("LIMITSCOPE_DRAG_TRACE")
            .and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
        INSTALLED.with(|slot| {
            *slot.borrow_mut() = Some(Installation {
                window,
                hwnd,
                hook,
                lifecycle: Lifecycle::new(generation),
                trace,
                #[cfg(test)]
                evidence: None,
            })
        });
        Ok(generation)
    }

    pub fn detach(generation: u32) {
        let current = INSTALLED.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|state| state.lifecycle.generation == generation)
        });
        if current {
            detach_current();
        }
    }

    pub fn restore_position(
        hwnd: HWND,
        generation: u32,
        gesture: u32,
        point: Position,
    ) -> Result<bool, String> {
        // Executed on the same GUI thread as begin/end. No await or message
        // pump separates evidence validation from the native position write.
        let eligible = INSTALLED.with(|slot| {
            slot.borrow().as_ref().is_some_and(|state| {
                state.hwnd == hwnd && state.lifecycle.allows_restore(generation, gesture)
            })
        });
        if !eligible {
            return Ok(false);
        }
        if unsafe {
            SetWindowPos(
                hwnd,
                null_mut(),
                point.x,
                point.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        } == 0
        {
            return Err(format!(
                "Floating cancelled-drag restore: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(true)
    }

    fn detach_current() {
        let old = INSTALLED.with(|slot| slot.borrow_mut().take());
        if let Some(mut old) = old {
            let was_active = old.lifecycle.active.is_some();
            old.cancel_and_end();
            unsafe {
                RemoveWindowSubclass(old.hwnd, Some(window_message), SUBCLASS);
                UnhookWindowsHookEx(old.hook);
                if was_active {
                    PostMessageW(old.hwnd, WM_CANCELMODE, 0, 0);
                }
            }
        }
    }

    unsafe extern "system" fn input_message(code: i32, removed: WPARAM, data: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && removed == PM_REMOVE as usize && data != 0 {
            let message = &*(data as *const MSG);
            INSTALLED.with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut() {
                    if let Some(state) = slot.as_mut() {
                        if state.lifecycle.active.is_none() {
                            return;
                        }
                        // Escape must be observed before DefWindowProc's modal
                        // loop consumes it; it need not reach our WindowProc.
                        match message.message {
                            WM_KEYDOWN | WM_SYSKEYDOWN if message.wParam == VK_ESCAPE as usize => {
                                state.lifecycle.cancelled()
                            }
                            WM_LBUTTONUP | WM_NCLBUTTONUP => {
                                if message.hwnd.is_null()
                                    || GetAncestor(message.hwnd, GA_ROOT) == state.hwnd
                                {
                                    state.lifecycle.released();
                                }
                            }
                            WM_QUIT => state.lifecycle.cancelled(),
                            _ => {}
                        }
                    }
                }
            });
        }
        CallNextHookEx(null_mut(), code, removed, data)
    }

    unsafe extern "system" fn window_message(
        hwnd: HWND,
        message: u32,
        wp: WPARAM,
        lp: LPARAM,
        _: usize,
        _: usize,
    ) -> LRESULT {
        // Tao posts this request asynchronously. A fast released click must
        // not start an orphaned system move loop after the button is up.
        if message == WM_NCLBUTTONDOWN
            && wp == HTCAPTION as usize
            && GetAsyncKeyState(VK_LBUTTON as i32) >= 0
        {
            return 0;
        }
        if message == WM_NCDESTROY {
            detach_current();
            return DefSubclassProc(hwnd, message, wp, lp);
        }
        // Observe entry/cancellation before the chain, geometry/exit after.
        INSTALLED.with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut() {
                if let Some(state) = slot.as_mut().filter(|state| state.hwnd == hwnd) {
                    match message {
                        WM_ENTERSIZEMOVE => {
                            if let Some(position) = position(hwnd) {
                                if let Some(event) = state
                                    .lifecycle
                                    .begin(position, GetAsyncKeyState(VK_LBUTTON as i32) < 0)
                                {
                                    state.publish(event);
                                }
                            }
                        }
                        WM_CANCELMODE => state.lifecycle.cancelled(),
                        // DefWindowProc may end its move loop before sending
                        // WM_ACTIVATE/WM_ACTIVATEAPP. Sample the actual native
                        // foreground owner at exit, before publishing a result.
                        WM_EXITSIZEMOVE if GetForegroundWindow() != hwnd => {
                            state.lifecycle.cancelled()
                        }
                        WM_ACTIVATEAPP if wp == 0 => state.lifecycle.cancelled(),
                        WM_ACTIVATE if wp & 0xffff == WA_INACTIVE as usize => {
                            state.lifecycle.cancelled()
                        }
                        WM_SHOWWINDOW if wp == 0 => state.lifecycle.cancelled(),
                        WM_CAPTURECHANGED => {
                            // Normal button release also drops capture. Only
                            // unexplained capture loss is external cancellation.
                            if state
                                .lifecycle
                                .active
                                .as_ref()
                                .is_some_and(|active| active.reason != Some(Reason::Release))
                            {
                                state.lifecycle.cancelled();
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
        let result = DefSubclassProc(hwnd, message, wp, lp);
        if message == WM_MOVE || message == WM_EXITSIZEMOVE {
            INSTALLED.with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut() {
                    if let Some(state) = slot.as_mut().filter(|state| state.hwnd == hwnd) {
                        if let Some(position) = position(hwnd) {
                            let event = if message == WM_EXITSIZEMOVE {
                                state.lifecycle.end(position)
                            } else {
                                state.lifecycle.moved(position)
                            };
                            if let Some(event) = event {
                                state.publish(event);
                            }
                        } else if message == WM_EXITSIZEMOVE {
                            state.cancel_and_end();
                        }
                    }
                }
            });
        }
        result
    }

    #[cfg(test)]
    mod desktop_proof {
        use super::*;
        use std::{
            sync::{Arc, Mutex},
            thread,
            time::{Duration, Instant},
        };
        use windows_sys::Win32::{
            System::LibraryLoader::GetModuleHandleW, UI::Input::KeyboardAndMouse::*,
        };

        const DETACH: u32 = WM_APP + 81;
        const REATTACH: u32 = WM_APP + 82;
        const RESTORE: u32 = WM_APP + 83;
        thread_local! { static EVIDENCE: RefCell<Option<Arc<Mutex<Vec<DragEvent>>>>> = const { RefCell::new(None) }; }

        unsafe extern "system" fn proof_window(
            hwnd: HWND,
            message: u32,
            wp: WPARAM,
            lp: LPARAM,
        ) -> LRESULT {
            match message {
                WM_LBUTTONDOWN => {
                    PostMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
                    0
                }
                DETACH => {
                    detach(wp as u32);
                    0
                }
                RESTORE => {
                    restore_position(hwnd, lp as u32, wp as u32, Position { x: 50, y: 150 })
                        .expect("scoped restore");
                    0
                }
                REATTACH => {
                    install(hwnd, None).expect("reattach native lifecycle");
                    EVIDENCE.with(|evidence| {
                        INSTALLED.with(|slot| {
                            slot.borrow_mut().as_mut().unwrap().evidence =
                                evidence.borrow().clone();
                        })
                    });
                    0
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(hwnd, message, wp, lp),
            }
        }

        fn input_mouse(flags: u32) {
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            assert_eq!(
                unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) },
                1
            );
        }
        fn escape() {
            for flags in [0, KEYEVENTF_KEYUP] {
                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VK_ESCAPE,
                            wScan: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                assert_eq!(
                    unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) },
                    1
                );
                thread::sleep(Duration::from_millis(80));
            }
        }
        fn wait_for(label: &str, mut predicate: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if predicate() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            panic!("Native desktop proof timed out: {label}");
        }
        fn events(evidence: &Arc<Mutex<Vec<DragEvent>>>, from: usize) -> Vec<DragEvent> {
            evidence.lock().unwrap()[from..].to_vec()
        }
        fn begin(hwnd: HWND, evidence: &Arc<Mutex<Vec<DragEvent>>>) -> usize {
            let from = evidence.lock().unwrap().len();
            unsafe {
                SetForegroundWindow(hwnd);
                let at = position(hwnd).unwrap();
                SetCursorPos(at.x + 20, at.y + 20);
            }
            thread::sleep(Duration::from_millis(30));
            input_mouse(MOUSEEVENTF_LEFTDOWN);
            wait_for("confirmed begin", || {
                events(evidence, from)
                    .iter()
                    .any(|event| event.phase == Phase::Begin)
            });
            from
        }
        fn finish(
            evidence: &Arc<Mutex<Vec<DragEvent>>>,
            from: usize,
            reason: Reason,
        ) -> Vec<DragEvent> {
            wait_for("terminal event", || {
                events(evidence, from)
                    .iter()
                    .any(|event| event.phase == Phase::End)
            });
            thread::sleep(Duration::from_millis(60));
            let observed = events(evidence, from);
            assert_eq!(
                observed
                    .iter()
                    .filter(|event| event.phase == Phase::Begin)
                    .count(),
                1
            );
            assert_eq!(
                observed
                    .iter()
                    .filter(|event| event.phase == Phase::End)
                    .count(),
                1
            );
            assert_eq!(observed.last().unwrap().reason, Some(reason));
            for (index, event) in observed.iter().enumerate() {
                assert_eq!(event.sequence as usize, index + 1);
            }
            observed
        }

        /// This exercises the actual subclass, thread hook, DefWindowProc move
        /// loop and SendInput, not fabricated WM_ENTER/EXIT notifications.
        /// It never touches LimitScope preferences or provider state.
        #[test]
        #[ignore = "requires an unlocked interactive Windows desktop"]
        fn real_windows_modal_drag_lifecycle() {
            let evidence = Arc::new(Mutex::new(Vec::new()));
            let original_foreground = unsafe { GetForegroundWindow() } as usize;
            let class: Vec<u16> = "LimitScopeDragLifecycleProof\0".encode_utf16().collect();
            let module = unsafe { GetModuleHandleW(std::ptr::null()) };
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proof_window),
                hInstance: module,
                lpszClassName: class.as_ptr(),
                hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
                ..unsafe { std::mem::zeroed() }
            };
            assert_ne!(unsafe { RegisterClassW(&wc) }, 0);
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST,
                    class.as_ptr(),
                    class.as_ptr(),
                    WS_POPUP | WS_VISIBLE,
                    700,
                    320,
                    600,
                    64,
                    null_mut(),
                    null_mut(),
                    module,
                    null_mut(),
                )
            };
            assert!(!hwnd.is_null());
            let generation = install(hwnd, None).expect("install actual scoped observer");
            EVIDENCE.with(|slot| *slot.borrow_mut() = Some(evidence.clone()));
            INSTALLED
                .with(|slot| slot.borrow_mut().as_mut().unwrap().evidence = Some(evidence.clone()));
            let handle = hwnd as usize;
            let driver = thread::spawn(move || {
                let hwnd = handle as HWND;
                let result = std::panic::catch_unwind(|| {
                    // Programmatic relocation never creates a gesture.
                    unsafe {
                        SetWindowPos(hwnd, null_mut(), 700, 320, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
                    }
                    thread::sleep(Duration::from_millis(80));
                    assert!(evidence.lock().unwrap().is_empty());

                    // Both inputs are queued as one batch. Tao's deferred
                    // caption request must not open a modal loop once up.
                    unsafe {
                        SetCursorPos(720, 340);
                    }
                    let click = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dx: 0,
                                dy: 0,
                                mouseData: 0,
                                dwFlags: flags,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                    assert_eq!(
                        unsafe {
                            SendInput(2, click.as_ptr(), std::mem::size_of::<INPUT>() as i32)
                        },
                        2
                    );
                    thread::sleep(Duration::from_millis(150));
                    assert!(evidence.lock().unwrap().is_empty());
                    println!(
                        "REAL_WINDOWS PASS release before deferred caption request/no orphan loop"
                    );

                    let from = begin(hwnd, &evidence);
                    input_mouse(MOUSEEVENTF_LEFTUP);
                    finish(&evidence, from, Reason::Release);
                    println!("REAL_WINDOWS PASS fast confirmed release");

                    let from = begin(hwnd, &evidence);
                    thread::sleep(Duration::from_millis(650));
                    assert!(!events(&evidence, from)
                        .iter()
                        .any(|event| event.phase == Phase::End));
                    input_mouse(MOUSEEVENTF_LEFTUP);
                    finish(&evidence, from, Reason::Release);
                    println!("REAL_WINDOWS PASS held stationary >200ms then release");

                    for (dx, dy, label) in [(140, 0, "horizontal"), (0, 90, "vertical")] {
                        let from = begin(hwnd, &evidence);
                        let at = position(hwnd).unwrap();
                        unsafe {
                            SetCursorPos(at.x + 20 + dx, at.y + 20 + dy);
                        }
                        wait_for("native movement", || {
                            events(&evidence, from)
                                .iter()
                                .any(|event| event.phase == Phase::Move)
                        });
                        input_mouse(MOUSEEVENTF_LEFTUP);
                        let observed = finish(&evidence, from, Reason::Release);
                        assert_ne!(observed.last().unwrap().position, observed[0].position);
                        println!("REAL_WINDOWS PASS {label} native movement/release");
                    }

                    let from = begin(hwnd, &evidence);
                    let origin = position(hwnd).unwrap();
                    unsafe {
                        SetCursorPos(origin.x + 160, origin.y + 90);
                    }
                    wait_for("move before Escape", || {
                        events(&evidence, from)
                            .iter()
                            .any(|event| event.phase == Phase::Move)
                    });
                    escape();
                    let observed = finish(&evidence, from, Reason::Cancel);
                    assert_eq!(observed.last().unwrap().position, origin);
                    input_mouse(MOUSEEVENTF_LEFTUP);
                    println!("REAL_WINDOWS PASS Escape consumed by modal loop, origin restored");

                    let from = begin(hwnd, &evidence);
                    unsafe {
                        PostMessageW(hwnd, WM_CANCELMODE, 0, 0);
                    }
                    finish(&evidence, from, Reason::Cancel);
                    input_mouse(MOUSEEVENTF_LEFTUP);
                    println!("REAL_WINDOWS PASS external WM_CANCELMODE cancellation");

                    assert!(
                        original_foreground != 0 && original_foreground != handle,
                        "Need an existing foreground application for real deactivation proof"
                    );
                    let from = begin(hwnd, &evidence);
                    unsafe {
                        SetForegroundWindow(original_foreground as HWND);
                    }
                    wait_for(
                        "real deactivation",
                        || unsafe { GetForegroundWindow() } != hwnd,
                    );
                    input_mouse(MOUSEEVENTF_LEFTUP);
                    finish(&evidence, from, Reason::Cancel);
                    println!("REAL_WINDOWS PASS actual foreground deactivation cancellation");

                    // Cleanup during a held drag emits one cancel; late input
                    // cannot affect a detached observer, then reattach safely.
                    let from = begin(hwnd, &evidence);
                    unsafe {
                        PostMessageW(hwnd, DETACH, generation as usize, 0);
                    }
                    finish(&evidence, from, Reason::Cancel);
                    input_mouse(MOUSEEVENTF_LEFTUP);
                    let count = evidence.lock().unwrap().len();
                    unsafe {
                        SetWindowPos(hwnd, null_mut(), 700, 320, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
                    }
                    thread::sleep(Duration::from_millis(80));
                    assert_eq!(evidence.lock().unwrap().len(), count);
                    unsafe {
                        PostMessageW(hwnd, REATTACH, 0, 0);
                    }
                    thread::sleep(Duration::from_millis(80));
                    let from = begin(hwnd, &evidence);
                    unsafe {
                        PostMessageW(hwnd, DETACH, generation as usize, 0);
                    } // stale generation
                    thread::sleep(Duration::from_millis(80));
                    assert!(!events(&evidence, from)
                        .iter()
                        .any(|event| event.phase == Phase::End));
                    input_mouse(MOUSEEVENTF_LEFTUP);
                    let observed = finish(&evidence, from, Reason::Release);
                    assert_ne!(observed[0].generation, generation);
                    println!("REAL_WINDOWS PASS cleanup, reattach, stale detach rejection");

                    let previous = observed[0].clone();
                    let from = begin(hwnd, &evidence);
                    let held_position = position(hwnd).unwrap();
                    unsafe {
                        PostMessageW(
                            hwnd,
                            RESTORE,
                            previous.gesture as usize,
                            previous.generation as isize,
                        );
                    }
                    thread::sleep(Duration::from_millis(80));
                    assert_eq!(position(hwnd), Some(held_position));
                    assert!(!events(&evidence, from)
                        .iter()
                        .any(|event| event.phase != Phase::Begin));
                    input_mouse(MOUSEEVENTF_LEFTUP);
                    finish(&evidence, from, Reason::Release);
                    // It remains stale after the newer drag has ended too.
                    unsafe {
                        PostMessageW(
                            hwnd,
                            RESTORE,
                            previous.gesture as usize,
                            previous.generation as isize,
                        );
                    }
                    thread::sleep(Duration::from_millis(80));
                    assert_eq!(position(hwnd), Some(held_position));
                    println!(
                        "REAL_WINDOWS PASS queued old restore rejected during/after newer gesture"
                    );

                    for _ in 0..3 {
                        let from = begin(hwnd, &evidence);
                        input_mouse(MOUSEEVENTF_LEFTUP);
                        finish(&evidence, from, Reason::Release);
                    }
                    println!("REAL_WINDOWS PASS repeated cycles/exactly-one terminal");
                });
                // Always release injected input and close the owned proof
                // window, even when an assertion fails.
                input_mouse(MOUSEEVENTF_LEFTUP);
                unsafe {
                    SetForegroundWindow(hwnd);
                }
                escape();
                unsafe {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                result
            });
            let mut message: MSG = unsafe { std::mem::zeroed() };
            while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            detach_current();
            EVIDENCE.with(|slot| slot.borrow_mut().take());
            unsafe {
                UnregisterClassW(class.as_ptr(), module);
                if original_foreground != 0 {
                    SetForegroundWindow(original_foreground as HWND);
                }
            }
            if let Err(panic) = driver.join().expect("native input driver") {
                std::panic::resume_unwind(panic);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: Position = Position { x: 80, y: 40 };
    const B: Position = Position { x: -120, y: 240 };

    #[test]
    fn restore_is_bound_to_the_current_ended_native_gesture() {
        let mut state = Lifecycle::new(7);
        assert!(!state.allows_restore(7, 0));
        state.begin(A, true);
        assert!(!state.allows_restore(7, 1));
        state.cancelled();
        state.end(A);
        assert!(state.allows_restore(7, 1));
        assert!(!state.allows_restore(6, 1));
        state.begin(B, true);
        assert!(!state.allows_restore(7, 1));
        state.end(B);
        assert!(!state.allows_restore(7, 1));
    }

    #[test]
    fn programmatic_move_and_unconfirmed_press_do_not_begin() {
        let mut state = Lifecycle::new(1);
        assert!(state.moved(B).is_none());
        assert!(state.begin(A, false).is_none());
        assert!(state.end(B).is_none());
    }
    #[test]
    fn stationary_held_drag_has_no_timer_terminal() {
        let mut state = Lifecycle::new(1);
        assert_eq!(state.begin(A, true).unwrap().sequence, 1);
        for _ in 0..100 {
            assert!(state.moved(A).is_none());
        }
        assert!(state.active.is_some());
        state.released();
        let end = state.end(A).unwrap();
        assert_eq!((end.sequence, end.reason), (2, Some(Reason::Release)));
    }
    #[test]
    fn fast_release_has_exactly_one_terminal_without_a_move() {
        let mut state = Lifecycle::new(2);
        state.begin(A, true);
        state.released();
        assert_eq!(state.end(A).unwrap().reason, Some(Reason::Release));
        assert!(state.end(A).is_none());
        assert!(state.moved(B).is_none());
    }
    #[test]
    fn horizontal_and_vertical_moves_are_ordered() {
        let mut state = Lifecycle::new(3);
        let begin = state.begin(A, true).unwrap();
        let horizontal = state.moved(Position { x: -20, ..A }).unwrap();
        let vertical = state.moved(B).unwrap();
        state.released();
        let end = state.end(B).unwrap();
        assert_eq!(
            [
                begin.sequence,
                horizontal.sequence,
                vertical.sequence,
                end.sequence
            ],
            [1, 2, 3, 4]
        );
        assert_eq!(end.position, B);
    }
    #[test]
    fn cancellation_overrides_release_and_subsequent_button_up() {
        let mut state = Lifecycle::new(1);
        state.begin(A, true);
        state.released();
        state.cancelled();
        state.released();
        assert_eq!(state.end(A).unwrap().reason, Some(Reason::Cancel));
    }
    #[test]
    fn unknown_exit_is_explicit_and_never_release() {
        let mut state = Lifecycle::new(1);
        state.begin(A, true);
        assert_eq!(state.end(B).unwrap().reason, Some(Reason::Unknown));
    }
    #[test]
    fn duplicate_begin_does_not_replace_origin_or_session() {
        let mut state = Lifecycle::new(1);
        state.begin(A, true);
        assert!(state.begin(B, true).is_none());
        assert_eq!(state.gesture, 1);
        assert_eq!(state.active.as_ref().unwrap().position, A);
    }
    #[test]
    fn evidence_after_exit_cannot_affect_next_gesture() {
        let mut state = Lifecycle::new(1);
        state.begin(A, true);
        state.cancelled();
        state.end(A);
        state.released(); // Tao posts an artificial button-up after exit.
        let begin = state.begin(B, true).unwrap();
        assert_eq!(begin.gesture, 2);
        assert_eq!(state.end(B).unwrap().reason, Some(Reason::Unknown));
    }
    #[test]
    fn cleanup_ends_once_as_cancel() {
        let mut state = Lifecycle::new(1);
        state.begin(A, true);
        state.cancelled();
        assert_eq!(state.end(A).unwrap().reason, Some(Reason::Cancel));
        assert!(state.end(A).is_none());
    }
    #[test]
    fn wire_contract_is_bounded_geometry_only() {
        let mut state = Lifecycle::new(7);
        let event = state.begin(A, true).unwrap();
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "generation":7,"gesture":1,"sequence":1,"phase":"begin","reason":null,"position":{"x":80,"y":40}
            })
        );
    }
    #[test]
    fn duplicate_terminal_with_new_geometry_is_inert_and_restore_stays_eligible() {
        let mut state = Lifecycle::new(1);
        state.begin(A, true);
        state.released();
        assert_eq!(state.end(A).unwrap().reason, Some(Reason::Release));
        // A late duplicate terminal carrying different geometry is rejected.
        assert!(state.end(B).is_none());
        assert!(state.moved(B).is_none());
        // Late cancel/release evidence after the terminal stays inert, and the
        // ended gesture remains the one a cancellation restore may commit.
        state.cancelled();
        state.released();
        assert!(state.end(B).is_none());
        assert!(state.allows_restore(1, 1));
    }
    #[test]
    fn stale_geometry_after_a_terminal_cannot_poison_the_next_gesture() {
        let mut state = Lifecycle::new(4);
        state.begin(A, true);
        state.cancelled();
        state.end(A);
        // The echo stream keeps posting after the terminal; none of it may
        // become the next gesture's origin.
        for stale in [
            B,
            Position {
                x: i32::MAX,
                y: i32::MIN,
            },
        ] {
            assert!(state.moved(stale).is_none());
        }
        state.released();
        let begin = state.begin(B, true).unwrap();
        assert_eq!(begin.gesture, 2);
        assert_eq!(state.active.as_ref().unwrap().position, B);
        state.released();
        let end = state.end(B).unwrap();
        assert_eq!((end.gesture, end.reason), (2, Some(Reason::Release)));
    }
    #[test]
    fn rapid_cancel_then_immediate_new_gesture_restarts_sequences() {
        let mut state = Lifecycle::new(2);
        state.begin(A, true);
        state.cancelled();
        let first = state.end(A).unwrap();
        assert_eq!(
            (first.gesture, first.sequence, first.reason),
            (1, 2, Some(Reason::Cancel))
        );
        let second = state.begin(B, true).unwrap();
        assert_eq!((second.gesture, second.sequence), (2, 1));
        let moved = state
            .moved(Position {
                x: B.x + 9,
                y: B.y - 9,
            })
            .unwrap();
        assert_eq!((moved.gesture, moved.sequence), (2, 2));
        state.released();
        let last = state.end(B).unwrap();
        assert_eq!(last.sequence, 3);
        assert_eq!(last.reason, Some(Reason::Release));
    }
    #[test]
    fn restore_supersession_survives_the_newer_gesture_ending() {
        let mut state = Lifecycle::new(9);
        state.begin(A, true);
        state.cancelled();
        state.end(A);
        assert!(state.allows_restore(9, 1));
        // Gesture ids that never ended here, gesture 0, and foreign
        // generations cannot commit a restore.
        assert!(!state.allows_restore(9, 2));
        assert!(!state.allows_restore(9, 0));
        assert!(!state.allows_restore(8, 1));
        state.begin(B, true);
        state.end(B);
        // The newer gesture's terminal permanently supersedes the older
        // restore target, even after it too has ended.
        assert!(!state.allows_restore(9, 1));
        assert!(state.allows_restore(9, 2));
    }
}
