// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod antigravity;
mod codex;
#[cfg(test)]
mod diagnostic_report;
mod diagnostics;
mod floating_drag;
mod grok;
mod history;
mod last_good;
mod local_data;
mod notifications;
mod opencode_go;
mod provider_error;
mod reset_plausibility;
mod runtime;
mod secret_scrub;
mod usage_analytics;
mod usage_export;
mod zai;
mod zcode_plans;
mod zcode_reset;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Listener, Manager,
};
use tauri_plugin_single_instance::init as single_instance_init;

const TRAY_SHOW_FLOATING_EVENT: &str = "tray://show-floating";
const TRAY_HIDE_FLOATING_EVENT: &str = "tray://hide-floating";
// The floating webview announces visibility changes on this event so the
// tray label can mirror the persisted pref (A10 symmetry).
const FLOATING_VISIBLE_EVENT: &str = "floating://visible-changed";

// The autostart entry is registered with this argument (written into the
// HKCU Run key together with the exe path), so a launch triggered by Windows
// can be told apart from a manual launch by the process command line.
const AUTOSTART_ARG: &str = "--hidden";

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn show_floating_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("floating-quota") {
        // Show the utility bar without pulling the foreground app away.
        // Clearing focusable around show is the reliable Windows path for
        // that; it is restored immediately so a later click can take focus.
        let _ = window.set_focusable(false);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focusable(true);
    }
}

/// A10: one tray item mirrors the floating bar's visibility state; the label
/// is the user-visible contract (and is pinned by tests together with the
/// event names the webview listeners share).
fn floating_menu_label(visible: bool) -> &'static str {
    if visible {
        "Hide floating quota bar"
    } else {
        "Show floating quota bar"
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum FloatingTrayAction {
    Show,
    Hide,
}

/// The toggle decision follows the floating window's real visibility, never
/// the menu label: a stale announcement can only leave a stale label, never
/// a wrong action.
fn floating_tray_action(window_visible: bool) -> FloatingTrayAction {
    if window_visible {
        FloatingTrayAction::Hide
    } else {
        FloatingTrayAction::Show
    }
}

fn floating_window_visible(app: &tauri::AppHandle) -> bool {
    app.get_webview_window("floating-quota")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

struct FloatingTrayItem(MenuItem<tauri::Wry>);

fn set_tray_floating_label(app: &tauri::AppHandle, visible: bool) {
    if let Some(state) = app.try_state::<FloatingTrayItem>() {
        let _ = state.0.set_text(floating_menu_label(visible));
    }
}

#[tauri::command]
fn open_main_window(app: tauri::AppHandle) {
    show_main_window(&app);
}

fn launched_from_autostart(argv: &[String]) -> bool {
    argv.iter().any(|arg| arg == AUTOSTART_ARG)
}

fn main() {
    let start_hidden = launched_from_autostart(&std::env::args().collect::<Vec<_>>());

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            codex::get_codex_usage,
            opencode_go::get_opencode_go_usage,
            zai::get_zai_usage,
            antigravity::get_antigravity_usage,
            grok::get_grok_usage,
            open_main_window,
            floating_drag::attach_floating_drag_lifecycle,
            floating_drag::detach_floating_drag_lifecycle,
            floating_drag::restore_floating_drag_origin,
            runtime::get_runtime_snapshot,
            runtime::request_refresh,
            runtime::request_refresh_on_reconnect,
            runtime::set_refresh_interval,
            runtime::set_quota_notifications_enabled,
            history::get_history,
            history::get_history_range,
            usage_analytics::get_usage_analytics,
            history::clear_history,
            history::import_legacy_history,
            local_data::clear_provider_cache,
            diagnostics::export_diagnostics,
            usage_export::pick_usage_export_directory,
            usage_export::export_usage_history
        ])
        // Must be the first registered plugin: relaunching the app focuses the
        // existing process instead of starting a second one.
        .plugin(single_instance_init(|app, argv, _cwd| {
            // An autostart relaunch while the app is already running must not
            // pop the dashboard open; it stays in the tray.
            if !launched_from_autostart(&argv) {
                show_main_window(app);
            }
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("LimitScope")
                .arg(AUTOSTART_ARG)
                .build(),
        )
        // Native delivery for the runtime's threshold notifications; the
        // webviews never send notifications themselves.
        .plugin(tauri_plugin_notification::init())
        // Rust-owned save dialog for the redacted diagnostics bundle. The
        // webview never sees or supplies an output path.
        .plugin(tauri_plugin_dialog::init())
        // Signed production updater: process relaunch plus the update check
        // plugin; the feed URL and public key live in tauri.conf.json.
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            // The shared provider runtime starts with the app (its scheduler
            // owns the refresh interval timer from now on); both webviews are
            // snapshot consumers.
            let runtime_handle = runtime::start(app.handle().clone());
            app.manage(runtime_handle);

            let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
            // A10: the tray exposes Show/Hide symmetrically as one item whose
            // label mirrors the floating window's visibility — including when
            // the main window is hidden.
            let floating_visible = floating_window_visible(app.handle());
            let floating = MenuItem::with_id(
                app,
                "toggle-floating",
                floating_menu_label(floating_visible),
                true,
                None::<&str>,
            )?;
            app.manage(FloatingTrayItem(floating.clone()));
            let refresh = MenuItem::with_id(app, "refresh", "Refresh", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(
                app,
                &[&open, &floating, &refresh, &separator, &quit],
            )?;

            TrayIconBuilder::with_id("main")
                .icon(
                    app.default_window_icon()
                        .expect("bundle icon is required for the tray icon")
                        .clone(),
                )
                .tooltip("LimitScope")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main_window(app),
                    "toggle-floating" => {
                        if floating_tray_action(floating_window_visible(app))
                            == FloatingTrayAction::Hide
                        {
                            if let Some(window) = app.get_webview_window("floating-quota") {
                                let _ = window.hide();
                            }
                            let _ = app.emit(TRAY_HIDE_FLOATING_EVENT, ());
                            set_tray_floating_label(app, false);
                        } else {
                            show_floating_window(app);
                            let _ = app.emit(TRAY_SHOW_FLOATING_EVENT, ());
                            set_tray_floating_label(app, true);
                        }
                    }
                    "refresh" => {
                        // The shared runtime is the only refresh owner: the
                        // tray acts on it directly; snapshot events tell the
                        // webviews what changed.
                        app.state::<runtime::RuntimeHandle>().request_refresh();
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // Left click opens the window; the context menu is reserved
                    // for right click on Windows.
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // The floating webview announces every visibility change it owns
            // (self-hide, close request, tray echo, initial restore) so the
            // tray label stays truthful without polling (A10).
            let label_handle = app.handle().clone();
            app.listen(FLOATING_VISIBLE_EVENT, move |event| {
                let visible = match event.payload() {
                    "true" => true,
                    "false" => false,
                    _ => return,
                };
                set_tray_floating_label(&label_handle, visible);
            });

            // The main window is created hidden (tauri.conf.json). A manual
            // launch shows it here; an autostart launch stays in the tray.
            if !start_hidden {
                show_main_window(app.handle());
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the app alive in the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running LimitScope");
}

#[cfg(test)]
mod tray_symmetry_tests {
    use super::{
        floating_menu_label, floating_tray_action, FloatingTrayAction,
        FLOATING_VISIBLE_EVENT, TRAY_HIDE_FLOATING_EVENT, TRAY_SHOW_FLOATING_EVENT,
    };

    #[test]
    fn tray_label_mirrors_floating_visibility() {
        assert_eq!(floating_menu_label(false), "Show floating quota bar");
        assert_eq!(floating_menu_label(true), "Hide floating quota bar");
    }

    #[test]
    fn tray_action_follows_real_window_state_not_the_label() {
        assert_eq!(floating_tray_action(false), FloatingTrayAction::Show);
        assert_eq!(floating_tray_action(true), FloatingTrayAction::Hide);
    }

    #[test]
    fn event_names_match_the_webview_listeners() {
        assert_eq!(TRAY_SHOW_FLOATING_EVENT, "tray://show-floating");
        assert_eq!(TRAY_HIDE_FLOATING_EVENT, "tray://hide-floating");
        assert_eq!(FLOATING_VISIBLE_EVENT, "floating://visible-changed");
    }

    #[test]
    fn hide_and_quit_stay_distinct_menu_names() {
        let hide = floating_menu_label(true);
        let show = floating_menu_label(false);
        assert_ne!(hide, show);
        assert!(hide.starts_with("Hide "));
        assert!(show.starts_with("Show "));
    }
}

