//! Exercises auth parsing, response parsing, normalization and the runtime gate.
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn fixture_auth(user: &str, workspace: &str) -> String {
    let claims = serde_json::json!({"https://api.openai.com/auth": {"chatgpt_user_id":user,"user_id":user,"chatgpt_account_id":workspace}});
    let jwt = format!("e30.{}.fixture", URL_SAFE_NO_PAD.encode(claims.to_string()));
    serde_json::json!({"tokens":{"access_token":jwt,"id_token":jwt,"account_id":workspace}}).to_string()
}

/// A temporary directory that removes itself when the test ends.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("limitscope-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two Codex accounts, each with its own home.
fn two_accounts(
    primary_home: &std::path::Path,
    secondary_home: &std::path::Path,
) -> Vec<codex_accounts::CodexAccountConfig> {
    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = primary_home.to_string_lossy().into_owned();
    accounts[0].alias = "Personal".to_string();
    accounts[1].codex_home = secondary_home.to_string_lossy().into_owned();
    accounts[1].alias = "Work".to_string();
    accounts
}

/// Collectors that answer from each account's own auth file.
fn specs_for(accounts: &[codex_accounts::CodexAccountConfig], body: &'static [u8]) -> Vec<ProviderSpec> {
    codex_specs_with_fetch(accounts, move |home| {
        let body = body;
        async move {
            let path = std::path::Path::new(home.as_deref().unwrap()).join("auth.json");
            let raw = std::fs::read_to_string(path).unwrap();
            codex::fixture_collect(&raw, body, Utc::now())
        }
    })
}

/// A quota body with one window at the given percentage.
fn quota_body(used_percent: u32) -> &'static [u8] {
    match used_percent {
        38 => br#"{"rate_limit":{"primary_window":{"used_percent":38,"limit_window_seconds":18000}}}"#,
        79 => br#"{"rate_limit":{"primary_window":{"used_percent":79,"limit_window_seconds":18000}}}"#,
        81 => br#"{"rate_limit":{"primary_window":{"used_percent":81,"limit_window_seconds":18000}}}"#,
        other => panic!("no fixture body for {other}"),
    }
}

/// The identity the runtime currently observes for one Codex slot.
fn observed_identity(snapshot: &RuntimeSnapshot, provider_id: &str) -> Option<String> {
    snapshot
        .codex_identity_states
        .iter()
        .find(|state| state.provider_id == provider_id)
        .and_then(|state| state.identity.clone())
}

/// The generation the runtime currently observes for one Codex slot.
fn observed_generation(snapshot: &RuntimeSnapshot, provider_id: &str) -> Option<u64> {
    snapshot
        .codex_identity_states
        .iter()
        .find(|state| state.provider_id == provider_id)
        .map(|state| state.generation)
}

/// The provider entry the snapshot carries for one id.
fn provider_entry(snapshot: &RuntimeSnapshot, provider_id: &str) -> ProviderUsageDto {
    snapshot
        .providers
        .iter()
        .find(|usage| usage.id == provider_id)
        .cloned()
        .unwrap_or_else(|| panic!("the snapshot carries no {provider_id} entry"))
}

/// An unrelated provider that always answers with one live window. It
/// advances the global snapshot sequence without touching any Codex state.
fn unrelated_spec(kind: ProviderKind, percent: f64) -> ProviderSpec {
    ProviderSpec {
        kind,
        fetch: Arc::new(move || {
            Box::pin(async move {
                Ok(ProviderUsageDto {
                    id: kind.id().to_string(),
                    name: kind.name().to_string(),
                    status: "ok",
                    health: ProviderHealth::Live,
                    checked_at: Utc::now().to_rfc3339(),
                    limits: vec![UsageLimitDto {
                        label: "5-hour".to_string(),
                        used_percent: percent,
                        reset_text: None,
                        reset_at: None,
                    }],
                    account: if matches!(kind.id(), "zai" | "opencode-go" | "grok") {
                Some(AccountAttributionDto { label: "fixture".into(), note: None,
                    identity: Some(crate::credential_identity::fingerprint(kind.id(), "fixture-credential")) })
            } else { None },
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
                    account_alias: None,
                    monitoring_paused: None,
                    fallback_failure: None,
                }) as Result<ProviderUsageDto, ProviderFailure>
            }) as FetchFuture
        }),
    }
}

#[tokio::test]
async fn same_workspace_switch_isolated_across_snapshot_failure_pause_and_cooldown() {
    let dir = std::env::temp_dir().join(format!("limitscope-identity-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let auth_file = dir.join("auth.json");
    let workspace = "fixture-workspace-12345678";
    std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();
    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = dir.to_string_lossy().into_owned();
    let fail = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let specs = codex_specs_with_fetch(&accounts, {
        let fail = fail.clone(); let calls = calls.clone();
        move |home| {
            let fail = fail.clone(); let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let path = std::path::Path::new(home.as_deref().unwrap()).join("auth.json");
                let raw = std::fs::read_to_string(path).unwrap();
                let body = if fail.load(Ordering::SeqCst) { b"{}".as_slice() } else { br#"{"rate_limit":{"primary_window":{"used_percent":38,"limit_window_seconds":18000}}}"#.as_slice() };
                codex::fixture_collect(&raw, body, Utc::now())
            }
        }
    });
    let mut core = RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);
    core.run_cycle().await;
    let a = core.snapshot().providers[0].clone();
    assert_eq!(a.limits[0].used_percent, 38.0);
    let a_id = a.account.as_ref().unwrap().identity.clone().unwrap();
    // Same session can be resolved again without losing the reading.
    std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();
    assert_eq!(core.snapshot().providers[0].limits.len(), 1);
    // A cooldown from A must not suppress B's collection.
    let mut cooldown = failure_from_provider_error(ProviderError::new("network", "fixture"));
    cooldown.identity = Some(a_id);
    cooldown.retry_after_ms = Some(60_000);
    core.maybe_apply_cooldown(ProviderKind::Codex, &cooldown);
    std::fs::write(&auth_file, fixture_auth("user-b", workspace)).unwrap();
    assert!(core.snapshot().providers[0].limits.is_empty());
    let mut paused = accounts.clone(); paused[0].monitored = false;
    core.set_codex_accounts(&paused);
    core.run_cycle().await;
    assert!(core.snapshot().providers[0].limits.is_empty());
    core.set_codex_accounts(&accounts);
    fail.store(true, Ordering::SeqCst);
    core.run_cycle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "B must bypass A's cooldown");
    assert!(core.snapshot().providers[0].limits.is_empty(), "B's failed collection must not retain A");
    fail.store(false, Ordering::SeqCst);
    core.run_cycle().await;
    let b = core.snapshot().providers[0].clone();
    assert_eq!(b.limits[0].used_percent, 38.0);
    assert_ne!(a.account.unwrap().identity, b.account.unwrap().identity);
    std::fs::remove_file(auth_file).unwrap();
    assert!(core.snapshot().providers[0].limits.is_empty());
    std::fs::remove_dir(dir).unwrap();
}

#[tokio::test]
async fn in_flight_result_cannot_commit_after_switch_or_pause() {
    for pause in [false, true] {
        let dir = std::env::temp_dir().join(format!("limitscope-delayed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let auth_file = dir.join("auth.json");
        let workspace = "fixture-workspace-12345678";
        std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();
        let mut accounts = codex_accounts::default_accounts();
        accounts[0].codex_home = dir.to_string_lossy().into_owned();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let specs = codex_specs_with_fetch(&accounts, {
            let entered = entered.clone(); let release = release.clone();
            move |home| {
                let entered = entered.clone(); let release = release.clone();
                async move {
                    let raw = std::fs::read_to_string(std::path::Path::new(home.as_deref().unwrap()).join("auth.json")).unwrap();
                    entered.notify_one();
                    release.notified().await;
                    codex::fixture_collect(&raw, br#"{"rate_limit":{"primary_window":{"used_percent":38,"limit_window_seconds":18000}}}"#, Utc::now())
                }
            }
        });
        let history = Arc::new(QuotaHistoryStore::open(dir.join("history.json")));
        let cache = Arc::new(ProviderLastGoodStore::open(dir.join("last-good.json")));
        let mut core = RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_history_store(Some(history.clone())).with_last_good_store(Some(cache.clone()));
        core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
        let core = Arc::new(core);
        core.set_codex_accounts(&accounts); core.set_codex_specs(specs);
        let running = { let core = core.clone(); tokio::spawn(async move { core.run_cycle().await }) };
        entered.notified().await;
        if pause { accounts[0].monitored = false; core.set_codex_accounts(&accounts); }
        else { std::fs::write(&auth_file, fixture_auth("user-b", workspace)).unwrap(); }
        release.notify_one(); running.await.unwrap();
        assert!(history.history().is_empty(), "obsolete job recorded history");
        assert!(cache.get_persisted(PRIMARY_ACCOUNT_ID).is_none(), "obsolete job persisted a reading");
        assert!(core.snapshot().providers.iter().all(|u| u.limits.is_empty()), "obsolete job surfaced a reading");
        let _ = std::fs::remove_file(auth_file);
        let _ = std::fs::remove_file(dir.join("history.json"));
        let _ = std::fs::remove_file(dir.join("last-good.json"));
        std::fs::remove_dir(dir).unwrap();
    }
}

#[tokio::test]
async fn missing_login_keeps_the_slot_unavailable_without_values() {
    let dir = std::env::temp_dir().join(format!("limitscope-no-login-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = dir.to_string_lossy().into_owned();
    let specs = codex_specs_with_fetch(&accounts, |_| async { panic!("missing identity must not fetch") });
    let mut core = RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core); core.set_codex_accounts(&accounts); core.set_codex_specs(specs);
    core.run_cycle().await;
    let snapshot = core.snapshot();
    assert_eq!(snapshot.providers.len(), 1);
    assert_eq!(snapshot.providers[0].health, ProviderHealth::Unavailable);
    assert!(snapshot.providers[0].limits.is_empty());
    std::fs::remove_dir(dir).unwrap();
}

/// Two different users in the SAME Team workspace. The workspace is shared;
/// only the user separates them, so each slot must keep its own binding,
/// its own reading and its own history rows.
#[tokio::test]
async fn two_users_in_one_team_workspace_stay_isolated_across_both_slots() {
    let dir = TempDir::new("identity-two-users");
    let primary_home = dir.join("primary");
    let secondary_home = dir.join("secondary");
    std::fs::create_dir_all(&primary_home).unwrap();
    std::fs::create_dir_all(&secondary_home).unwrap();
    let workspace = "fixture-workspace-12345678";
    std::fs::write(primary_home.join("auth.json"), fixture_auth("user-a", workspace)).unwrap();
    std::fs::write(secondary_home.join("auth.json"), fixture_auth("user-b", workspace)).unwrap();

    let accounts = two_accounts(&primary_home, &secondary_home);
    let specs = specs_for(&accounts, quota_body(38));
    let history = Arc::new(QuotaHistoryStore::open(dir.join("history.json")));
    let mut core =
        RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_history_store(Some(history.clone()));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);
    core.run_cycle().await;

    let snapshot = core.snapshot();
    let primary_identity = observed_identity(&snapshot, "openai-codex").unwrap();
    let secondary_identity = observed_identity(&snapshot, "openai-codex:secondary").unwrap();
    assert_ne!(
        primary_identity, secondary_identity,
        "two users in one workspace must never share a binding"
    );
    assert_eq!(provider_entry(&snapshot, "openai-codex").limits[0].used_percent, 38.0);
    assert_eq!(
        provider_entry(&snapshot, "openai-codex:secondary").limits[0].used_percent,
        38.0
    );

    // Each slot's history rows carry its own binding, so neither slot can
    // be read as the other one's user.
    let rows = history.history();
    let primary_rows: Vec<_> = rows.iter().filter(|row| row.provider_id == "openai-codex").collect();
    let secondary_rows: Vec<_> = rows
        .iter()
        .filter(|row| row.provider_id == "openai-codex:secondary")
        .collect();
    assert_eq!(primary_rows.len(), 1);
    assert_eq!(secondary_rows.len(), 1);
    assert_eq!(primary_rows[0].account.as_deref(), Some(primary_identity.as_str()));
    assert_eq!(secondary_rows[0].account.as_deref(), Some(secondary_identity.as_str()));
}

/// One slot cannot prove its login. It fails closed on its own: the other
/// slot keeps its reading, and the unrelated providers are untouched.
#[tokio::test]
async fn a_missing_login_in_one_slot_leaves_the_other_slot_untouched() {
    let dir = TempDir::new("identity-one-slot-fails");
    let primary_home = dir.join("primary");
    let secondary_home = dir.join("secondary");
    std::fs::create_dir_all(&primary_home).unwrap();
    std::fs::create_dir_all(&secondary_home).unwrap();
    let workspace = "fixture-workspace-12345678";
    std::fs::write(primary_home.join("auth.json"), fixture_auth("user-a", workspace)).unwrap();
    // The secondary slot has a home but no login at all.

    let accounts = two_accounts(&primary_home, &secondary_home);
    let specs = codex_specs_with_fetch(&accounts, |home| async move {
        let path = std::path::Path::new(home.as_deref().unwrap()).join("auth.json");
        let raw = std::fs::read_to_string(path).unwrap();
        codex::fixture_collect(&raw, quota_body(38), Utc::now())
    });
    let history = Arc::new(QuotaHistoryStore::open(dir.join("history.json")));
    let mut core =
        RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_history_store(Some(history.clone()));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);
    core.run_cycle().await;

    let snapshot = core.snapshot();
    let secondary = provider_entry(&snapshot, "openai-codex:secondary");
    assert_eq!(secondary.health, ProviderHealth::Unavailable);
    assert!(secondary.limits.is_empty());
    assert!(secondary.account.is_none());
    assert!(
        observed_identity(&snapshot, "openai-codex:secondary").is_none(),
        "the unprovable slot has no binding"
    );

    // The healthy slot is untouched.
    assert_eq!(provider_entry(&snapshot, "openai-codex").limits[0].used_percent, 38.0);
    let rows = history.history();
    assert_eq!(
        rows.iter().filter(|row| row.provider_id == "openai-codex").count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.provider_id == "openai-codex:secondary")
            .count(),
        0,
        "the unprovable slot records nothing"
    );
}

#[test]
fn reregistered_slot_gets_a_new_generation_even_for_the_same_user() {
    let mut accounts = codex_accounts::default_accounts();
    accounts[1].codex_home = "fixture-secondary".to_string();
    let primary_identity = format!("chatgpt-v2:{}", "a".repeat(64));
    let secondary_identity = format!("chatgpt-v2:{}", "b".repeat(64));
    let mut core = RuntimeCore::with_injections(Vec::new(), 5, Box::new(|| 0), Box::new(Utc::now));
    core.codex_identity_resolver = Some(Arc::new({
        let primary_identity = primary_identity.clone();
        let secondary_identity = secondary_identity.clone();
        move |home| Some(if home.is_some() { secondary_identity.clone() } else { primary_identity.clone() })
    }));
    core.set_codex_accounts(&accounts);
    let first = core.snapshot();
    let primary_generation = observed_generation(&first, PRIMARY_ACCOUNT_ID).unwrap();
    let secondary_generation = observed_generation(&first, SECONDARY_ACCOUNT_ID).unwrap();

    let mut removed = accounts.clone();
    removed[1].codex_home.clear();
    core.set_codex_accounts(&removed);
    let absent = core.snapshot();
    assert_eq!(observed_generation(&absent, SECONDARY_ACCOUNT_ID), None);
    assert_eq!(observed_generation(&absent, PRIMARY_ACCOUNT_ID), Some(primary_generation));

    core.set_codex_accounts(&accounts);
    let returned = core.snapshot();
    assert!(observed_generation(&returned, SECONDARY_ACCOUNT_ID).unwrap() > secondary_generation);
    assert_eq!(observed_identity(&returned, SECONDARY_ACCOUNT_ID), Some(secondary_identity));
    assert_eq!(observed_generation(&returned, PRIMARY_ACCOUNT_ID), Some(primary_generation));
    assert_eq!(returned.codex_identity_states, core.snapshot().codex_identity_states);
}

/// The observed session moves A -> B -> A. Returning to A must not revive
/// A's earlier reading: the generation advanced twice, so the old
/// observation belongs to a session the runtime has moved past.
#[tokio::test]
async fn returning_to_an_earlier_user_does_not_revive_its_old_reading() {
    let dir = TempDir::new("identity-aba");
    let home = dir.join("primary");
    std::fs::create_dir_all(&home).unwrap();
    let auth_file = home.join("auth.json");
    let workspace = "fixture-workspace-12345678";
    std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();

    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = home.to_string_lossy().into_owned();
    let specs = specs_for(&accounts, quota_body(38));
    let mut core = RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);
    core.run_cycle().await;

    let first = core.snapshot();
    let first_generation = observed_generation(&first, "openai-codex").unwrap();
    assert_eq!(provider_entry(&first, "openai-codex").limits[0].used_percent, 38.0);

    // A -> B: the reading is dropped, and the generation advances.
    std::fs::write(&auth_file, fixture_auth("user-b", workspace)).unwrap();
    let at_b = core.snapshot();
    assert!(provider_entry(&at_b, "openai-codex").limits.is_empty());
    let b_generation = observed_generation(&at_b, "openai-codex").unwrap();
    assert!(b_generation > first_generation);

    // B -> A: the identity is A again, but the generation advanced twice.
    std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();
    let back_at_a = core.snapshot();
    let a_generation = observed_generation(&back_at_a, "openai-codex").unwrap();
    assert!(a_generation > b_generation);
    assert_eq!(
        observed_identity(&back_at_a, "openai-codex"),
        observed_identity(&first, "openai-codex"),
        "the identity is A again"
    );
    assert!(
        provider_entry(&back_at_a, "openai-codex").limits.is_empty(),
        "A's earlier reading must not be revived by returning to A"
    );
}

/// A collection result that was started under an older observed generation
/// must not commit, even when the identity it carries is the current one
/// again. The A -> B -> A move advanced the generation twice, so the result
/// belongs to a session the runtime has moved past.
#[tokio::test]
async fn a_result_started_under_an_older_generation_cannot_commit_after_a_move() {
    let dir = TempDir::new("identity-out-of-order");
    let home = dir.join("primary");
    std::fs::create_dir_all(&home).unwrap();
    let auth_file = home.join("auth.json");
    let workspace = "fixture-workspace-12345678";
    std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();

    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = home.to_string_lossy().into_owned();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let specs = codex_specs_with_fetch(&accounts, {
        let entered = entered.clone();
        let release = release.clone();
        move |home| {
            let entered = entered.clone();
            let release = release.clone();
            async move {
                let raw = std::fs::read_to_string(
                    std::path::Path::new(home.as_deref().unwrap()).join("auth.json"),
                )
                .unwrap();
                entered.notify_one();
                release.notified().await;
                codex::fixture_collect(&raw, quota_body(38), Utc::now())
            }
        }
    });
    let history = Arc::new(QuotaHistoryStore::open(dir.join("history.json")));
    let cache = Arc::new(ProviderLastGoodStore::open(dir.join("last-good.json")));
    let mut core =
        RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_history_store(Some(history.clone()))
            .with_last_good_store(Some(cache.clone()));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);

    let running = {
        let core = core.clone();
        tokio::spawn(async move { core.run_cycle().await })
    };
    entered.notified().await;
    // The session moves away and back while the collection is in flight.
    std::fs::write(&auth_file, fixture_auth("user-b", workspace)).unwrap();
    let _ = core.snapshot();
    std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();
    let _ = core.snapshot();
    release.notify_one();
    running.await.unwrap();

    assert!(
        history.history().is_empty(),
        "a result from an older generation records no history"
    );
    assert!(
        cache.get_persisted("openai-codex").is_none(),
        "a result from an older generation persists no reading"
    );
    assert!(
        provider_entry(&core.snapshot(), "openai-codex").limits.is_empty(),
        "a result from an older generation surfaces no reading"
    );
}

/// A session change immediately before the protected state mutations. The
/// result must not reach the history, the last-good cache or the
/// notification lane.
#[tokio::test]
async fn a_session_change_before_the_mutations_writes_no_history_cache_or_notification() {
    let dir = TempDir::new("identity-mutation-gap");
    let home = dir.join("primary");
    std::fs::create_dir_all(&home).unwrap();
    let auth_file = home.join("auth.json");
    let workspace = "fixture-workspace-12345678";
    std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();

    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = home.to_string_lossy().into_owned();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let specs = codex_specs_with_fetch(&accounts, {
        let entered = entered.clone();
        let release = release.clone();
        move |home| {
            let entered = entered.clone();
            let release = release.clone();
            async move {
                let raw = std::fs::read_to_string(
                    std::path::Path::new(home.as_deref().unwrap()).join("auth.json"),
                )
                .unwrap();
                entered.notify_one();
                release.notified().await;
                codex::fixture_collect(&raw, quota_body(81), Utc::now())
            }
        }
    });
    let history = Arc::new(QuotaHistoryStore::open(dir.join("history.json")));
    let cache = Arc::new(ProviderLastGoodStore::open(dir.join("last-good.json")));
    let delivered: Arc<std::sync::Mutex<Vec<crate::notifications::QuotaNotification>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = delivered.clone();
    let lane = Arc::new(crate::notifications::NotificationLane::open(
        dir.join("notifications.json"),
        Box::new(move |notes| {
            sink.lock().unwrap().extend_from_slice(notes);
        }),
    ));
    lane.set_enabled(true);
    let mut core =
        RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_history_store(Some(history.clone()))
            .with_last_good_store(Some(cache.clone()))
            .with_notification_lane(Some(lane));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);

    let running = {
        let core = core.clone();
        tokio::spawn(async move { core.run_cycle().await })
    };
    entered.notified().await;
    // The session moves while the collection is in flight: the result would
    // cross the threshold, so a notification would be delivered if it
    // committed.
    std::fs::write(&auth_file, fixture_auth("user-b", workspace)).unwrap();
    release.notify_one();
    running.await.unwrap();

    assert!(
        history.history().is_empty(),
        "the obsolete result records no history"
    );
    assert!(
        cache.get_persisted("openai-codex").is_none(),
        "the obsolete result persists no reading"
    );
    assert!(
        delivered.lock().unwrap().is_empty(),
        "the obsolete result raises no notification"
    );
}

/// The cache holds the previous user's reading. After the session changes,
/// the cached reading is neither restored nor kept on disk.
#[tokio::test]
async fn a_cached_reading_from_another_user_is_neither_restored_nor_kept() {
    let dir = TempDir::new("identity-cache-restoration");
    let home = dir.join("primary");
    std::fs::create_dir_all(&home).unwrap();
    let auth_file = home.join("auth.json");
    let workspace = "fixture-workspace-12345678";
    std::fs::write(&auth_file, fixture_auth("user-a", workspace)).unwrap();
    let cache_path = dir.join("last-good.json");

    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = home.to_string_lossy().into_owned();
    let specs = specs_for(&accounts, quota_body(38));
    let store = Arc::new(ProviderLastGoodStore::open(cache_path.clone()));
    let mut core =
        RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_last_good_store(Some(store.clone()));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs.clone());
    core.run_cycle().await;
    assert!(
        store.get_persisted("openai-codex").is_some(),
        "the first session caches its reading"
    );

    // The account is now logged into a different user, and the app starts
    // again over the same cache file.
    std::fs::write(&auth_file, fixture_auth("user-b", workspace)).unwrap();
    let reopened = Arc::new(ProviderLastGoodStore::open(cache_path.clone()));
    let mut core =
        RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_last_good_store(Some(reopened.clone()));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);

    let snapshot = core.snapshot();
    assert!(
        provider_entry(&snapshot, "openai-codex").limits.is_empty(),
        "the previous user's cached reading is not restored"
    );
    assert!(
        reopened.get_persisted("openai-codex").is_none(),
        "the previous user's cached reading is not kept on disk"
    );
}

/// An unrelated provider advances the global snapshot sequence. The Codex
/// ownership fence is per slot, so valid Codex history must survive that
/// activity unchanged.
#[tokio::test]
async fn unrelated_provider_activity_does_not_discard_valid_codex_history() {
    let dir = TempDir::new("identity-unrelated-activity");
    let home = dir.join("primary");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = "fixture-workspace-12345678";
    std::fs::write(home.join("auth.json"), fixture_auth("user-a", workspace)).unwrap();

    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = home.to_string_lossy().into_owned();
    let mut specs = specs_for(&accounts, quota_body(38));
    specs.push(unrelated_spec(ProviderKind::Zai, 12.0));
    let history = Arc::new(QuotaHistoryStore::open(dir.join("history.json")));
    let mut core =
        RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_history_store(Some(history.clone()));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);
    core.run_cycle().await;

    let first = core.snapshot();
    let first_fence = crate::history::CodexSnapshotFence::new(&first.codex_identity_states);

    // The unrelated provider runs again and moves the global sequence.
    core.run_cycle().await;
    let settled = core.snapshot();
    assert!(
        settled.seq > first.seq,
        "the global snapshot sequence moved: the previous fence compared that +         sequence, so this activity would have discarded every Codex row"
    );
    let settled_fence = crate::history::CodexSnapshotFence::new(&settled.codex_identity_states);

    let stored = history.history();
    let stored_codex = stored
        .iter()
        .filter(|row| row.provider_id == "openai-codex")
        .count();
    let stored_zai = stored
        .iter()
        .filter(|row| row.provider_id == "zai")
        .count();
    assert!(stored_codex >= 2, "both cycles recorded a Codex row");
    let rows = crate::history::settle_codex_accounts_for_fences(stored, &first_fence, &settled_fence);
    assert_eq!(
        rows.iter()
            .filter(|row| row.provider_id == "openai-codex")
            .count(),
        stored_codex,
        "valid Codex history survives unrelated provider activity"
    );
    assert_eq!(
        rows.iter().filter(|row| row.provider_id == "zai").count(),
        stored_zai,
        "the unrelated provider is unaffected"
    );
}

/// A single Codex account with an unrelated provider beside it: the
/// identity machinery is inert for the single-account installation and
/// leaves every other provider alone.
#[tokio::test]
async fn a_single_codex_account_leaves_unrelated_providers_untouched() {
    let dir = TempDir::new("identity-single-account");
    let home = dir.join("primary");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = "fixture-workspace-12345678";
    std::fs::write(home.join("auth.json"), fixture_auth("user-a", workspace)).unwrap();

    let mut accounts = codex_accounts::default_accounts();
    accounts[0].codex_home = home.to_string_lossy().into_owned();
    // Only the primary account is configured.
    accounts[1].codex_home = String::new();
    let mut specs = specs_for(&accounts, quota_body(38));
    specs.push(unrelated_spec(ProviderKind::Zai, 12.0));
    let history = Arc::new(QuotaHistoryStore::open(dir.join("history.json")));
    let mut core =
        RuntimeCore::with_injections(specs.clone(), 5, Box::new(|| 0), Box::new(Utc::now))
            .with_history_store(Some(history.clone()));
    core.codex_identity_resolver = Some(Arc::new(codex::current_codex_identity));
    let core = Arc::new(core);
    core.set_codex_accounts(&accounts);
    core.set_codex_specs(specs);
    core.run_cycle().await;

    let snapshot = core.snapshot();
    assert_eq!(provider_entry(&snapshot, "openai-codex").limits[0].used_percent, 38.0);
    assert_eq!(provider_entry(&snapshot, "zai").limits[0].used_percent, 12.0);
    assert!(
        !snapshot
            .providers
            .iter()
            .any(|usage| usage.id == "openai-codex:secondary"),
        "an unconfigured second slot contributes no provider"
    );
    let rows = history.history();
    assert_eq!(
        rows.iter()
            .filter(|row| row.provider_id == "openai-codex")
            .count(),
        1
    );
    assert_eq!(
        rows.iter().filter(|row| row.provider_id == "zai").count(),
        1
    );
}
