//! IPC window authorization, exercised through Tauri's own IPC machinery.
//!
//! The manifest test in src/lib/ipcWindowAuthorization.test.ts proves that the
//! capability files and the permission manifest describe the intended grants.
//! These tests check the other half of the contract: they build the application
//! from the real tauri.conf.json context, so the ACL the installed Tauri
//! resolved when this crate was compiled is the one under test, create both
//! production window labels on tauri::test::MockRuntime, and drive
//! Webview::on_message the way the webview IPC handler does. A granted command
//! has to reach its handler; a command that no capability grants to the calling
//! window has to be rejected by the ACL before the handler runs.
//!
//! The ACL is baked in at compile time by tauri-build: the verdicts below come
//! from target/<profile>/build/<pkg>/out/{capabilities,acl-manifests}.json,
//! which is generated from src-tauri/capabilities and src-tauri/permissions.
//! Editing those files without a rebuild leaves a stale binary; when the
//! sources legitimately change, `cargo clean -p rate-limits` before trusting a
//! run. The matrix test therefore also pins the table in this file to the
//! sources it is compiled against.

use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{get_ipc_response, mock_builder, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use std::collections::BTreeSet;

/// Authorization sources compiled into the test binary. `include_str!` fails the
/// build if either file disappears, and the matrix test fails if the table
/// below drifts from them.
const PERMISSION_MANIFEST: &str = include_str!("../permissions/ipc-windows.toml");
const MAIN_RS: &str = include_str!("main.rs");

/// Local origin the webview reports on Windows.
const LOCAL_ORIGIN: &str = "http://tauri.localhost";

/// Every command the application registers, with the windows that a capability
/// grants it to: (command, granted to main, granted to floating-quota).
/// src-tauri/permissions/ipc-windows.toml together with
/// src-tauri/capabilities/*.json is the source of truth; this table mirrors
/// them so the runtime verdict can be compared against the intent.
const COMMANDS: &[(&str, bool, bool)] = &[
    ("get_codex_usage", false, false),
    ("get_opencode_go_usage", false, false),
    ("get_zai_usage", false, false),
    ("get_antigravity_usage", false, false),
    ("get_grok_usage", false, false),
    ("refresh_grok_bot_usage", true, true),
    ("refresh_opendesign_usage", true, true),
    ("open_main_window", false, true),
    ("attach_floating_drag_lifecycle", false, true),
    ("detach_floating_drag_lifecycle", false, true),
    ("restore_floating_drag_origin", false, true),
    ("attach_floating_monitor_observer", false, true),
    ("detach_floating_monitor_observer", false, true),
    ("get_runtime_snapshot", true, true),
    ("request_refresh", true, true),
    ("request_wallet_refresh", true, true),
    ("request_refresh_on_reconnect", true, true),
    ("set_refresh_interval", true, true),
    ("set_quota_notifications_enabled", true, false),
    ("set_wallet_monitoring", true, false),
    ("get_operational_settings", true, true),
    ("get_codex_accounts", true, true),
    ("set_codex_accounts", true, false),
    ("get_history", true, false),
    ("get_history_range", true, false),
    ("get_usage_analytics", true, false),
    ("get_usage_intelligence", true, true),
    ("set_usage_intelligence_enabled", true, false),
    ("clear_usage_intelligence", true, false),
    ("clear_history", true, false),
    ("import_legacy_history", true, false),
    ("clear_provider_cache", true, false),
    ("export_diagnostics", true, false),
    ("pick_usage_export_directory", true, false),
    ("export_usage_history", true, false),
];

/// Production command wrappers that do not take a Wry-typed handle, so they can
/// be dispatched on the mock runtime. Reaching them proves the ACL let the call
/// through; they then fail on the runtime state this test app never registers.
fn production_subset_handler(
) -> impl Fn(tauri::ipc::Invoke<MockRuntime>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        crate::runtime::get_runtime_snapshot,
        crate::runtime::get_codex_accounts,
        crate::history::get_history,
        crate::history::clear_history,
        crate::local_data::clear_provider_cache,
    ]
}

/// Echo dispatcher for the full command matrix. It never runs application
/// code; the ACL check in Webview::on_message already decided whether the call
/// is allowed, and this handler only reports the command it received.
fn echo_invoke_handler(
) -> impl Fn(tauri::ipc::Invoke<MockRuntime>) -> bool + Send + Sync + 'static {
    |invoke: tauri::ipc::Invoke<MockRuntime>| {
        let command = invoke.message.command().to_string();
        invoke.resolver.resolve(command);
        true
    }
}

fn build_app<F>(handler: F) -> tauri::App<MockRuntime>
where
    F: Fn(tauri::ipc::Invoke<MockRuntime>) -> bool + Send + Sync + 'static,
{
    mock_builder()
        .invoke_handler(handler)
        .build(tauri::generate_context!())
        .expect("build the mock app from the real tauri.conf.json context")
}

fn window(app: &tauri::App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
    if let Some(existing) = app.get_webview_window(label) {
        return existing;
    }
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .build()
        .expect("create the webview window")
}

fn dispatch_from(
    webview: &WebviewWindow<MockRuntime>,
    cmd: &str,
    url: &str,
) -> Result<InvokeResponseBody, String> {
    let request = InvokeRequest {
        cmd: cmd.to_string(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: url.parse().expect("valid IPC origin URL"),
        body: InvokeBody::default(),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    };
    get_ipc_response(webview, request).map_err(|error| match error {
        // The rejection payload is a JSON string; unwrap it so assertions see
        // the plain ACL message instead of its escaped JSON form.
        serde_json::Value::String(message) => message,
        other => other.to_string(),
    })
}

fn dispatch(webview: &WebviewWindow<MockRuntime>, cmd: &str) -> Result<InvokeResponseBody, String> {
    dispatch_from(webview, cmd, LOCAL_ORIGIN)
}

/// How Tauri reports a denial depends on where the command sits in the
/// resolved manifest. A command that some capability grants to another window
/// is rejected with the "not allowed on window ..." form, which names the
/// calling window. A command that no permission mentions at all never gets
/// resolved for anybody and is rejected with "Command not found". A command
/// whose permission exists but that no capability references is rejected with
/// "Permissions associated with this command: ...". All three are ACL
/// rejections; the distinction is what the manifest says.
#[derive(Debug, PartialEq, Eq)]
enum Denial {
    /// Granted to some window, rejected for this one by window scope.
    WindowScope,
    /// No permission mentions the command; the ACL never resolved it.
    Unresolved,
    /// A permission exists for the command but no capability references it.
    ManifestMismatch,
}

/// What one dispatch proved: the ACL let the call through, or how it refused.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Allowed,
    Denied(Denial),
}

fn classify_denial(error: &str) -> Option<Denial> {
    if error.contains("not allowed on window") {
        Some(Denial::WindowScope)
    } else if error.contains("Command not found") {
        Some(Denial::Unresolved)
    } else if error.contains("Permissions associated with this command") {
        Some(Denial::ManifestMismatch)
    } else {
        None
    }
}

/// The expected shape of a denial: a command granted to another window is
/// refused by window scope, a command granted to nobody is unresolved.
fn expected_denial(granted_anywhere: bool) -> Denial {
    if granted_anywhere {
        Denial::WindowScope
    } else {
        Denial::Unresolved
    }
}

fn assert_acl_denied(webview: &WebviewWindow<MockRuntime>, cmd: &str, granted_anywhere: bool) {
    let label = webview.label().to_string();
    let error = dispatch(webview, cmd)
        .expect_err(&format!("command {cmd} must be denied for window {label}"));
    let denial = classify_denial(&error).unwrap_or_else(|| {
        panic!("command {cmd} on window {label} failed for a reason other than the ACL: {error}")
    });
    assert_eq!(
        denial,
        expected_denial(granted_anywhere),
        "command {cmd} on window {label} was denied in an unexpected shape: {error}"
    );
    if denial == Denial::WindowScope {
        assert!(
            error.contains(&format!("\"{label}\"")),
            "the ACL rejection should name the calling window {label}: {error}"
        );
    }
}

/// The ACL passed, so whatever the command reported afterwards came from the
/// handler (echo, missing argument or missing state) rather than from the ACL.
fn assert_reached_handler(webview: &WebviewWindow<MockRuntime>, cmd: &str) {
    let label = webview.label().to_string();
    if let Err(error) = dispatch(webview, cmd) {
        assert!(
            classify_denial(&error).is_none(),
            "command {cmd} was rejected by the ACL for window {label}: {error}"
        );
    }
}

/// Dispatches one (command, window) pair and returns the verdict so the matrix
/// test can count what the runtime actually decided.
fn assert_command(
    webview: &WebviewWindow<MockRuntime>,
    command: &str,
    allowed: bool,
    granted_anywhere: bool,
) -> Verdict {
    let label = webview.label().to_string();
    match dispatch(webview, command) {
        Ok(body) => {
            assert!(
                allowed,
                "command {command} reached its handler from window {label}, but no capability grants it"
            );
            let ran: String = body.deserialize().expect("the echo handler returns its name");
            assert_eq!(
                ran, command,
                "the dispatcher ran a different command for {command}"
            );
            Verdict::Allowed
        }
        Err(error) => {
            assert!(
                !allowed,
                "command {command} must reach its handler from window {label}: {error}\n\
                 (the ACL is compiled in by tauri-build; if the capability files already grant \
                 this command, rerun with `cargo clean -p rate-limits` to drop a stale ACL build)"
            );
            let denial = classify_denial(&error).unwrap_or_else(|| {
                panic!(
                    "command {command} on window {label} failed for a reason other than the ACL: {error}"
                )
            });
            assert_eq!(
                denial,
                expected_denial(granted_anywhere),
                "command {command} on window {label} was denied in an unexpected shape: {error}"
            );
            if denial == Denial::WindowScope {
                assert!(
                    error.contains(&format!("\"{label}\"")),
                    "the ACL rejection should name the calling window {label}: {error}"
                );
            }
            Verdict::Denied(denial)
        }
    }
}

/// Commands main.rs actually hands to Tauri's invoke handler.
fn registered_commands() -> BTreeSet<&'static str> {
    let body = MAIN_RS
        .split_once("generate_handler![")
        .expect("main.rs registers its commands with generate_handler!")
        .1
        .split_once("])")
        .expect("the generate_handler! list is closed")
        .0;
    body.split(|character: char| character.is_whitespace() || character == ',')
        .filter(|token| !token.is_empty())
        .map(|token| token.rsplit("::").next().unwrap_or(token))
        .collect()
}

/// Commands the application permission manifest allows at all, whoever holds
/// them. A command missing here is registered but unreachable.
fn manifest_granted_commands() -> BTreeSet<&'static str> {
    let mut granted = BTreeSet::new();
    let mut rest = PERMISSION_MANIFEST;
    while let Some((_, after)) = rest.split_once("commands.allow") {
        let Some(open) = after.find('[') else { break };
        let Some(close) = after.find(']') else { break };
        for quoted in after[open + 1..close].split('"').skip(1).step_by(2) {
            granted.insert(quoted);
        }
        rest = &after[close..];
    }
    granted
}

#[test]
fn production_commands_follow_the_window_capabilities() {
    let app = build_app(production_subset_handler());
    let main = window(&app, "main");
    let floating = window(&app, "floating-quota");

    // Granted to both windows and state-free enough to reach the handler.
    for webview in [&main, &floating] {
        for command in ["get_runtime_snapshot", "get_codex_accounts"] {
            let error =
                dispatch(webview, command).expect_err("no runtime state is managed in this test app");
            assert!(
                error.contains("state not managed"),
                "command {command} on window {} should fail on the missing state: {error}",
                webview.label()
            );
        }
    }

    // Dashboard-only history reads and cache clears: the dashboard reaches the
    // real command body, the floating bar never gets past the ACL.
    for command in ["get_history", "clear_history", "clear_provider_cache"] {
        assert_reached_handler(&main, command);
        assert_acl_denied(&floating, command, true);
        println!("production dispatch: {command} -> main reached handler, floating-quota denied by window scope");
    }
}

#[test]
fn every_registered_command_is_granted_only_where_the_capabilities_say() {
    // The table is a claim about the sources; check the claim first.
    let table: BTreeSet<&str> = COMMANDS.iter().map(|(command, _, _)| *command).collect();
    assert_eq!(
        table.len(),
        COMMANDS.len(),
        "the table lists every command exactly once"
    );
    assert_eq!(
        table,
        registered_commands(),
        "the table must list exactly the commands main.rs registers"
    );
    let table_granted: BTreeSet<&str> = COMMANDS
        .iter()
        .filter(|(_, main, floating)| *main || *floating)
        .map(|(command, _, _)| *command)
        .collect();
    assert_eq!(
        table_granted,
        manifest_granted_commands(),
        "the manifest must grant exactly the commands the table marks as granted"
    );

    assert_eq!(COMMANDS.len(), 35, "the application registers 35 commands");
    assert_eq!(
        COMMANDS.iter().filter(|(_, main, _)| *main).count(),
        24,
        "the dashboard is granted 24 application commands"
    );
    assert_eq!(
        COMMANDS.iter().filter(|(_, _, floating)| *floating).count(),
        16,
        "the floating bar is granted 16 application commands"
    );
    assert_eq!(
        COMMANDS
            .iter()
            .filter(|(_, main, floating)| *main || *floating)
            .count(),
        30,
        "30 of the 35 registered commands are granted to at least one window"
    );

    let app = build_app(echo_invoke_handler());
    let main = window(&app, "main");
    let floating = window(&app, "floating-quota");

    let mut allowed = 0usize;
    let mut window_scope = 0usize;
    let mut unresolved = 0usize;
    for &(command, main_allowed, floating_allowed) in COMMANDS {
        let granted_anywhere = main_allowed || floating_allowed;
        for (webview, expected_allowed) in [(&main, main_allowed), (&floating, floating_allowed)] {
            match assert_command(webview, command, expected_allowed, granted_anywhere) {
                Verdict::Allowed => allowed += 1,
                Verdict::Denied(Denial::WindowScope) => window_scope += 1,
                Verdict::Denied(Denial::Unresolved | Denial::ManifestMismatch) => unresolved += 1,
            }
        }
    }

    println!(
        "IPC ACL matrix: {} dispatches ({} commands x 2 windows) -> {allowed} allowed, \
         {window_scope} denied by window scope, {unresolved} unresolved",
        COMMANDS.len() * 2,
        COMMANDS.len()
    );
    assert_eq!(
        allowed + window_scope + unresolved,
        COMMANDS.len() * 2,
        "every dispatch produced one verdict"
    );
    assert_eq!(allowed, 40, "24 dashboard grants + 16 floating grants");
    assert_eq!(
        window_scope, 20,
        "the 14 dashboard-only and 6 floating-only commands are refused on the other window"
    );
    assert_eq!(
        unresolved, 10,
        "the five legacy provider fetches are unreachable from both windows"
    );
}

#[test]
fn remote_origin_cannot_reach_app_commands() {
    let app = build_app(echo_invoke_handler());
    let main = window(&app, "main");

    let error = dispatch_from(&main, "get_runtime_snapshot", "https://attacker.example/")
        .expect_err("a remote origin must not reach application commands");
    assert!(
        error.contains("not allowed"),
        "remote origin rejection: {error}"
    );
    println!("remote origin: get_runtime_snapshot from https://attacker.example/ -> {error}");
}

