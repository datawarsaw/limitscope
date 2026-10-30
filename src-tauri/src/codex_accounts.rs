//! Per-account OpenAI/Codex configuration (multi-account monitoring).
//!
//! LimitScope can monitor two independent ChatGPT accounts side by side. Each
//! account owns:
//!
//! - a **credential reference**: the Codex home directory that holds that
//!   account's own `auth.json`. The account owner logs that account in with
//!   the official Codex CLI (`codex login` inside its own `CODEX_HOME`); the
//!   app only ever reads the file, never writes it, never copies it and
//!   never shares one account's file with another identity;
//! - an **alias**: the user-recognizable name of the account. It is display
//!   only and never appears beside the compact bar's percentages;
//! - a **monitoring flag**: `false` pauses collection. The account keeps its
//!   segment and its last known reading, but the runtime stops polling it.
//!
//! The account id IS the provider id, so every provider-keyed subsystem
//! (last-good retention, history, cooldowns, notification state) isolates
//! the two accounts automatically. The primary account keeps the historical
//! id `openai-codex`, so an existing single-account installation keeps its
//! history, preferences and notifications continuous. The second account
//! uses the account-scoped id `openai-codex:secondary`.
//!
//! The store persists one bounded JSON file in the app-data directory,
//! written temp-then-rename like the sibling stores. A corrupt or
//! foreign-version file degrades to the defaults: one primary account, no
//! second account, both monitored — exactly the pre-multi-account behavior.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Versioned file name, alongside the other app-data artifacts.
pub const CODEX_ACCOUNTS_FILE_NAME: &str = "codex-accounts-v1.json";

const SCHEMA_VERSION: u32 = 1;

/// The primary account's stable id. It is the provider id too, and it is
/// the id every pre-multi-account installation already uses.
pub const PRIMARY_ACCOUNT_ID: &str = "openai-codex";

/// The second account's stable id. Account-scoped on purpose: everything
/// that only knows the provider resolves it back to `openai-codex` by
/// dropping the scope suffix.
pub const SECONDARY_ACCOUNT_ID: &str = "openai-codex:secondary";

/// The known account ids, in registry order.
pub const KNOWN_ACCOUNT_IDS: [&str; 2] = [PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID];

/// One configured Codex account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAccountConfig {
    /// Stable internal identifier; one of `PRIMARY_ACCOUNT_ID` or
    /// `SECONDARY_ACCOUNT_ID`. Never user-editable and never reused for a
    /// different identity.
    pub account_id: String,
    /// The user-facing name of the account. May be empty; an empty alias
    /// simply means the account is not given a name yet.
    #[serde(default)]
    pub alias: String,
    /// The Codex home that holds this account's own `auth.json`. An empty
    /// string means "the default Codex home" for the primary account, and
    /// "not configured" for the second one (it has no default of its own).
    #[serde(default)]
    pub codex_home: String,
    /// Whether the runtime still collects this account. `false` is the
    /// "Pause monitoring" state: the reading freezes, the segment stays.
    #[serde(default = "default_monitored")]
    pub monitored: bool,
}

fn default_monitored() -> bool {
    true
}

impl CodexAccountConfig {
    /// The default entry for one known account id.
    pub fn default_for(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            alias: String::new(),
            codex_home: String::new(),
            monitored: true,
        }
    }

    /// Whether this account can actually be read.
    ///
    /// The primary account always can: with no explicit home it falls back
    /// to the ambient `CODEX_HOME` and then to `~/.codex`, which is the
    /// single-login behavior this app has always had. The second account
    /// has no default of its own — reading it from the primary account's
    /// home would silently duplicate one identity, so it counts as
    /// configured only when it names its own Codex home.
    pub fn is_configured(&self) -> bool {
        if self.account_id == PRIMARY_ACCOUNT_ID {
            return true;
        }
        !self.codex_home.trim().is_empty()
    }

    /// The credential reference as the Codex adapter takes it: `None` means
    /// "use the default Codex home".
    pub fn codex_home_option(&self) -> Option<&str> {
        let home = self.codex_home.trim();
        if home.is_empty() {
            None
        } else {
            Some(home)
        }
    }
}

/// The default configuration: one primary account, no second account.
pub fn default_accounts() -> Vec<CodexAccountConfig> {
    KNOWN_ACCOUNT_IDS
        .iter()
        .map(|id| CodexAccountConfig::default_for(id))
        .collect()
}

/// Normalizes any incoming list into exactly the known accounts, in
/// registry order.
///
/// Unknown ids are dropped, duplicates keep the first occurrence, and a
/// known account that is missing falls back to its defaults. A malformed
/// field never voids a valid sibling.
pub fn normalize_accounts(accounts: &[CodexAccountConfig]) -> Vec<CodexAccountConfig> {
    let mut out: Vec<CodexAccountConfig> = Vec::with_capacity(KNOWN_ACCOUNT_IDS.len());
    for id in KNOWN_ACCOUNT_IDS.iter() {
        let found = accounts.iter().find(|account| account.account_id == *id);
        let mut account = found
            .cloned()
            .unwrap_or_else(|| CodexAccountConfig::default_for(id));
        account.account_id = (*id).to_string();
        account.alias = account.alias.trim().to_string();
        account.codex_home = account.codex_home.trim().to_string();
        out.push(account);
    }
    out
}

/// Rejects a configuration in which both Codex accounts resolve to the same
/// credential home. The comparison uses canonical paths where possible and a
/// lexical fallback for paths that do not exist yet, so alternate separators,
/// `.`/`..`, trailing separators, and Windows path aliases cannot bypass it.
///
/// The environment is passed in so tests never consult the process home
/// directory, `CODEX_HOME`, or the real working directory.
fn validate_codex_home_collisions_in(
    accounts: &[CodexAccountConfig],
    ambient_codex_home: Option<&str>,
    user_home: &Path,
    current_dir: &Path,
) -> io::Result<()> {
    let Some(primary) = accounts
        .iter()
        .find(|account| account.account_id == PRIMARY_ACCOUNT_ID)
    else {
        return Ok(());
    };
    let Some(secondary) = accounts
        .iter()
        .find(|account| account.account_id == SECONDARY_ACCOUNT_ID)
    else {
        return Ok(());
    };
    if secondary.codex_home.trim().is_empty() {
        return Ok(());
    }

    let primary_home = if primary.codex_home.trim().is_empty() {
        default_primary_codex_home(ambient_codex_home, user_home, current_dir)
    } else {
        PathBuf::from(primary.codex_home.trim())
    };
    let secondary_home = PathBuf::from(secondary.codex_home.trim());
    let primary_key = resolve_codex_home(&primary_home, current_dir);
    let secondary_key = resolve_codex_home(&secondary_home, current_dir);
    if codex_homes_collide(&primary_key, &secondary_key) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Primary and secondary Codex accounts must use different Codex home directories",
        ));
    }
    Ok(())
}

fn default_primary_codex_home(
    ambient_codex_home: Option<&str>,
    user_home: &Path,
    current_dir: &Path,
) -> PathBuf {
    if let Some(ambient) = ambient_codex_home
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        let ambient = absolute_codex_home_path(Path::new(ambient), current_dir);
        if ambient.is_dir() {
            return ambient;
        }
    }
    user_home.join(".codex")
}

fn absolute_codex_home_path(path: &Path, current_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    }
}

/// A Codex home resolved for a collision comparison.
///
/// The comparison is deliberately split in two. The part of the path that
/// exists on disk is compared through its canonical path: the canonical form
/// carries the spelling the filesystem actually uses, so one directory keeps
/// one identity however it is written, while two real directories that differ
/// only in case inside a case-sensitive directory keep their own identity.
/// The part that does not exist yet is compared component by component.
struct ResolvedCodexHome {
    /// The volume or share the home lives on. Volume names are not
    /// case-sensitive, so this is compared in lower case.
    root: String,
    /// The canonical path of the deepest existing ancestor, empty when no
    /// component of the path exists.
    existing: PathBuf,
    /// The components below `existing` that do not exist yet.
    pending: Vec<String>,
}

fn resolve_codex_home(path: &Path, current_dir: &Path) -> ResolvedCodexHome {
    let absolute = absolute_codex_home_path(path, current_dir);
    let (root, components) = lexical_codex_home_components(&absolute);

    let mut existing = PathBuf::new();
    let mut existing_len = 0usize;
    for index in 0..components.len() {
        let mut candidate = PathBuf::new();
        if !root.is_empty() {
            candidate.push(&root);
        }
        for component in &components[..=index] {
            candidate.push(component);
        }
        // A component that does not exist rules out every deeper one, so the
        // first failure ends the walk.
        let Ok(canonical) = fs::canonicalize(&candidate) else {
            break;
        };
        existing = canonical;
        existing_len = index + 1;
    }

    ResolvedCodexHome {
        root,
        existing,
        pending: components[existing_len..].to_vec(),
    }
}

/// The lexical form of a path: absolute, with `.` and `..` resolved and with
/// the trailing dots and spaces the filesystem ignores trimmed from the
/// component names. The case of the components is preserved: only the
/// filesystem can tell a case-sensitive directory apart.
fn lexical_codex_home_components(path: &Path) -> (String, Vec<String>) {
    let mut root = String::new();
    let mut components: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                root = prefix.as_os_str().to_string_lossy().to_lowercase();
            }
            Component::RootDir => {
                root.push(std::path::MAIN_SEPARATOR);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop();
            }
            Component::Normal(part) => {
                let name = normalize_codex_home_component(&part.to_string_lossy());
                if !name.is_empty() {
                    components.push(name);
                }
            }
        }
    }
    (root, components)
}

#[cfg(windows)]
fn normalize_codex_home_component(component: &str) -> String {
    // Win32 ignores trailing dots and spaces in a directory name.
    component
        .trim_end_matches(|character| character == '.' || character == ' ')
        .to_string()
}

#[cfg(not(windows))]
fn normalize_codex_home_component(component: &str) -> String {
    component.to_string()
}

/// Whether both accounts would end up using the same credential home.
fn codex_homes_collide(primary: &ResolvedCodexHome, secondary: &ResolvedCodexHome) -> bool {
    if primary.root != secondary.root {
        return false;
    }
    if primary.existing != secondary.existing {
        return false;
    }
    if primary.pending == secondary.pending {
        return true;
    }
    if !primary
        .pending
        .iter()
        .zip(&secondary.pending)
        .all(|(left, right)| left.eq_ignore_ascii_case(right))
    {
        return false;
    }
    // Only the case differs. On a case-insensitive volume that is one entry;
    // in a case-sensitive directory it is two. The directory itself decides,
    // and the conservative reading stands when it cannot be asked.
    directory_resolves_case_insensitively(&primary.existing).unwrap_or(true)
}

/// Whether the directory resolves an entry written in a different case to the
/// same entry. `None` when the directory cannot answer: no entry to compare,
/// or no access to it.
fn directory_resolves_case_insensitively(directory: &Path) -> Option<bool> {
    let entries = fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(variant) = case_variant(&name) else {
            continue;
        };
        let original = fs::canonicalize(directory.join(&name)).ok();
        let spelled_differently = fs::canonicalize(directory.join(&variant)).ok();
        match (original, spelled_differently) {
            (Some(original), Some(spelled_differently)) => {
                return Some(original == spelled_differently);
            }
            // The differently spelled name does not resolve, so this
            // directory tells the two names apart.
            (Some(_), None) => return Some(false),
            (None, _) => continue,
        }
    }
    None
}

/// The same name with the case of its first ASCII letter flipped, if it has
/// one.
fn case_variant(name: &str) -> Option<String> {
    let mut variant = String::with_capacity(name.len());
    let mut flipped = false;
    for character in name.chars() {
        if !flipped && character.is_ascii_alphabetic() {
            variant.push(if character.is_ascii_uppercase() {
                character.to_ascii_lowercase()
            } else {
                character.to_ascii_uppercase()
            });
            flipped = true;
        } else {
            variant.push(character);
        }
    }
    flipped.then_some(variant)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedCodexAccountsEnvelope {
    version: u32,
    /// The raw entries: each one is parsed on its own, so one malformed
    /// account never voids its valid siblings.
    #[serde(default)]
    accounts: Vec<serde_json::Value>,
}

/// The persisted mirror of the account configuration. Read once at runtime
/// start (before the scheduler's first cycle, so the startup cycle already
/// honors the configured accounts) and rewritten on every applied change
/// from the settings lane.
pub struct CodexAccountStore {
    writable: bool,
    unsupported_version: Option<u64>,
    path: PathBuf,
    accounts: Mutex<Vec<CodexAccountConfig>>,
}

impl CodexAccountStore {
    pub fn open(path: PathBuf) -> Self {
        let unsupported_version = fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
            .and_then(|value| value.get("version").and_then(serde_json::Value::as_u64))
            .filter(|version| *version != SCHEMA_VERSION as u64);
        let store = Self {
            unsupported_version,
            writable: crate::durable_file::prepare_json(&path, &[SCHEMA_VERSION as u64], |value| {
                let normalized = serde_json::to_value(parse_envelope(&value.to_string())).ok();
                value.get("accounts") == normalized.as_ref()
            }),
            path,
            accounts: Mutex::new(default_accounts()),
        };
        store.load_from_disk();
        store
    }

    /// The effective configuration, always normalized to the known ids.
    pub fn accounts(&self) -> Vec<CodexAccountConfig> {
        self.accounts.lock().unwrap().clone()
    }

    pub fn unsupported_version(&self) -> Option<u64> {
        self.unsupported_version
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    /// Replaces the configuration and persists it. A failed write keeps the
    /// in-memory state (this session gates on it) and surfaces the error, so
    /// a restart falls back to the defaults rather than to a stale file
    /// silently contradicting the UI.
    ///
    /// A collision between the two accounts' Codex homes is rejected before
    /// anything is locked or written, so neither memory nor disk changes.
    pub fn set_accounts(&self, accounts: &[CodexAccountConfig]) -> Result<(), io::Error> {
        let ambient_codex_home = std::env::var("CODEX_HOME").ok();
        let user_home = std::env::home_dir().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Could not locate the user home directory",
            )
        })?;
        let current_dir = std::env::current_dir()?;
        self.set_accounts_in(accounts, ambient_codex_home.as_deref(), &user_home, &current_dir)
    }

    /// The write path with the environment supplied by the caller, so tests
    /// exercise the same validation and persistence without reading the
    /// process home directory, `CODEX_HOME`, or the real working directory.
    fn set_accounts_in(
        &self,
        accounts: &[CodexAccountConfig],
        ambient_codex_home: Option<&str>,
        user_home: &Path,
        current_dir: &Path,
    ) -> Result<(), io::Error> {
        if !self.writable { return Err(io::Error::other("Unsupported or unavailable account store")); }
        let normalized = normalize_accounts(accounts);
        validate_codex_home_collisions_in(&normalized, ambient_codex_home, user_home, current_dir)?;
        let mut state = self.accounts.lock().unwrap();
        let envelope = PersistedCodexAccountsEnvelope {
            version: SCHEMA_VERSION,
            accounts: serde_json::to_value(&normalized)?.as_array().cloned().unwrap_or_default(),
        };
        let json = serde_json::to_vec(&envelope)?;
        if fs::read(&self.path).ok().as_deref() != Some(json.as_slice()) {
            crate::durable_file::replace(&self.path, &json)?;
        }
        *state = normalized;
        Ok(())
    }

    fn load_from_disk(&self) {
        let Ok(raw) = fs::read_to_string(&self.path) else {
            return;
        };
        let accounts = parse_envelope(&raw);
        *self.accounts.lock().unwrap() = accounts;
    }
}

/// Tolerant parse: a corrupt or foreign-version file yields the defaults —
/// the fail-open direction, which is exactly the pre-multi-account state —
/// and one malformed entry drops instead of voiding its valid siblings.
fn parse_envelope(raw: &str) -> Vec<CodexAccountConfig> {
    let parsed: Result<PersistedCodexAccountsEnvelope, _> = serde_json::from_str(raw);
    let envelope = match parsed {
        Ok(envelope) if envelope.version == SCHEMA_VERSION => envelope,
        _ => return default_accounts(),
    };
    let accounts: Vec<CodexAccountConfig> = envelope
        .accounts
        .into_iter()
        .filter_map(|entry| parse_account(&entry))
        .collect();
    normalize_accounts(&accounts)
}

/// One persisted account, parsed on its own.
///
/// A wrong field type falls back to that field's default instead of
/// dropping the account: the alias is display only and the monitor flag
/// defaults to on, so a malformed value degrades one field, never the
/// account and never its sibling.
fn parse_account(value: &serde_json::Value) -> Option<CodexAccountConfig> {
    let account_id = value
        .get("accountId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    if !KNOWN_ACCOUNT_IDS.contains(&account_id) {
        return None;
    }
    Some(CodexAccountConfig {
        account_id: account_id.to_string(),
        alias: value
            .get("alias")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        codex_home: value
            .get("codexHome")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        monitored: value
            .get("monitored")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_path(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "limitscope-codex-accounts-{tag}-{}-{n}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        path
    }

    fn remove_store(path: &PathBuf) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("json.tmp"));
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let path = store_path(tag).with_extension("dir");
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// A hermetic environment for the write path: no test consults the
    /// process home directory, the ambient `CODEX_HOME`, or the real working
    /// directory, and nothing here points at a production path.
    fn test_context(root: &Path) -> (PathBuf, PathBuf) {
        (root.join("user"), root.to_path_buf())
    }

    /// Applies accounts through the production validation and persistence path
    /// with that hermetic context.
    fn apply(
        store: &CodexAccountStore,
        accounts: &[CodexAccountConfig],
        root: &Path,
    ) -> io::Result<()> {
        let (user_home, current_dir) = test_context(root);
        store.set_accounts_in(accounts, None, &user_home, &current_dir)
    }

    #[test]
    fn applying_defaults_after_semantic_corruption_really_repairs_disk() {
        let path = store_path("defaults-after-corruption");
        let root = temp_dir("defaults-after-corruption-context");
        fs::write(&path, r#"{"version":1,"accounts":"broken"}"#).unwrap();
        let store = CodexAccountStore::open(path.clone());
        apply(&store, &store.accounts(), &root).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(value["accounts"].is_array());
        // Remove only this fixture's preserved recovery copies.
        for entry in fs::read_dir(path.parent().unwrap()).unwrap().filter_map(Result::ok) {
            if entry.file_name().to_string_lossy().starts_with(&format!("{}.recovery-", path.file_stem().unwrap().to_string_lossy())) {
                fs::remove_file(entry.path()).unwrap();
            }
        }
        remove_store(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_write_does_not_change_memory_and_identical_retry_succeeds() {
        let root = temp_dir("failed-retry-context");
        let path = store_path("failed-retry");
        let store = CodexAccountStore::open(path.clone());
        let before = store.accounts();
        let mut accounts = before.clone();
        accounts[0].alias = "Retry".into();
        fs::create_dir(&path).unwrap();
        assert!(apply(&store, &accounts, &root).is_err());
        assert_eq!(store.accounts(), before);
        fs::remove_dir(&path).unwrap();
        apply(&store, &accounts, &root).unwrap();
        assert_eq!(CodexAccountStore::open(path.clone()).accounts(), accounts);
        remove_store(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_file_starts_with_the_default_accounts() {
        let path = store_path("missing");
        let store = CodexAccountStore::open(path.clone());
        let accounts = store.accounts();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].account_id, PRIMARY_ACCOUNT_ID);
        assert_eq!(accounts[1].account_id, SECONDARY_ACCOUNT_ID);
        assert!(accounts.iter().all(|account| account.monitored));
        remove_store(&path);
    }

    #[test]
    fn startup_reads_preserve_main_cowork_and_explicit_edits_still_persist() {
        let root = temp_dir("startup-read-context");
        let path = store_path("startup-read");
        let mut accounts = default_accounts();
        accounts[0].alias = "Main".to_string();
        accounts[0].codex_home = "C:\\fixture\\main".to_string();
        accounts[1].alias = "Cowork".to_string();
        accounts[1].codex_home = "C:\\fixture\\cowork".to_string();
        accounts[1].monitored = false;
        apply(&CodexAccountStore::open(path.clone()), &accounts, &root)
            .expect("persist fixture");
        let before = fs::read(&path).expect("read fixture");
        let modified = fs::metadata(&path).unwrap().modified().unwrap();

        let startup = CodexAccountStore::open(path.clone());
        assert_eq!(startup.accounts(), accounts);
        assert_eq!(startup.accounts(), accounts, "reopening Settings only reads");
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);

        let mut edited = startup.accounts();
        edited[0].alias = "Main edited".to_string();
        apply(&startup, &edited, &root).expect("explicit edit");
        assert_eq!(CodexAccountStore::open(path.clone()).accounts(), edited);
        assert_eq!(edited[1], accounts[1], "the other account stays isolated");
        remove_store(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn applied_accounts_round_trip_through_the_file() {
        let root = temp_dir("roundtrip-context");
        let path = store_path("roundtrip");
        let store = CodexAccountStore::open(path.clone());
        let mut accounts = default_accounts();
        accounts[1].alias = "Work".to_string();
        accounts[1].codex_home = "C:\\\\codex-home-second".to_string();
        accounts[1].monitored = false;
        apply(&store, &accounts, &root).expect("persist");

        let reopened = CodexAccountStore::open(path.clone());
        assert_eq!(reopened.accounts(), accounts);
        remove_store(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_corrupt_file_degrades_to_the_defaults() {
        let path = store_path("corrupt");
        fs::write(&path, "{ not json").expect("write fixture");
        let store = CodexAccountStore::open(path.clone());
        assert_eq!(store.accounts(), default_accounts());
        remove_store(&path);
    }

    #[test]
    fn a_foreign_version_file_degrades_to_the_defaults() {
        let root = temp_dir("version-context");
        let path = store_path("version");
        fs::write(&path, r#"{ "version": 99, "accounts": [] }"#).expect("write fixture");
        let store = CodexAccountStore::open(path.clone());
        assert_eq!(store.accounts(), default_accounts());
        assert_eq!(store.unsupported_version(), Some(99));
        assert!(apply(&store, &default_accounts(), &root).is_err());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{ "version": 99, "accounts": [] }"#
        );
        remove_store(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_or_invalid_version_is_not_writable() {
        let root = temp_dir("invalid-version-context");
        for raw in [r#"{"accounts":[]}"#, r#"{"version":"1","accounts":[]}"#] {
            let path = store_path("invalid-version");
            fs::write(&path, raw).unwrap();
            let store = CodexAccountStore::open(path.clone());
            assert!(!store.is_writable());
            assert!(apply(&store, &default_accounts(), &root).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), raw);
            remove_store(&path);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unknown_account_id_is_dropped_and_a_valid_sibling_survives() {
        let path = store_path("unknown");
        fs::write(
            &path,
            r#"{
              "version": 1,
              "accounts": [
                { "accountId": "openai-codex", "alias": "Personal", "codexHome": "", "monitored": true },
                { "accountId": "openai-codex:rogue", "alias": "Nope", "codexHome": "C:\\x", "monitored": true }
              ]
            }"#,
        )
        .expect("write fixture");
        let store = CodexAccountStore::open(path.clone());
        let accounts = store.accounts();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].alias, "Personal");
        assert_eq!(accounts[1].alias, "");
        remove_store(&path);
    }

    #[test]
    fn a_malformed_entry_drops_instead_of_voiding_its_sibling() {
        let path = store_path("malformed");
        fs::write(
            &path,
            r#"{
              "version": 1,
              "accounts": [
                { "accountId": "openai-codex", "alias": 7, "monitored": "yes" },
                { "accountId": "openai-codex:secondary", "alias": "Work", "codexHome": "C:\\h", "monitored": false }
              ]
            }"#,
        )
        .expect("write fixture");
        let store = CodexAccountStore::open(path.clone());
        let accounts = store.accounts();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].alias, "");
        assert!(accounts[0].monitored, "a non-boolean flag falls back to the default");
        assert_eq!(accounts[1].alias, "Work");
        assert!(!accounts[1].monitored);
        remove_store(&path);
    }

    #[test]
    fn only_the_primary_account_has_a_default_codex_home() {
        let accounts = default_accounts();
        assert!(accounts[0].is_configured(), "the primary account always resolves");
        assert!(
            !accounts[1].is_configured(),
            "the second account needs its own Codex home"
        );
        let mut secondary = accounts[1].clone();
        secondary.codex_home = "C:\\\\codex-home-second".to_string();
        assert!(secondary.is_configured());
        assert_eq!(secondary.codex_home_option(), Some("C:\\\\codex-home-second"));
        assert_eq!(accounts[0].codex_home_option(), None);
    }

    #[test]
    fn normalization_keeps_the_registry_order_and_fills_missing_accounts() {
        let mut accounts = default_accounts();
        accounts.swap(0, 1);
        accounts[1].alias = "Second".to_string();
        let normalized = normalize_accounts(&accounts);
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].account_id, PRIMARY_ACCOUNT_ID);
        assert_eq!(normalized[1].account_id, SECONDARY_ACCOUNT_ID);
        // The alias travelled with its account, not with its position.
        assert_eq!(normalized[0].alias, "Second");
        assert_eq!(normalized[1].alias, "");
    }

    #[test]
    fn dot_and_parent_segments_cannot_bypass_codex_home_collision_check() {
        let root = temp_dir("home-dotdot");
        let shared = root.join("homes");
        fs::create_dir_all(shared.join("nested")).unwrap();
        let mut accounts = default_accounts();
        accounts[0].codex_home = shared.join("nested").join("..").to_string_lossy().into();
        accounts[1].codex_home = shared.to_string_lossy().into();

        let error = validate_codex_home_collisions_in(
            &accounts,
            None,
            &root.join("user"),
            &root,
        )
        .expect_err("equivalent normalized homes must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_separator_and_trailing_dot_aliases_collide() {
        let root = temp_dir("home-windows-alias");
        let shared = root.join("Shared");
        fs::create_dir_all(&shared).unwrap();
        let mut accounts = default_accounts();
        accounts[0].codex_home = shared.to_string_lossy().into();
        let alias = shared.to_string_lossy().replace('\\', "/").to_uppercase();
        accounts[1].codex_home = format!("{alias}.");

        let error = validate_codex_home_collisions_in(
            &accounts,
            None,
            &root.join("user"),
            &root,
        )
        .expect_err("Windows path aliases must be treated as one home");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_sensitive_directory_keeps_distinct_codex_homes() {
        let root = temp_dir("home-windows-case-sensitive");
        let status = std::process::Command::new("fsutil")
            .args(["file", "setCaseSensitiveInfo"])
            .arg(&root)
            .arg("enable")
            .status()
            .expect("fsutil must be available to enable directory case sensitivity");
        if !status.success() {
            let _ = fs::remove_dir_all(root);
            return;
        }

        let upper = root.join("Shared");
        let lower = root.join("shared");
        fs::create_dir(&upper).unwrap();
        fs::create_dir(&lower).unwrap();
        assert_ne!(
            fs::canonicalize(&upper).unwrap(),
            fs::canonicalize(&lower).unwrap(),
            "the fixture must contain two case-distinct directories"
        );

        let mut accounts = default_accounts();
        accounts[0].codex_home = upper.to_string_lossy().into();
        accounts[1].codex_home = lower.to_string_lossy().into();
        validate_codex_home_collisions_in(&accounts, None, &root.join("user"), &root)
            .expect("case-distinct existing homes are different directories");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn case_variant_flips_the_first_ascii_letter_in_either_direction() {
        assert_eq!(case_variant("shared"), Some("Shared".to_string()));
        assert_eq!(case_variant("Shared"), Some("shared".to_string()));
        assert_eq!(case_variant("123"), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn codex_home_paths_remain_case_sensitive_off_windows() {
        let root = temp_dir("home-case-sensitive");
        let mut accounts = default_accounts();
        accounts[0].codex_home = root.join("Shared").to_string_lossy().into();
        accounts[1].codex_home = root.join("shared").to_string_lossy().into();

        validate_codex_home_collisions_in(
            &accounts,
            None,
            &root.join("user"),
            &root,
        )
        .expect("case-distinct homes are distinct on this platform");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn primary_default_home_cannot_be_reused_by_secondary() {
        let root = temp_dir("home-default");
        let user_home = root.join("user");
        let fallback = user_home.join(".codex");
        fs::create_dir_all(fallback.join("nested")).unwrap();
        let current_dir = root.as_path();
        let mut accounts = default_accounts();
        accounts[1].codex_home = fallback.join("nested").join("..").to_string_lossy().into();

        let default_collision =
            validate_codex_home_collisions_in(&accounts, None, &user_home, current_dir);
        assert!(default_collision.is_err());

        let ambient = root.join("ambient");
        fs::create_dir_all(ambient.join("nested")).unwrap();
        accounts[1].codex_home = ambient.join("nested").join("..").to_string_lossy().into();
        let ambient_collision = validate_codex_home_collisions_in(
            &accounts,
            Some(ambient.to_string_lossy().as_ref()),
            &user_home,
            current_dir,
        );
        assert!(ambient_collision.is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn colliding_codex_homes_are_rejected_before_persisting() {
        let root = temp_dir("home-store-collision");
        let shared = root.join("shared");
        let path = store_path("home-store-collision");
        let store = CodexAccountStore::open(path.clone());
        let before = store.accounts();
        let mut accounts = before.clone();
        accounts[0].codex_home = shared.to_string_lossy().into();
        accounts[1].codex_home = shared.join(".").to_string_lossy().into();

        let error = apply(&store, &accounts, &root)
            .expect_err("the store must reject a credential-home collision");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(store.accounts(), before);
        assert!(!path.exists(), "validation happens before persistence");

        remove_store(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn distinct_codex_homes_persist_without_rewriting_the_paths() {
        let root = temp_dir("home-store-distinct");
        let path = store_path("home-store-distinct");
        let store = CodexAccountStore::open(path.clone());
        let mut accounts = default_accounts();
        accounts[0].codex_home = root.join("main").to_string_lossy().into();
        accounts[1].codex_home = root.join("cowork").to_string_lossy().into();

        apply(&store, &accounts, &root)
            .expect("distinct homes are valid");
        assert_eq!(store.accounts(), accounts);
        assert_eq!(CodexAccountStore::open(path.clone()).accounts(), accounts);

        remove_store(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn applying_the_same_accounts_is_a_no_op_and_rewrites_nothing() {
        let root = temp_dir("idempotent-context");
        let path = store_path("idempotent");
        let store = CodexAccountStore::open(path.clone());
        let mut accounts = default_accounts();
        accounts[1].codex_home = "C:\\\\codex-home-second".to_string();
        apply(&store, &accounts, &root).expect("first apply");
        let written = fs::metadata(&path).expect("file exists");

        std::thread::sleep(std::time::Duration::from_millis(20));
        apply(&store, &accounts, &root).expect("second apply");
        let metadata = fs::metadata(&path).expect("file exists");
        assert_eq!(
            written.modified().unwrap(),
            metadata.modified().unwrap(),
            "an unchanged configuration must not rewrite the store"
        );
        remove_store(&path);
        let _ = fs::remove_dir_all(root);
    }
}
