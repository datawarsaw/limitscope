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
//!   it reads no Claude Code setting file either. It writes no file of its
//!   own beyond the empty scratch directory it creates for the launch.
//! * The executable is resolved from a bounded list of known install
//!   locations and the PATH, never from the webview. It is started with a
//!   direct CreateProcessW call: no shell, no command-string
//!   interpolation, no caller-supplied argument.
//! * The child gets its own console, so Claude Code runs with a real
//!   terminal. Its arguments keep the session away from tools, MCP servers
//!   and project configuration, and its working directory is a fresh empty
//!   scratch directory, so the one-shot session cannot reach the user's
//!   projects.
//! * While that child lives, the attempt lock is held: a second click is
//!   refused with `alreadyRunning` instead of starting a second paid
//!   request. The lock is released when the child exits.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use serde::Serialize;

/// The only prompt this module can send. It is a constant: nothing from the
/// webview, the store, the environment or a file can change it.
const PING_PROMPT: &str = "Reply exactly OK.";

/// Fixed arguments for the one-shot session.
///
/// `--restricted` is deliberately NOT among them. Its own documentation
/// says it "ignores user, project and local settings files", and the
/// statusLine hook this whole feature exists for lives in the USER's
/// settings: with `--restricted` the hook could never run and the action
/// could never work. The flags below take its place and keep the session
/// harmless:
///
/// * `--setting-sources user` loads the user settings (hook included) and
///   no project or local setting, so no project configuration is applied;
/// * `--tools ""` disables every built-in tool;
/// * `--disable-slash-commands` disables skills and commands;
/// * `--strict-mcp-config` starts the session without any MCP server.
///
/// `--safe-mode` and `--bare` are not used either: both disable hooks.
///
/// The prompt stays the FIRST argument on purpose. `--tools` takes a
/// variable number of values, so a prompt placed after it would be read as
/// a tool name instead of as the prompt.
const PING_ARGUMENTS: [&str; 6] = [
    "--setting-sources",
    "user",
    "--tools",
    "",
    "--disable-slash-commands",
    "--strict-mcp-config",
];

/// The executable name Claude Code installs on Windows.
const EXECUTABLE_NAME: &str = "claude.exe";

/// The directory the scratch runs live in, below the app's own per-user
/// data directory. It is never the shared %TEMP% root, which holds
/// whatever other applications and earlier sessions left there.
const SCRATCH_DIRECTORY_NAME: &str = "claude-ping";

/// One bounded wait for the child process, and how many of them the waiter
/// thread performs at most. The waiter therefore always ends, and the
/// attempt lock keeps a session whose child outlived the bound until a
/// later attempt sees that the child is gone.
const WAIT_SLICE_MS: u32 = 60_000;
const WAIT_SLICES: u32 = 720;

/// The creation flags of the production launch: the child gets a console
/// of its own, so Claude Code runs with a real terminal.
#[cfg(windows)]
const LAUNCH_FLAGS: u32 = windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;
#[cfg(not(windows))]
const LAUNCH_FLAGS: u32 = 0;

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
    /// The isolated scratch directory could not be created, so the session
    /// was not started at all.
    ScratchUnavailable,
    /// A Claude session is still running, so a second paid request is
    /// refused.
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

/// The PATH entries that could hold the executable. A relative PATH entry
/// is never used: it would resolve against whatever directory the app
/// happens to run in, so it would not identify a program at all.
fn path_executables() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| path_executables_in(&value))
        .unwrap_or_default()
}

/// The candidates of one PATH value. Only absolute directories count.
fn path_executables_in(value: &std::ffi::OsStr) -> Vec<PathBuf> {
    std::env::split_paths(value)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(EXECUTABLE_NAME))
        .collect()
}

/// Resolves the Claude CLI from a bounded candidate list. Only an existing
/// file with the exact executable name at an ABSOLUTE path is accepted: a
/// script, a shortcut or a directory is never launched, a .cmd shim is not
/// a console program, and a relative path does not identify a program.
fn resolve(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
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

/// A retained Windows process handle. The handle stays valid until it is
/// closed, so the attempt lock can ask whether the child is still alive and
/// the waiter can watch it until it exits.
struct ProcessHandle {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// A kernel handle is a reference to an operating system object, not to
// memory: it may be used from any thread, and its ownership here is
// unique. The automatic Send/Sync derivation is wrong only because the
// raw handle type is a pointer.
#[cfg(windows)]
unsafe impl Send for ProcessHandle {}
#[cfg(windows)]
unsafe impl Sync for ProcessHandle {}

impl ProcessHandle {
    /// True once the child process has exited.
    #[cfg(windows)]
    fn has_exited(&self) -> bool {
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(self.handle, 0)
                == windows_sys::Win32::Foundation::WAIT_OBJECT_0
        }
    }

    #[cfg(not(windows))]
    fn has_exited(&self) -> bool {
        true
    }

    /// Waits for the child to exit. The wait is bounded: one slice at a
    /// time and at most `slices` of them, so the waiter thread ends.
    #[cfg(windows)]
    fn wait_for_exit(&self, slice_ms: u32, slices: u32) -> bool {
        for _ in 0..slices {
            let waited = unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(self.handle, slice_ms)
            };
            if waited == windows_sys::Win32::Foundation::WAIT_OBJECT_0 {
                return true;
            }
        }
        false
    }

    #[cfg(not(windows))]
    fn wait_for_exit(&self, _slice_ms: u32, _slices: u32) -> bool {
        true
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Starts the executable in a console window of its own and KEEPS the
/// process handle, so the attempt lock can hold the session until the
/// child exits.
#[cfg(windows)]
fn spawn_in_new_console(
    executable: &Path,
    working_directory: &Path,
    arguments: &[&str],
    creation_flags: u32,
) -> Result<Arc<ProcessHandle>, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW},
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
            creation_flags,
            std::ptr::null(),
            directory.as_ptr(),
            &startup,
            &mut information,
        )
    };
    if created == 0 {
        return Err("the Claude CLI could not be started".to_string());
    }
    // The thread handle is of no further use. The PROCESS handle is kept:
    // it is what the attempt lock and the waiter work with.
    unsafe {
        CloseHandle(information.hThread);
    }
    Ok(Arc::new(ProcessHandle {
        handle: information.hProcess,
    }))
}

#[cfg(not(windows))]
fn spawn_in_new_console(
    _executable: &Path,
    _working_directory: &Path,
    _arguments: &[&str],
    _creation_flags: u32,
) -> Result<Arc<ProcessHandle>, String> {
    Err("the Claude CLI can only be started on Windows".to_string())
}

/// The root of the scratch directories: the app's own per-user data
/// directory, never the shared %TEMP% root.
fn scratch_root() -> Option<PathBuf> {
    crate::claude_statusline::data_dir().map(|directory| directory.join(SCRATCH_DIRECTORY_NAME))
}

/// Creates a fresh, empty, isolated working directory for one session.
/// `create_dir` (not `create_dir_all`) is deliberate for the leaf: the
/// directory must not exist yet, so a session can never start in a
/// directory that something else filled.
fn create_scratch_directory(id: u64) -> Result<PathBuf, String> {
    let root = scratch_root().ok_or_else(|| "no per-user data directory".to_string())?;
    std::fs::create_dir_all(&root)
        .map_err(|_| "the scratch root is not available".to_string())?;
    let directory = root.join(format!("ping-{id}"));
    std::fs::create_dir(&directory)
        .map_err(|_| "the scratch directory is not available".to_string())?;
    Ok(directory)
}

/// Removes the scratch directory of a finished session. The removal is NOT
/// recursive: it succeeds only for an empty directory, and a directory
/// Claude Code wrote into is left in place.
fn remove_scratch_directory(directory: &Path) {
    let _ = std::fs::remove_dir(directory);
}

/// One live Claude session. The retained process handle is what makes the
/// attempt lock honest: it is held exactly as long as the child process
/// lives.
struct RunningSession {
    id: u64,
    scratch: PathBuf,
    process: Arc<ProcessHandle>,
}

impl RunningSession {
    fn has_exited(&self) -> bool {
        self.process.has_exited()
    }
}

/// The attempt lock. It holds the ONE live session, if there is one. It is
/// taken for the whole session, not just for the launch: a second click
/// while Claude Code is running is refused instead of starting a second
/// paid request.
static SESSION: Mutex<Option<RunningSession>> = Mutex::new(None);

/// Numbers the sessions, so the waiter can tell whether the lock still
/// holds its own session.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// Starts one session and takes the attempt lock for it. The lock is held
/// in the same critical section as the launch, so two clicks that arrive
/// together cannot both start a Claude session.
fn begin_attempt(
    executable: &Path,
) -> Result<(u64, Arc<ProcessHandle>, PathBuf), ClaudeRefreshStatus> {
    let mut slot = SESSION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot.as_ref().is_some_and(|session| !session.has_exited()) {
        return Err(ClaudeRefreshStatus::AlreadyRunning);
    }
    // The previous session is over: release it and its scratch directory.
    if let Some(finished) = slot.take() {
        remove_scratch_directory(&finished.scratch);
    }
    let id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
    let scratch = create_scratch_directory(id).map_err(|_| ClaudeRefreshStatus::ScratchUnavailable)?;
    let mut arguments = vec![PING_PROMPT];
    arguments.extend(PING_ARGUMENTS.iter().copied());
    let process =
        spawn_in_new_console(executable, &scratch, &arguments, LAUNCH_FLAGS).map_err(|_| {
            remove_scratch_directory(&scratch);
            ClaudeRefreshStatus::SpawnFailed
        })?;
    *slot = Some(RunningSession {
        id,
        scratch: scratch.clone(),
        process: process.clone(),
    });
    Ok((id, process, scratch))
}

/// Releases the attempt lock when the child exits. The wait is bounded, so
/// the thread always ends; when the bound passes while the child is still
/// alive the lock stays held, and a later attempt asks the retained handle
/// whether the child has finally exited.
fn wait_for_session(id: u64, process: Arc<ProcessHandle>, scratch: PathBuf) {
    if !process.wait_for_exit(WAIT_SLICE_MS, WAIT_SLICES) {
        return;
    }
    {
        let mut slot = SESSION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.as_ref().is_some_and(|session| session.id == id) {
            *slot = None;
        }
    }
    remove_scratch_directory(&scratch);
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
    match begin_attempt(&executable) {
        Ok((id, process, scratch)) => {
            // One bounded waiter thread per session. It watches the
            // retained handle, releases the lock when the child exits and
            // removes the empty scratch directory.
            std::thread::spawn(move || wait_for_session(id, process, scratch));
            ClaudeRefreshResult {
                status: ClaudeRefreshStatus::Launched,
            }
        }
        Err(status) => ClaudeRefreshResult { status },
    }
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
    fn only_an_absolute_candidate_is_ever_launched() {
        let directory = scratch("absolute");
        let present = directory.join(EXECUTABLE_NAME);
        std::fs::write(&present, b"stub").unwrap();

        // The same file, named relatively, identifies nothing: it would be
        // resolved against the directory the app happens to run in.
        assert_eq!(resolve(&[PathBuf::from(EXECUTABLE_NAME)]), None);
        assert_eq!(resolve(&[PathBuf::from("sub").join(EXECUTABLE_NAME)]), None);
        assert_eq!(resolve(&[present.clone()]), Some(present));

        // A relative PATH entry is skipped for the same reason.
        assert!(path_executables_in(std::ffi::OsStr::new(".")).is_empty());
        let mixed = path_executables_in(std::ffi::OsStr::new(&format!(
            ".;{}\tools;..\relative",
            std::env::temp_dir().display()
        )));
        assert!(mixed.iter().all(|candidate| candidate.is_absolute()));
        assert_eq!(mixed.len(), 1);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_ping_keeps_the_user_settings_and_drops_every_other_source() {
        let mut arguments = vec![PING_PROMPT];
        arguments.extend(PING_ARGUMENTS.iter().copied());
        let line = command_line(
            Path::new(r"C:\Program Files\Claude\claude.exe"),
            &arguments,
        );

        // The statusLine hook lives in the user's settings: they are loaded.
        assert!(line.contains("--setting-sources user"), "{line}");
        // No tool, no command, no MCP server, no project or local setting.
        assert!(line.contains("--tools \"\""), "{line}");
        assert!(line.contains("--disable-slash-commands"), "{line}");
        assert!(line.contains("--strict-mcp-config"), "{line}");
        // The flags that would disable the hook or the user's settings are
        // never used.
        for forbidden in ["--restricted", "--safe-mode", "--bare", "--print"] {
            assert!(!line.contains(forbidden), "{forbidden} must not be used: {line}");
        }
        // The prompt is sent once and stays in front of the variable-length
        // --tools option, so it cannot be read as a tool name.
        assert_eq!(line.matches("Reply exactly OK.").count(), 1);
        let prompt_at = line.find("Reply exactly OK.").unwrap();
        let tools_at = line.find("--tools").unwrap();
        assert!(prompt_at < tools_at, "{line}");
        assert_eq!(PING_PROMPT, "Reply exactly OK.");
        assert_eq!(
            PING_ARGUMENTS,
            [
                "--setting-sources",
                "user",
                "--tools",
                "",
                "--disable-slash-commands",
                "--strict-mcp-config"
            ]
        );
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
    fn the_scratch_directory_is_fresh_empty_and_not_the_shared_temp_root() {
        let first = create_scratch_directory(9_001).expect("scratch directory");
        let second = create_scratch_directory(9_002).expect("scratch directory");
        assert_ne!(first, second);
        // It is empty: a session cannot inherit instructions from a file.
        assert_eq!(std::fs::read_dir(&first).unwrap().count(), 0);
        // It lives below the app's own per-user data directory.
        let root = scratch_root().expect("scratch root");
        assert_eq!(first.parent().unwrap(), root);
        assert!(!first.starts_with(std::env::temp_dir()));

        // The cleanup is not recursive: a directory that received a file is
        // left in place.
        std::fs::write(first.join("note.txt"), b"x").unwrap();
        remove_scratch_directory(&first);
        assert!(first.exists());
        std::fs::remove_file(first.join("note.txt")).unwrap();
        remove_scratch_directory(&first);
        assert!(!first.exists());
        remove_scratch_directory(&second);
        assert!(!second.exists());
    }

    /// A harmless child process that stays alive for a moment, used to prove
    /// that the attempt lock is held for the whole life of the child.
    #[cfg(windows)]
    fn sleeping_child() -> Option<(PathBuf, Vec<&'static str>)> {
        let root = std::env::var_os("SystemRoot")?;
        let executable = PathBuf::from(root).join("System32").join("ping.exe");
        if !executable.is_file() {
            return None;
        }
        Some((executable, vec!["-n", "3", "127.0.0.1"]))
    }

    #[cfg(windows)]
    #[test]
    fn the_attempt_lock_is_held_until_the_child_exits() {
        let Some((executable, arguments)) = sleeping_child() else {
            return;
        };
        let directory = scratch("lock");
        let process = spawn_in_new_console(
            &executable,
            &directory,
            &arguments,
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
        )
        .expect("the test child starts");

        // The live child holds the lock: a second paid request is refused.
        {
            let mut slot = SESSION.lock().unwrap();
            *slot = Some(RunningSession {
                id: 42,
                scratch: directory.clone(),
                process: process.clone(),
            });
        }
        let refused = begin_attempt(Path::new(r"C:\does\not\exist\claude.exe")).err();
        assert_eq!(
            refused,
            Some(ClaudeRefreshStatus::AlreadyRunning),
            "a live session must refuse a second launch"
        );

        // Once the child is gone the lock is free again.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !process.has_exited() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(process.has_exited(), "the test child must exit");
        // The lock is free again, so the next attempt is allowed to start.
        let slot = SESSION.lock().unwrap();
        assert!(slot.as_ref().is_some_and(|session| session.has_exited()));
        drop(slot);
        assert_eq!(
            begin_attempt(Path::new(r"C:\does\not\exist\claude.exe")).err(),
            Some(ClaudeRefreshStatus::SpawnFailed),
            "a finished session must not block the next attempt"
        );
        let slot = SESSION.lock().unwrap();
        assert!(slot.is_none());
        // The finished session already removed its (empty) scratch
        // directory, so the test directory may be gone.
        let _ = std::fs::remove_dir_all(&directory);
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
            ClaudeRefreshStatus::ScratchUnavailable,
            ClaudeRefreshStatus::AlreadyRunning,
        ] {
            let wire = serde_json::to_string(&status).unwrap();
            assert!(!wire.contains("fresh"), "{wire}");
        }
        // The refusal of a second paid request is a status of its own.
        let wire = serde_json::to_string(&ClaudeRefreshStatus::AlreadyRunning).unwrap();
        assert_eq!(wire, "\"alreadyRunning\"");
    }
}
