//! Persisted per-wallet monitor preferences (v0.8.17 wallet configuration).
//!
//! The runtime's wallet monitor gate must be known before the first
//! collection cycle, and that cycle starts before any webview attaches — so
//! the Rust side keeps its own persisted mirror of the per-wallet "monitor"
//! flags the settings lane owns in the WebView (pushed on attach and on
//! every change, like the notification lane's enabled flag). One bounded
//! JSON file (`wallet-preferences-v1.json`) in the app-data directory,
//! written temp-then-rename like the sibling stores. A corrupt or
//! foreign-version file degrades to an empty map: every registered wallet
//! then falls back to its spec's own default (monitored for OpenRouter and
//! DeepSeek, not monitored for the OpenAI placeholder).
//!
//! Only the monitor flag lives here. Floating-bar visibility is
//! presentation-only and stays entirely in the WebView settings lane.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Versioned file name, alongside the other app-data artifacts.
pub const WALLET_PREFS_FILE_NAME: &str = "wallet-preferences-v1.json";

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedWalletPrefsEnvelope {
    version: u32,
    /// Wallet id -> explicit monitor flag. Ids the runtime does not know are
    /// ignored at apply time; absent ids fall back to the spec's own
    /// default. Parsed as raw JSON values so one malformed entry drops on
    /// its own instead of voiding every valid sibling.
    #[serde(default)]
    monitored: HashMap<String, serde_json::Value>,
}

/// The persisted mirror of the per-wallet monitor flags. Read once at
/// runtime start (before the scheduler's first cycle) and rewritten on every
/// applied change from the settings lane.
pub struct WalletPrefsStore {
    path: PathBuf,
    monitored: Mutex<HashMap<String, bool>>,
}

impl WalletPrefsStore {
    pub fn open(path: PathBuf) -> Self {
        let store = Self {
            path,
            monitored: Mutex::new(HashMap::new()),
        };
        store.load_from_disk();
        store
    }

    /// The persisted monitor flags as loaded (or last applied).
    pub fn monitored(&self) -> HashMap<String, bool> {
        self.monitored.lock().unwrap().clone()
    }

    /// Replaces the monitor map and persists it. Only the flags the caller
    /// resolved for known wallet ids reach this point; a failed write keeps
    /// the in-memory state (this session gates on it) and surfaces the
    /// error, so a restart falls back to defaults rather than to a stale
    /// file silently contradicting the UI.
    pub fn set_monitored(&self, monitored: &HashMap<String, bool>) -> Result<(), std::io::Error> {
        {
            let mut state = self.monitored.lock().unwrap();
            if *state == *monitored {
                return Ok(());
            }
            *state = monitored.clone();
        }
        self.persist()
    }

    fn persist(&self) -> Result<(), std::io::Error> {
        let envelope = PersistedWalletPrefsEnvelope {
            version: SCHEMA_VERSION,
            monitored: self
                .monitored()
                .into_iter()
                .map(|(id, flag)| (id, serde_json::Value::Bool(flag)))
                .collect(),
        };
        let json = serde_json::to_string_pretty(&envelope)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let temp_path = self.path.with_extension("json.tmp");
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&temp_path, json.as_bytes()).and_then(|()| fs::rename(&temp_path, &self.path))
    }

    fn load_from_disk(&self) {
        let Ok(raw) = fs::read_to_string(&self.path) else {
            return;
        };
        let monitored = parse_envelope(&raw);
        *self.monitored.lock().unwrap() = monitored;
    }
}

/// Tolerant parse: a corrupt or foreign-version file yields an empty map —
/// the fail-open direction is the spec's own default per wallet — and a
/// non-boolean entry drops instead of voiding the valid siblings.
fn parse_envelope(raw: &str) -> HashMap<String, bool> {
    let parsed: Result<PersistedWalletPrefsEnvelope, _> = serde_json::from_str(raw);
    let envelope = match parsed {
        Ok(envelope) if envelope.version == SCHEMA_VERSION => envelope,
        _ => return HashMap::new(),
    };
    envelope
        .monitored
        .into_iter()
        .filter(|(id, flag)| !id.trim().is_empty() && flag.is_boolean())
        .map(|(id, flag)| {
            let flag = match flag {
                serde_json::Value::Bool(flag) => flag,
                _ => unreachable!("filtered above"),
            };
            (id, flag)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_path(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "limitscope-wallet-prefs-{tag}-{}-{n}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        path
    }

    fn remove_store(path: &std::path::Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("json.tmp"));
    }

    #[test]
    fn a_missing_file_starts_empty() {
        let path = store_path("missing");
        let store = WalletPrefsStore::open(path.clone());
        assert!(store.monitored().is_empty());
        remove_store(&path);
    }

    #[test]
    fn applied_flags_round_trip_through_the_file() {
        let path = store_path("roundtrip");
        let store = WalletPrefsStore::open(path.clone());
        let mut monitored = HashMap::new();
        monitored.insert("openrouter".to_string(), false);
        monitored.insert("deepseek".to_string(), true);
        store.set_monitored(&monitored).expect("persist");

        let reopened = WalletPrefsStore::open(path.clone());
        assert_eq!(reopened.monitored(), monitored);
        remove_store(&path);
    }

    #[test]
    fn a_corrupt_file_degrades_to_an_empty_map() {
        let path = store_path("corrupt");
        fs::write(&path, "{ not json").expect("write fixture");
        let store = WalletPrefsStore::open(path.clone());
        assert!(store.monitored().is_empty());
        remove_store(&path);
    }

    #[test]
    fn a_foreign_version_file_degrades_to_an_empty_map() {
        let path = store_path("version");
        fs::write(
            &path,
            r#"{ "version": 99, "monitored": { "openrouter": false } }"#,
        )
        .expect("write fixture");
        let store = WalletPrefsStore::open(path.clone());
        assert!(store.monitored().is_empty());
        remove_store(&path);
    }

    #[test]
    fn non_boolean_entries_drop_instead_of_poisoning_the_map() {
        let path = store_path("entries");
        fs::write(
            &path,
            r#"{ "version": 1, "monitored": { "openrouter": false, "deepseek": "yes" } }"#,
        )
        .expect("write fixture");
        let store = WalletPrefsStore::open(path.clone());
        let monitored = store.monitored();
        assert_eq!(monitored.get("openrouter"), Some(&false));
        assert!(!monitored.contains_key("deepseek"));
        remove_store(&path);
    }

    #[test]
    fn applying_the_same_map_is_a_no_op_and_rewrites_nothing() {
        let path = store_path("idempotent");
        let store = WalletPrefsStore::open(path.clone());
        let mut monitored = HashMap::new();
        monitored.insert("openrouter".to_string(), false);
        store.set_monitored(&monitored).expect("first apply");
        let written = fs::metadata(&path).expect("file exists");

        std::thread::sleep(std::time::Duration::from_millis(20));
        store.set_monitored(&monitored).expect("second apply");
        let metadata = fs::metadata(&path).expect("file exists");
        assert_eq!(
            written.modified().unwrap(),
            metadata.modified().unwrap(),
            "an unchanged map must not rewrite the store"
        );
        remove_store(&path);
    }
}
