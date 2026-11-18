//! On-demand Claude CLI ping for the passive statusLine provider.
//!
//! The Claude Code provider is passive by design: it learns the
//! subscription's 5-hour and 7-day windows only when Claude Code runs its
//! documented statusLine hook (see claude_statusline.rs). This module adds
//! ONE explicit user action that makes that hook run on demand: it starts
//! the already installed Claude CLI in a visible console window with a
//! single fixed prompt.
//!
//! What the action does and does not do:
//!
//! * It sends exactly ONE fixed prompt ("Reply exactly OK."), which costs
//!   one subscription request. The dashboard says so before the click.
//! * It never polls, schedules or retries, and it never measures or
//!   asserts a quota value. The provider runtime owns the derived documents
//!   and refreshes them on its own schedule; this module only starts Claude.
//! * It reads no credential, session file, transcript or private API, and
//!   it writes no file of its own.
//! * The executable is resolved from a bounded list of known install
//!   locations and the PATH, never from the webview. It is started with a
//!   direct CreateProcessW call: no shell, no command-string
//!   interpolation, no caller-supplied argument.
//! * The child gets its own console, so Claude Code runs with a real
//!   terminal. Its arguments are restricted (--restricted,
//!   --strict-mcp-config) and its working directory is the scratch temp
//!   directory, so the one-shot session cannot reach the user's projects.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use serde::Serialize;

/// The only prompt this module can send. It is a constant: nothing from the
/// webview, the store, the environment or a file can change it.
const PING_PROMPT: &str = "Reply exactly OK.";
/// Fixed arguments that keep the one-shot session away from tools and MCP
/// servers. Fixed for the same reason the prompt is.
const PING_ARGUMENTS: [&str; 2] = ["--restricted", "--strict-mcp-config"];
/// The executable name Claude Code installs on Windows.
const EXECUTABLE_NAME: &str = "claude.exe";

/// One attempt at a time: a second click while an attempt runs is refused
/// instead of starting a second Claude session.
static ATTEMPT_RUNNING: AtomicBool = AtomicBool::new(false);

/// What one attempt did. Every variant describes the ATTEMPT, never the
/// freshness of a quota value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeRefreshStatus {
    /// The Claude CLI was started. Its one request is on its way; the quota
    /// windows follow when Claude Code reports them.
    Launched,
    /// No Claude CLI could be resolved.
    NotFound,
    /// The CLI was found but could not be started.
    SpawnFailed,
    /// Another attempt is still running.
    AlreadyRunning,
}

/// The result of one attempt.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeRefreshResult {
    pub status: ClaudeRefreshStatus,
}

/// The install locations this module knows about. Both are fixed by the
/// environment, never by the webview.
fn known_install_locations() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join(EXECUTABLE_NAME),
        );
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        candidates.push(PathBuf::from(profile).join(".local").join("bin").join(EXECUTABLE_NAME));
    }
    candidates
}

/// The PATH entries that could hold the executable.
fn path_executables() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| {
            std::env::split_paths(&value)
                .map(|directory| directory.join(EXECUTABLE_NAME))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolves the Claude CLI from a bounded candidate list. Only an existing
/// file with the exact executable name is accepted: a script, a shortcut or
/// a directory is never launched, and a .cmd shim is not a console
/// program, so it is left to the operator's own PATH setup.
fn resolve(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
}

fn resolve_claude_executable() -> Option<PathBuf> {
    let mut candidates = known_install_locations();
    candidates.extend(path_executables());
    resolve(&candidates)
}

/// Quotes one argument the way the Windows command-line parser expects.
fn quote_argument(argument: &str) -> String {
    if !argument.is_empty()
        && argument
            .chars()
            .all(|character| !character.is_whitespace() && character != '"')
    {
        return argument.to_string();
    }
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for character in argument.chars() {
        match character {
            '\\' => {
                backslashes += 1;
                quoted.push('\\');
            }
            '"' => {
                for _ in 0..=backslashes {
                    quoted.push('\\');
                }
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                quoted.push(character);
            }
        }
    }
    for _ in 0..backslashes {
        quoted.push('\\');
    }
    quoted.push('"');
    quoted
}

/// The command line for the child process. It is built from the resolved
/// executable and the module's own constants; nothing else reaches it.
fn command_line(executable: &Path, arguments: &[&str]) -> String {
    let mut parts = vec![quote_argument(&executable.to_string_lossy())];
    parts.extend(arguments.iter().map(|argument| quote_argument(argument)));
    parts.join(" ")
}

/// Starts the executable in a console window of its own.
#[cfg(windows)]
fn spawn_in_new_console(
    executable: &Path,
    working_directory: &Path,
    arguments: &[&str],
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CreateProcessW, CREATE_NEW_CONSOLE, PROCESS_INFORMATION, STARTUPINFOW,
        },
    };

    let mut application: Vec<u16> = executable.as_os_str().encode_wide().collect();
    application.push(0);
    let mut line: Vec<u16> = command_line(executable, arguments).encode_utf16().collect();
    line.push(0);
    let mut directory: Vec<u16> = working_directory.as_os_str().encode_wide().collect();
    directory.push(0);

    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // STARTF_USESTDHANDLES is deliberately NOT set. LimitScope is a windowed
    // process without a console, so it has no standard handles to pass on;
    // the child must take the handles of the console it was just given.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NEW_CONSOLE,
            std::ptr::null(),
            directory.as_ptr(),
            &startup,
            &mut information,
        )
    };
    if created == 0 {
        return Err("the Claude CLI could not be started".to_string());
    }
    // The child keeps running on its own; only its handles are closed here.
    unsafe {
        CloseHandle(information.hThread);
        CloseHandle(information.hProcess);
    }
    Ok(())
}

#[cfg(not(windows))]
fn spawn_in_new_console(
    _executable: &Path,
    _working_directory: &Path,
    _arguments: &[&str],
) -> Result<(), String> {
    Err("the Claude CLI can only be started on Windows".to_string())
}

/// Releases the attempt guard even when the attempt fails.
struct AttemptGuard;

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        ATTEMPT_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// The one explicit on-demand action: start the installed Claude CLI once,
/// with one fixed prompt, in a visible console of its own. The provider
/// runtime keeps owning the derived documents; this command only starts
/// Claude, and reports what that attempt did.
#[tauri::command]
pub async fn refresh_claude_usage() -> ClaudeRefreshResult {
    let Some(executable) = resolve_claude_executable() else {
        return ClaudeRefreshResult {
            status: ClaudeRefreshStatus::NotFound,
        };
    };
    if ATTEMPT_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return ClaudeRefreshResult {
            status: ClaudeRefreshStatus::AlreadyRunning,
        };
    }
    let _guard = AttemptGuard;

    // The scratch directory is the system temp directory: it holds no
    // project content, so the one-shot session cannot read the user's work.
    let working_directory = std::env::temp_dir();
    let mut arguments = vec![PING_PROMPT];
    arguments.extend(PING_ARGUMENTS.iter().copied());
    let status = match spawn_in_new_console(&executable, &working_directory, &arguments) {
        Ok(()) => ClaudeRefreshStatus::Launched,
        Err(_) => ClaudeRefreshStatus::SpawnFailed,
    };
    ClaudeRefreshResult { status }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("limitscope-claude-cli-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    #[test]
    fn resolve_takes_the_first_existing_candidate_and_nothing_else() {
        let directory = scratch("resolve");
        let present = directory.join(EXECUTABLE_NAME);
        std::fs::write(&present, b"stub").unwrap();
        let missing = directory.join("missing.exe");
        let as_directory = directory.join("folder.exe");
        std::fs::create_dir_all(&as_directory).unwrap();

        assert_eq!(
            resolve(&[missing.clone(), present.clone()]),
            Some(present.clone())
        );
        // A directory that happens to carry the executable name is not a
        // program, and a missing file is not either.
        assert_eq!(resolve(&[missing.clone()]), None);
        assert_eq!(resolve(&[as_directory.clone()]), None);
        assert_eq!(resolve(&[]), None);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_ping_sends_one_fixed_prompt_and_no_caller_input() {
        assert_eq!(PING_PROMPT, "Reply exactly OK.");
        assert_eq!(PING_ARGUMENTS, ["--restricted", "--strict-mcp-config"]);
        let line = command_line(
            Path::new(r"C:\Program Files\Claude\claude.exe"),
            &[PING_PROMPT, PING_ARGUMENTS[0], PING_ARGUMENTS[1]],
        );
        assert_eq!(
            line,
            r#""C:\Program Files\Claude\claude.exe" "Reply exactly OK." --restricted --strict-mcp-config"#
        );
        // No shell metacharacter survives into the command line, and the
        // prompt stays one quoted argument.
        assert!(!line.contains('&') && !line.contains('|') && !line.contains('>'));
        assert_eq!(line.matches("Reply exactly OK.").count(), 1);
    }

    #[test]
    fn quoting_follows_the_windows_command_line_rules() {
        assert_eq!(quote_argument("plain"), "plain");
        assert_eq!(quote_argument("two words"), "\"two words\"");
        assert_eq!(quote_argument(""), "\"\"");
        assert_eq!(quote_argument("a\"b"), "\"a\\\"b\"");
        // Inside quotes a trailing backslash is doubled, so it cannot
        // escape the closing quote.
        assert_eq!(quote_argument("two words\\"), "\"two words\\\\\"");
        // An argument that needs no quoting keeps its backslash literal.
        assert_eq!(quote_argument("trail\\"), "trail\\");
    }

    #[test]
    fn one_attempt_runs_at_a_time() {
        assert!(ATTEMPT_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok());
        assert!(
            ATTEMPT_RUNNING
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err(),
            "a second attempt must not start while one runs"
        );
        ATTEMPT_RUNNING.store(false, Ordering::SeqCst);
    }

    #[test]
    fn the_result_never_claims_freshness() {
        let result = ClaudeRefreshResult {
            status: ClaudeRefreshStatus::Launched,
        };
        let wire = serde_json::to_string(&result).unwrap();
        assert_eq!(wire, "{\"status\":\"launched\"}");
        assert!(!wire.contains("fresh"), "{wire}");
        for status in [
            ClaudeRefreshStatus::Launched,
            ClaudeRefreshStatus::NotFound,
            ClaudeRefreshStatus::SpawnFailed,
            ClaudeRefreshStatus::AlreadyRunning,
        ] {
            let wire = serde_json::to_string(&status).unwrap();
            assert!(!wire.contains("fresh"), "{wire}");
        }
    }
}
