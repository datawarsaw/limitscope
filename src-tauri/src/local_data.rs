//! LimitScope-owned local data: the explicit v0.7 clear boundary.
//!
//! # What is owned and clearable
//!
//! Two Rust-managed local data classes are clearable from the product surface
//! on this lineage. Usage history keeps its own canonical command; the
//! provider cache gets the one added here.
//!
//! | Class | Owner | Store | Clear path |
//! |---|---|---|---|
//! | Usage history | LimitScope (Rust) | `<app-data>/quota-history-v1.json` | `history::clear_history` -> `QuotaHistoryStore::clear` |
//! | Provider cache | LimitScope (Rust) | `<app-data>/provider-last-good-v1.json` | `clear_provider_cache` (this module) |
//! | Core + floating preferences | LimitScope (WebView) | `rate-limits.settings.v1`, `rate-limits.floating-quota.v1` | `src/lib/localData.ts` via the settings hook |
//! | Execution runs | LimitScope (WebView) | `limitscope.execution-runs.v1` | `src/lib/localData.ts` via the execution-runs store (active run only behind deliberate confirmation) |
//! | Launch-at-startup entry | OS integration | HKCU Run key, via the autostart plugin | the settings hook disables LimitScope's own entry |
//! | Notification state | LimitScope (Rust) | `<app-data>/quota-notifications-v1.json` | not a user-facing category: the enabled flag mirrors the preference reset; the bounded dedup latch stays |
//!
//! The manual provenance CLI's context file (`.limitscope-provenance-run.json`)
//! is workspace-dependent, CLI-managed state. No app-owned clear reads,
//! scans for, or deletes it; that boundary is documented in
//! `docs/local-data-controls-v0.7.md`.
//!
//! # Hard safety boundary
//!
//! LimitScope never deletes or mutates external authentication material —
//! `~/.codex/auth.json`, OpenCode `auth.json`, the OpenCodex/ZCode/Grok/xAI
//! credential stores, Google/Antigravity credential material, browser cookies,
//! Windows credential stores, or any other application's files. Nothing in
//! this module resolves a path outside the app-data directory, and every
//! command here takes no arguments at all: a webview cannot name a target, so
//! no `delete_file(path)`-shaped IPC command exists to abuse.
//!
//! # Idempotency
//!
//! Every clear is idempotent. A missing file, an empty store, or a corrupt
//! file all succeed; `removed: false` means "there was nothing to remove",
//! never an error.

use std::sync::Arc;

use serde::Serialize;

use crate::last_good::ProviderLastGoodStore;
use crate::runtime::RuntimeHandle;

/// Fixed wire name of the provider cache category.
pub const PROVIDER_CACHE_CATEGORY: &str = "providerCache";

/// Result of a local-data clear: the fixed category it ran against, and
/// whether that store actually held something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDataClearOutcome {
    pub category: &'static str,
    /// `false` is success: the operation is idempotent and "nothing to
    /// remove" is never reported as a failure.
    pub removed: bool,
}

/// Clears the persisted provider last-good cache.
///
/// After this returns, a cold start hydrates no retained quota; the next
/// successful live refresh repopulates the cache normally. Quota history,
/// preferences, and every credential store are untouched — this command
/// takes no arguments and acts only on the store the runtime already owns.
#[tauri::command]
pub fn clear_provider_cache(handle: tauri::State<RuntimeHandle>) -> LocalDataClearOutcome {
    // Lane B: the cache clear also drops held suspicious-drop candidates so
    // no gated window survives a data reset (see runtime::RuntimeCore).
    handle.clear_pending_confirmations();
    clear_provider_cache_in(handle.last_good_store())
}

/// The command body without the Tauri state, so the clear is testable
/// against a real store on a temp path.
fn clear_provider_cache_in(store: Option<&Arc<ProviderLastGoodStore>>) -> LocalDataClearOutcome {
    LocalDataClearOutcome {
        category: PROVIDER_CACHE_CATEGORY,
        removed: store.map(|store| store.clear()).unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::last_good::LAST_GOOD_FILE_NAME;
    use crate::runtime::{
        AccountAttributionDto, ProviderFailure, ProviderHealth, ProviderKind, ProviderSpec,
        ProviderUsageDto, RuntimeCore, UsageLimitDto,
    };
    use chrono::{DateTime, Utc};
    use std::fs;
    use std::path::PathBuf;

    mod tempdir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn path(&self) -> &PathBuf {
                &self.path
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        pub fn temp_dir(tag: &str) -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "rate-limits-local-data-test-{}-{}-{}",
                tag,
                std::process::id(),
                n
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-29T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn sample_usage(kind: ProviderKind, percent: f64, reset_at: Option<&str>) -> ProviderUsageDto {
        ProviderUsageDto {
            id: kind.id().to_string(),
            name: kind.name().to_string(),
            health: ProviderHealth::Live,
            status: ProviderHealth::Live.legacy_status(),
            checked_at: "2026-09-29T11:55:00.000Z".to_string(),
            limits: vec![UsageLimitDto {
                label: "5-hour limit".to_string(),
                used_percent: percent,
                reset_at: reset_at.map(str::to_string),
            }],
            account: Some(AccountAttributionDto {
                label: "Account A".to_string(),
                note: None,
                identity: Some("key:1234".to_string()),
            }),
            plan_type: None,
            reset_credits: None,
            zcode_reset_cards: None,
            zcode_plans: None,
            grok_bot: None,
            error: None,
            error_category: None,
            error_http_status: None,
            source_updated_at: None,
            data_freshness: None,
            fallback_failure: None,
        }
    }

    fn mock_spec(
        kind: ProviderKind,
        outcome: Result<ProviderUsageDto, ProviderFailure>,
    ) -> ProviderSpec {
        let outcome = Arc::new(outcome);
        ProviderSpec {
            kind,
            fetch: Arc::new(move || {
                let outcome = outcome.clone();
                Box::pin(async move { (*outcome).clone() })
            }),
        }
    }

    fn store_at(path: PathBuf) -> Arc<ProviderLastGoodStore> {
        Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)))
    }

    // 1. clearing a populated cache removes memory and file, and cold start
    //    hydrates nothing afterwards
    #[test]
    fn clear_populated_cache_removes_memory_file_and_hydration() {
        let dir = tempdir::temp_dir("clear-populated");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = store_at(path.clone());
        store.record_success(
            ProviderKind::Codex,
            &sample_usage(ProviderKind::Codex, 42.0, Some("2026-09-29T15:00:00Z")),
        );
        assert!(path.exists(), "a successful record persists the cache");

        let outcome = clear_provider_cache_in(Some(&store));
        assert_eq!(outcome.category, PROVIDER_CACHE_CATEGORY);
        assert!(outcome.removed, "a populated cache reports what it removed");
        assert!(!path.exists(), "the persisted cache file is gone");
        assert!(
            store.get_persisted("openai-codex").is_none(),
            "the in-memory cache is empty"
        );

        let specs = vec![mock_spec(
            ProviderKind::Codex,
            Ok(sample_usage(ProviderKind::Codex, 42.0, None)),
        )];
        let reopened = store_at(path);
        let (hydrated, retained) = reopened.hydrate(&specs, fixed_now());
        assert!(hydrated.is_empty(), "cold start hydrates no retained quota");
        assert!(retained.is_empty(), "no retained last-good map survives");
    }

    // 1b. clearing the recovery cache leaves the current in-session runtime
    //     snapshot visible while preventing the next cold start from
    //     hydrating the removed cache.
    #[tokio::test]
    async fn clear_preserves_runtime_snapshot_and_prevents_hydration() {
        let dir = tempdir::temp_dir("clear-runtime-snapshot");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = store_at(path.clone());
        let live = sample_usage(ProviderKind::Codex, 65.0, Some("2026-09-29T15:00:00Z"));
        let specs = vec![mock_spec(ProviderKind::Codex, Ok(live.clone()))];
        let core = Arc::new(RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        )
        .with_last_good_store(Some(store.clone())));
        core.run_cycle().await;

        let before = core.snapshot().providers;
        assert_eq!(before[0].limits[0].used_percent, 65.0);

        let outcome = clear_provider_cache_in(Some(&store));
        assert!(outcome.removed);
        assert_eq!(core.snapshot().providers, before);
        assert!(!path.exists());

        let reopened = store_at(path);
        let (hydrated, retained) = reopened.hydrate(
            &[mock_spec(
                ProviderKind::Codex,
                Ok(sample_usage(ProviderKind::Codex, 10.0, None)),
            )],
            fixed_now(),
        );
        assert!(hydrated.is_empty());
        assert!(retained.is_empty());
    }

    // 2. clearing a missing cache is a success (idempotency: nothing to remove)
    #[test]
    fn clear_missing_cache_succeeds() {
        let dir = tempdir::temp_dir("clear-missing");
        let store = store_at(dir.path().join(LAST_GOOD_FILE_NAME));
        let outcome = clear_provider_cache_in(Some(&store));
        assert_eq!(outcome.category, PROVIDER_CACHE_CATEGORY);
        assert!(!outcome.removed, "nothing was there to remove");
    }

    // 3. a corrupt cache file is still removable
    #[test]
    fn clear_corrupt_cache_succeeds() {
        let dir = tempdir::temp_dir("clear-corrupt");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        fs::write(&path, b"{ not json at all").unwrap();
        let store = store_at(path.clone());

        let outcome = clear_provider_cache_in(Some(&store));
        assert!(outcome.removed, "the corrupt file counted as present");
        assert!(!path.exists(), "a corrupt cache cannot block its own removal");
    }

    // 4. repeated clears are idempotent
    #[test]
    fn clear_is_idempotent() {
        let dir = tempdir::temp_dir("clear-twice");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = store_at(path.clone());
        store.record_success(
            ProviderKind::Zai,
            &sample_usage(ProviderKind::Zai, 10.0, Some("2026-09-29T18:00:00Z")),
        );

        assert!(clear_provider_cache_in(Some(&store)).removed);
        let second = clear_provider_cache_in(Some(&store));
        assert!(!second.removed, "the second clear found nothing and still succeeded");
        assert!(!path.exists());
    }

    // 5. the cache clear touches only the cache file - history, the
    //    notification latch, and (critically) credential-shaped siblings in
    //    the same directory are left exactly as they were
    #[test]
    fn clear_touches_only_the_cache_file() {
        let dir = tempdir::temp_dir("clear-scope");
        let cache_path = dir.path().join(LAST_GOOD_FILE_NAME);
        let sentinels = [
            ("quota-history-v1.json", "{ \"version\": 1, \"observations\": [] }"),
            ("quota-notifications-v1.json", "{ \"version\": 1, \"enabled\": true, \"windows\": [] }"),
            ("auth.json", "{ \"token\": \"external-credential\" }"),
            ("credentials.json", "{ \"secret\": \"external-credential\" }"),
        ];
        for (name, body) in sentinels {
            fs::write(dir.path().join(name), body).unwrap();
        }
        let store = store_at(cache_path.clone());
        store.record_success(
            ProviderKind::Codex,
            &sample_usage(ProviderKind::Codex, 7.0, Some("2026-09-29T15:00:00Z")),
        );

        clear_provider_cache_in(Some(&store));

        assert!(!cache_path.exists(), "the owned cache file is cleared");
        for (name, body) in sentinels {
            let sibling = dir.path().join(name);
            assert!(sibling.exists(), "{name} must survive a cache clear");
            assert_eq!(
                fs::read_to_string(&sibling).unwrap(),
                body,
                "{name} must be byte-identical after a cache clear"
            );
        }
    }

    // 6. a runtime without a cache store degrades to a successful no-op
    #[test]
    fn clear_without_a_store_succeeds() {
        let outcome = clear_provider_cache_in(None);
        assert_eq!(outcome.category, PROVIDER_CACHE_CATEGORY);
        assert!(!outcome.removed);
    }
}
