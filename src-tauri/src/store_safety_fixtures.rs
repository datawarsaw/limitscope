//! Synthetic cross-store fixtures for the 0.8.25 corruption/quarantine
//! parity review on the 0.8.24 data-safety base. Every fixture runs in its
//! own fresh directory under the test process temp dir; no real user-data
//! file is ever read or written.
//!
//! Each RC (0.8.25) quarantine goal is pinned against the 0.8.24 recovery
//! semantics the stores already use (`<stem>.recovery-<uuid>.json` sibling
//! copies, unknown schemas write-blocked in place):
//! - corrupt / non-UTF8 / partially invalid bytes keep an exact copy before
//!   any replacement; unknown (future) schemas are never rewritten;
//! - an existing recovery copy is never overwritten;
//! - a failed (interrupted) persistence leaves the last valid file intact;
//! - file names and schema versions stay backward compatible (G6).
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};

use crate::history::{QuotaHistoryStore, HISTORY_FILE_NAME};
use crate::last_good::{ProviderLastGoodStore, LAST_GOOD_FILE_NAME};
use crate::runtime::{
    AccountAttributionDto, ProviderHealth, ProviderKind, ProviderUsageDto, UsageLimitDto,
};
use crate::usage_intelligence::{UsageIntelligenceStore, USAGE_INTELLIGENCE_FILE_NAME};
use crate::wallet_prefs::{WalletPrefsStore, WALLET_PREFS_FILE_NAME};

const NOW_RFC3339: &str = "2026-09-29T12:00:00Z";
const OBSERVED_AT: &str = "2026-09-29T11:55:00.000Z";
const NON_UTF8: &[u8] = &[0xFF, 0xFE, b'{', 0x00, 0xC3, 0x28, 0xFF];

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn dir(tag: &str) -> Dir {
    static N: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "limitscope-store-fixture-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    Dir(path)
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(NOW_RFC3339).unwrap().with_timezone(&Utc)
}
fn now_ms() -> i64 {
    now().timestamp_millis()
}

/// Every sibling `<stem>.recovery-*.json` copy of `path`, as bytes.
fn recovery_copies(path: &Path) -> Vec<Vec<u8>> {
    let prefix = format!("{}.recovery-", path.file_stem().unwrap().to_string_lossy());
    let mut copies: Vec<Vec<u8>> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.starts_with(&prefix) && name.ends_with(".json")
        })
        .map(|entry| fs::read(entry.path()).unwrap())
        .collect();
    copies.sort();
    copies
}

/// Holds the file open without FILE_SHARE_DELETE so a rename onto it fails:
/// a deterministic, user-independent "interrupted persistence" on Windows.
#[cfg(windows)]
fn lock(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new().read(true).share_mode(0).open(path).unwrap()
}

/// A stray, truncated temp sibling as left by a crash between write and rename.
fn plant_crashed_temps(path: &Path) {
    let parent = path.parent().unwrap();
    fs::write(parent.join(".limitscope-crashed.tmp"), b"{\"version\":1,\"trunc").unwrap();
    fs::write(path.with_extension("json.tmp"), b"{\"version\":1,\"trunc").unwrap();
}

fn zai_usage(percent: f64) -> ProviderUsageDto {
    ProviderUsageDto {
        id: ProviderKind::Zai.id().to_string(),
        name: ProviderKind::Zai.name().to_string(),
        health: ProviderHealth::Live,
        status: ProviderHealth::Live.legacy_status(),
        checked_at: OBSERVED_AT.to_string(),
        limits: vec![UsageLimitDto {
            label: "5-hour limit".to_string(),
            used_percent: percent,
            reset_text: None,
            reset_at: Some("2026-09-29T16:00:00Z".to_string()),
        }],
        account: Some(AccountAttributionDto {
            label: "Account A".to_string(),
            note: None,
            identity: Some("key:1234".to_string()),
        }),
        plan_type: None,
        error: None,
        error_category: None,
        error_http_status: None,
        source_updated_at: None,
        data_freshness: None,
        reset_credits: None,
        zcode_reset_cards: None,
        zcode_plans: None,
        grok_bot: None,
        fallback_failure: None,
        account_alias: None,
        monitoring_paused: None,
    }
}

fn last_good(path: &Path) -> ProviderLastGoodStore {
    ProviderLastGoodStore::open_with_clock(path.to_path_buf(), Box::new(now))
}
fn usage(path: &Path) -> UsageIntelligenceStore {
    UsageIntelligenceStore::open_with(path.to_path_buf(), Box::new(now_ms), Box::new(|| None))
}
fn history(path: &Path) -> QuotaHistoryStore {
    QuotaHistoryStore::open_with_clock(path.to_path_buf(), Box::new(now_ms))
}
fn history_blob(version: u64) -> String {
    serde_json::json!({
        "version": version,
        "observations": [{ "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": OBSERVED_AT }]
    })
    .to_string()
}

// ---------------- wallet_prefs ----------------

#[test]
fn wallet_corrupt_non_utf8_and_dropped_entries_keep_exact_copies_before_rewrite() {
    let fixtures: [&[u8]; 3] = [
        b"{ not json",
        NON_UTF8,
        br#"{"version":1,"monitored":{"openrouter":false,"deepseek":"yes"}}"#,
    ];
    for raw in fixtures {
        let d = dir("wallet-degraded");
        let path = d.0.join(WALLET_PREFS_FILE_NAME);
        fs::write(&path, raw).unwrap();
        let store = WalletPrefsStore::open(path.clone());
        assert_eq!(fs::read(&path).unwrap(), raw, "open never rewrites degraded bytes");
        assert_eq!(recovery_copies(&path), vec![raw.to_vec()], "exactly one exact copy");
        let flags = std::collections::HashMap::from([("openrouter".to_string(), true)]);
        store.set_monitored(&flags).unwrap();
        assert_eq!(WalletPrefsStore::open(path.clone()).monitored(), flags);
        assert!(recovery_copies(&path).contains(&raw.to_vec()), "copy survives the rewrite");
    }
}

#[test]
fn wallet_future_schema_bytes_survive_and_writes_fail_closed() {
    let d = dir("wallet-future");
    let path = d.0.join(WALLET_PREFS_FILE_NAME);
    let future = br#"{"version":2,"monitored":{"openrouter":false},"newField":1}"#;
    fs::write(&path, future).unwrap();
    let store = WalletPrefsStore::open(path.clone());
    assert!(store.monitored().is_empty());
    let flags = std::collections::HashMap::from([("openrouter".to_string(), true)]);
    assert!(store.set_monitored(&flags).is_err(), "unknown schema is write-blocked");
    assert!(store.monitored().is_empty(), "failed write leaves memory unchanged");
    assert_eq!(fs::read(&path).unwrap(), future, "unknown schema never converted to defaults");
    assert!(recovery_copies(&path).is_empty(), "blocked in place, no copy needed");
}

#[test]
fn wallet_existing_recovery_copy_is_never_overwritten() {
    let d = dir("wallet-existing-copy");
    let path = d.0.join(WALLET_PREFS_FILE_NAME);
    let older = d.0.join("wallet-preferences-v1.recovery-older.json");
    fs::write(&older, b"older preserved bytes").unwrap();
    fs::write(&path, b"{ newer corruption").unwrap();
    let store = WalletPrefsStore::open(path.clone());
    store.set_monitored(&std::collections::HashMap::from([("deepseek".to_string(), false)])).unwrap();
    assert_eq!(fs::read(&older).unwrap(), b"older preserved bytes");
    assert!(recovery_copies(&path).contains(&b"{ newer corruption".to_vec()));
}

#[test]
fn wallet_valid_state_survives_interrupted_persistence() {
    let d = dir("wallet-interrupted");
    let path = d.0.join(WALLET_PREFS_FILE_NAME);
    let valid = std::collections::HashMap::from([("openrouter".to_string(), false)]);
    WalletPrefsStore::open(path.clone()).set_monitored(&valid).unwrap();
    let bytes = fs::read(&path).unwrap();
    plant_crashed_temps(&path);
    let store = WalletPrefsStore::open(path.clone());
    assert_eq!(store.monitored(), valid, "crashed temp siblings are never loaded");
    #[cfg(windows)]
    {
        let held = lock(&path);
        let next = std::collections::HashMap::from([("openrouter".to_string(), true)]);
        assert!(store.set_monitored(&next).is_err());
        drop(held);
        assert_eq!(store.monitored(), valid, "memory matches the surviving disk state");
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(WalletPrefsStore::open(path.clone()).monitored(), valid);
    assert!(recovery_copies(&path).is_empty(), "healthy files never get copies");
}

// ---------------- last_good (existing .json.tmp write path untouched) ----------------

#[test]
fn last_good_corrupt_and_non_utf8_keep_exact_copies() {
    for raw in [b"{not valid json".as_slice(), NON_UTF8] {
        let d = dir("last-good-degraded");
        let path = d.0.join(LAST_GOOD_FILE_NAME);
        fs::write(&path, raw).unwrap();
        let store = last_good(&path);
        assert!(store.get_persisted("zai").is_none());
        assert_eq!(recovery_copies(&path), vec![raw.to_vec()]);
        store.record_success(ProviderKind::Zai, &zai_usage(40.0));
        assert!(last_good(&path).get_persisted("zai").is_some(), "healed after exact copy");
        assert_eq!(recovery_copies(&path), vec![raw.to_vec()]);
    }
}

#[test]
fn last_good_future_schema_is_write_and_clear_blocked() {
    let d = dir("last-good-future");
    let path = d.0.join(LAST_GOOD_FILE_NAME);
    let future = br#"{"version":2,"providers":[],"wallets":[]}"#;
    fs::write(&path, future).unwrap();
    let store = last_good(&path);
    store.record_success(ProviderKind::Zai, &zai_usage(40.0));
    assert!(store.persist().is_err());
    assert!(store.clear().is_err());
    assert_eq!(fs::read(&path).unwrap(), future);
}

#[test]
fn last_good_valid_state_survives_interrupted_persistence() {
    let d = dir("last-good-interrupted");
    let path = d.0.join(LAST_GOOD_FILE_NAME);
    last_good(&path).record_success(ProviderKind::Zai, &zai_usage(40.0));
    let bytes = fs::read(&path).unwrap();
    plant_crashed_temps(&path);
    let store = last_good(&path);
    assert_eq!(store.get_persisted("zai").unwrap().limits[0].used_percent, 40.0);
    #[cfg(windows)]
    {
        let held = lock(&path);
        store.record_success(ProviderKind::Zai, &zai_usage(90.0));
        assert!(store.persist().is_err());
        drop(held);
    }
    assert_eq!(fs::read(&path).unwrap(), bytes, "last valid envelope intact");
    assert_eq!(last_good(&path).get_persisted("zai").unwrap().limits[0].used_percent, 40.0);
}

// ---------------- usage_intelligence ----------------

#[test]
fn usage_degraded_store_reopened_repeatedly_keeps_one_exact_copy() {
    let partial = serde_json::json!({
        "version": 1, "enabled": true, "events": [{ "id": "" }], "cursors": []
    })
    .to_string()
    .into_bytes();
    for raw in [b"{not json at all".to_vec(), partial] {
        let d = dir("usage-reopen");
        let path = d.0.join(USAGE_INTELLIGENCE_FILE_NAME);
        fs::write(&path, &raw).unwrap();
        for _ in 0..3 {
            let _ = usage(&path);
        }
        assert_eq!(fs::read(&path).unwrap(), raw, "loads never rewrite the original");
        assert_eq!(recovery_copies(&path), vec![raw.clone()], "restarts reuse the exact copy");
        // A later explicit write replaces the file only after that copy exists.
        usage(&path).set_enabled(true).unwrap();
        assert!(usage(&path).enabled());
        assert_eq!(recovery_copies(&path), vec![raw]);
    }
}

#[test]
fn usage_existing_different_recovery_copy_is_never_overwritten() {
    let d = dir("usage-existing-copy");
    let path = d.0.join(USAGE_INTELLIGENCE_FILE_NAME);
    let older = d.0.join("usage-intelligence-v1.recovery-older.json");
    fs::write(&older, b"older preserved bytes").unwrap();
    fs::write(&path, b"{ newer corruption").unwrap();
    usage(&path).set_enabled(true).unwrap();
    assert_eq!(fs::read(&older).unwrap(), b"older preserved bytes");
    assert_eq!(
        recovery_copies(&path),
        vec![b"older preserved bytes".to_vec(), b"{ newer corruption".to_vec()]
    );
}

#[test]
fn usage_non_utf8_is_preserved_in_place_and_write_blocked() {
    let d = dir("usage-non-utf8");
    let path = d.0.join(USAGE_INTELLIGENCE_FILE_NAME);
    fs::write(&path, NON_UTF8).unwrap();
    let store = usage(&path);
    assert!(!store.enabled(), "opt-in fails closed");
    assert!(store.set_enabled(true).is_err());
    assert!(store.clear_owned().is_err());
    assert_eq!(fs::read(&path).unwrap(), NON_UTF8, "bytes never replaced");
}

#[test]
fn usage_future_schema_is_write_blocked_in_place() {
    let d = dir("usage-future");
    let path = d.0.join(USAGE_INTELLIGENCE_FILE_NAME);
    let future = br#"{"version":2,"enabled":true,"events":[],"cursors":[]}"#;
    fs::write(&path, future).unwrap();
    let store = usage(&path);
    assert!(!store.enabled());
    assert!(store.set_enabled(true).is_err());
    assert_eq!(fs::read(&path).unwrap(), future);
    assert!(recovery_copies(&path).is_empty());
}

#[test]
fn usage_valid_state_survives_interrupted_persistence() {
    let d = dir("usage-interrupted");
    let path = d.0.join(USAGE_INTELLIGENCE_FILE_NAME);
    usage(&path).set_enabled(true).unwrap();
    let bytes = fs::read(&path).unwrap();
    plant_crashed_temps(&path);
    let store = usage(&path);
    assert!(store.enabled());
    #[cfg(windows)]
    {
        let held = lock(&path);
        assert!(store.set_enabled(false).is_err());
        drop(held);
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(usage(&path).enabled(), "last valid opt-in survives");
}

// ---------------- history ----------------

#[test]
fn history_corrupt_and_partial_keep_exact_copies_and_heal() {
    let partial = serde_json::json!({
        "version": 2,
        "observations": [
            { "providerId": "zai", "windowLabel": "5-hour", "usedPercent": 30.0, "observedAt": OBSERVED_AT },
            { "providerId": "invalid" }
        ]
    })
    .to_string()
    .into_bytes();
    for (raw, kept) in [(b"{not json".to_vec(), 0), (NON_UTF8.to_vec(), 0), (partial, 1)] {
        let d = dir("history-degraded");
        let path = d.0.join(HISTORY_FILE_NAME);
        let older = d.0.join("quota-history-v1.recovery-older.json");
        fs::write(&older, b"older preserved bytes").unwrap();
        fs::write(&path, &raw).unwrap();
        assert_eq!(history(&path).history().len(), kept);
        assert_eq!(fs::read(&older).unwrap(), b"older preserved bytes");
        assert!(recovery_copies(&path).contains(&raw), "exact copy before self-heal");
        let _ = history(&path);
        assert_eq!(recovery_copies(&path).len(), 2, "healed file needs no further copies");
        assert_eq!(history(&path).history().len(), kept);
    }
}

#[test]
fn history_future_schema_is_write_and_clear_blocked() {
    let d = dir("history-future");
    let path = d.0.join(HISTORY_FILE_NAME);
    let future = history_blob(3);
    fs::write(&path, &future).unwrap();
    let store = history(&path);
    assert!(store.history().is_empty());
    assert!(store.clear().is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), future);
    assert!(recovery_copies(&path).is_empty());
}

#[test]
fn history_valid_state_survives_interrupted_persistence() {
    let d = dir("history-interrupted");
    let path = d.0.join(HISTORY_FILE_NAME);
    let valid = history_blob(2);
    fs::write(&path, &valid).unwrap();
    plant_crashed_temps(&path);
    let store = history(&path);
    assert_eq!(store.history().len(), 1);
    #[cfg(windows)]
    {
        let held = lock(&path);
        assert!(store.clear().is_err(), "a failed durable clear reports failure");
        drop(held);
        assert_eq!(store.history().len(), 1, "memory not cleared on failed write");
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), valid);
    assert_eq!(history(&path).history().len(), 1);
}

// ---------------- G6: rollback / backward compatibility ----------------

#[test]
fn g6_file_names_and_schema_versions_stay_0_8_24_compatible() {
    assert_eq!(WALLET_PREFS_FILE_NAME, "wallet-preferences-v1.json");
    assert_eq!(LAST_GOOD_FILE_NAME, "provider-last-good-v1.json");
    assert_eq!(USAGE_INTELLIGENCE_FILE_NAME, "usage-intelligence-v1.json");
    assert_eq!(HISTORY_FILE_NAME, "quota-history-v1.json");

    let d = dir("g6");
    let version = |name: &str| -> u64 {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(d.0.join(name)).unwrap()).unwrap();
        value["version"].as_u64().unwrap()
    };
    WalletPrefsStore::open(d.0.join(WALLET_PREFS_FILE_NAME))
        .set_monitored(&std::collections::HashMap::from([("openrouter".to_string(), true)]))
        .unwrap();
    last_good(&d.0.join(LAST_GOOD_FILE_NAME)).record_success(ProviderKind::Zai, &zai_usage(10.0));
    usage(&d.0.join(USAGE_INTELLIGENCE_FILE_NAME)).set_enabled(true).unwrap();
    // A 0.8.24-era version-1 history blob still loads and keeps its entry.
    fs::write(d.0.join(HISTORY_FILE_NAME), history_blob(1)).unwrap();
    assert_eq!(history(&d.0.join(HISTORY_FILE_NAME)).history().len(), 1);
    assert!(history(&d.0.join(HISTORY_FILE_NAME)).clear().is_ok());

    assert_eq!(version(WALLET_PREFS_FILE_NAME), 1);
    assert_eq!(version(LAST_GOOD_FILE_NAME), 1);
    assert_eq!(version(USAGE_INTELLIGENCE_FILE_NAME), 1);
    assert_eq!(version(HISTORY_FILE_NAME), 2, "the 0.8.24 base already writes history v2");
    // Recovery copies are siblings with distinct names; no store file is renamed.
    for entry in fs::read_dir(&d.0).unwrap().filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(!name.contains("quarantined"), "no RC-only sidecar naming introduced: {name}");
    }
}
