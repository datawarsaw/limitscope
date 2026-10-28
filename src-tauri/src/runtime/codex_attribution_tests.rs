//! Raw collector fixtures enter the same registration closure and normalizer
//! as production. No credentials, HTTP calls, or pre-normalized DTOs are used.
use super::*;
use chrono::TimeZone;
use std::sync::atomic::{AtomicUsize, Ordering};

const IDS: [&str; 2] = [PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID];
const HINTS: [&str; 2] = ["11111111", "22222222"];

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap()
}

fn accounts() -> Vec<CodexAccountConfig> {
    let mut accounts = codex_accounts::default_accounts();
    accounts[0].alias = "Personal".into();
    accounts[1].alias = "Work".into();
    accounts[1].codex_home = "fixture-secondary".into();
    accounts
}

fn account_index(home: Option<String>) -> usize {
    match home.as_deref() {
        None => 0,
        Some("fixture-secondary") => 1,
        other => panic!("unexpected fixture reference: {other:?}"),
    }
}

fn usage(index: usize, increment: f64) -> CodexUsage {
    CodexUsage {
        limits: vec![codex::CodexLimitWindow {
            label: "Weekly".into(),
            used_percent: [14.0, 81.0][index] + increment,
            reset_at: None,
        }],
        account: Some(codex::CodexAccount {
            account_hint: HINTS[index].into(),
        }),
        plan_type: Some([" Plus ", " Pro "][index].into()),
        reset_credits: Some(CodexResetCredits {
            banked_credits: [2, 7][index],
            currently_applicable: None,
            checked_at: now().to_rfc3339(),
            source: codex::RESET_CREDITS_SOURCE,
        }),
    }
}

fn assert_own_reading(dto: &ProviderUsageDto, index: usize, increment: f64) {
    assert_eq!(dto.id, IDS[index]);
    assert_eq!(dto.limits.len(), 1);
    assert_eq!(dto.limits[0].used_percent, [14.0, 81.0][index] + increment);
    assert_eq!(
        dto.account.as_ref().unwrap().identity.as_deref(),
        Some(format!("chatgpt:{}", HINTS[index]).as_str())
    );
    assert_eq!(dto.limits[0].reset_at, None);
}

fn core(specs: Vec<ProviderSpec>, clock: &Arc<Mutex<DateTime<Utc>>>) -> RuntimeCore {
    let clock = clock.clone();
    RuntimeCore::with_injections(
        specs,
        5,
        Box::new(|| 0),
        Box::new(move || *clock.lock().unwrap()),
    )
}

async fn publish(core: &Arc<RuntimeCore>) -> RuntimeSnapshot {
    assert!(core.try_begin_cycle());
    let mut snapshots = Vec::new();
    core.run_chain(|event| {
        if let RuntimeEvent::Snapshot(snapshot) = event {
            snapshots.push(snapshot);
        }
    })
    .await;
    assert_eq!(snapshots.len(), 1);
    let snapshot = snapshots.pop().unwrap();
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        serde_json::to_value(core.snapshot()).unwrap()
    );
    snapshot
}

#[tokio::test]
async fn production_normalization_preserves_primary_single_account_behavior() {
    let specs = codex_specs_with_fetch(&codex_accounts::default_accounts(), |home| async move {
        assert_eq!(home, None, "the primary keeps its default source");
        Ok(usage(0, 0.0))
    });
    assert_eq!(specs.len(), 1);
    let core = Arc::new(core(specs, &Arc::new(Mutex::new(now()))));
    let snapshot = publish(&core).await;
    assert_eq!(snapshot.providers.len(), 1);
    let dto = &snapshot.providers[0];
    assert_own_reading(dto, 0, 0.0);
    assert_eq!(dto.health, ProviderHealth::Live);
    assert_eq!(dto.status, "ok");
    assert_eq!(dto.plan_type.as_deref(), Some("plus"));
    assert_eq!(dto.reset_credits.as_ref().unwrap().banked_credits, 2);
    assert_eq!(dto.account_alias, None);
    assert_eq!(dto.monitoring_paused, Some(false));
}

#[tokio::test]
async fn production_normalization_keeps_slot_identity_without_payload_account_hints() {
    let specs = codex_specs_with_fetch(&accounts(), |home| async move {
        let mut result = usage(account_index(home), 0.0);
        result.account = None;
        Ok(result)
    });
    let core = Arc::new(core(specs, &Arc::new(Mutex::new(now()))));
    let snapshot = publish(&core).await;
    assert_eq!(
        snapshot
            .providers
            .iter()
            .map(|dto| dto.id.as_str())
            .collect::<Vec<_>>(),
        IDS
    );
    for (index, dto) in snapshot.providers.iter().enumerate() {
        assert!(dto.account.is_none());
        assert_eq!(dto.limits[0].used_percent, [14.0, 81.0][index]);
    }
}

#[tokio::test]
async fn production_normalization_keeps_bare_failures_in_the_originating_slot() {
    for failed in 0..2 {
        let specs = codex_specs_with_fetch(&accounts(), move |home| async move {
            let index = account_index(home);
            if index == failed {
                Err(ProviderError::new("auth_expired", "fixture expired"))
            } else {
                Ok(usage(index, 0.0))
            }
        });
        let core = Arc::new(core(specs, &Arc::new(Mutex::new(now()))));
        let snapshot = publish(&core).await;
        assert_eq!(snapshot.providers.len(), 2);
        let error = &snapshot.providers[failed];
        assert_eq!(error.id, IDS[failed]);
        assert_eq!(error.health, ProviderHealth::Unavailable);
        assert_eq!(error.error_category.as_deref(), Some("auth_expired"));
        assert!(error.limits.is_empty());
        assert_own_reading(&snapshot.providers[1 - failed], 1 - failed, 0.0);
    }
}

#[tokio::test]
async fn production_normalization_isolated_when_concurrent_accounts_finish_in_either_order() {
    let permits = Arc::new([tokio::sync::Notify::new(), tokio::sync::Notify::new()]);
    let calls = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let specs = codex_specs_with_fetch(&accounts(), {
        let permits = permits.clone();
        let calls = calls.clone();
        move |home| {
            let index = account_index(home);
            let permits = permits.clone();
            let events = events.clone();
            let call = calls[index].fetch_add(1, Ordering::SeqCst);
            async move {
                events.send((index, "started")).unwrap();
                permits[index].notified().await;
                events.send((index, "finished")).unwrap();
                Ok(usage(index, call as f64))
            }
        }
    });
    let clock = Arc::new(Mutex::new(now()));
    let core = Arc::new(core(specs, &clock));
    // Each cycle has both collectors in flight. Reverse completion order on
    // the second cycle; publication and last-good must follow registered ids.
    for (cycle, order) in [[1, 0], [0, 1]].into_iter().enumerate() {
        let task = {
            let core = core.clone();
            tokio::spawn(async move { publish(&core).await })
        };
        let mut started = Vec::new();
        for _ in 0..2 {
            let (index, event) = tokio::time::timeout(Duration::from_secs(5), received.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(event, "started");
            started.push(index);
        }
        started.sort();
        assert_eq!(started, [0, 1]);
        for index in order {
            permits[index].notify_one();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), received.recv())
                    .await
                    .unwrap(),
                Some((index, "finished"))
            );
        }
        let snapshot = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.providers.len(), 2);
        for (index, dto) in snapshot.providers.iter().enumerate() {
            assert_own_reading(dto, index, cycle as f64);
            assert_eq!(dto.health, ProviderHealth::Live);
            assert_eq!(dto.plan_type.as_deref(), Some(["plus", "pro"][index]));
            assert_eq!(
                dto.reset_credits.as_ref().unwrap().banked_credits,
                [2, 7][index]
            );
            assert_own_reading(
                &core.inner.lock().unwrap().last_good[IDS[index]],
                index,
                cycle as f64,
            );
        }
        *clock.lock().unwrap() += chrono::Duration::seconds(1);
    }
}

struct TempStores(std::path::PathBuf);
impl Drop for TempStores {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("last-good.json"));
        let _ = std::fs::remove_file(self.0.join("history.json"));
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[tokio::test]
async fn production_normalization_isolates_last_good_cooldown_history_pause_and_stale_cache() {
    // Exercise both directions: either slot may fail, cool down and pause.
    for affected in 0..2 {
        let other = 1 - affected;
        let directory = TempStores(
            std::env::temp_dir().join(format!("limitscope-attribution-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir(&directory.0).unwrap();
        let clock = Arc::new(Mutex::new(now()));
        let history_clock = clock.clone();
        let history = Arc::new(QuotaHistoryStore::open_with_clock(
            directory.0.join("history.json"),
            Box::new(move || history_clock.lock().unwrap().timestamp_millis()),
        ));
        let cache_clock = clock.clone();
        let cache = Arc::new(ProviderLastGoodStore::open_with_clock(
            directory.0.join("last-good.json"),
            Box::new(move || *cache_clock.lock().unwrap()),
        ));
        let calls = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let configs = accounts();
        let specs = codex_specs_with_fetch(&configs, {
            let calls = calls.clone();
            move |home| {
                let index = account_index(home);
                let call = calls[index].fetch_add(1, Ordering::SeqCst);
                async move {
                    if index == affected && call == 1 {
                        let mut error = ProviderError::http_failure(
                            reqwest::StatusCode::TOO_MANY_REQUESTS,
                            "fixture cooldown",
                        )
                        .with_identity_hint(Some(format!("chatgpt:{}", HINTS[index])));
                        error.retry_after_ms = Some(60_000);
                        Err(error)
                    } else {
                        Ok(usage(index, call as f64))
                    }
                }
            }
        });
        let core = Arc::new(
            core(specs.clone(), &clock)
                .with_history_store(Some(history.clone()))
                .with_last_good_store(Some(cache.clone())),
        );
        // Set config before replacing the real fetch source with the fixture
        // registry. Subsequent pause changes use the public production setter.
        core.set_codex_accounts(&configs);
        core.set_codex_specs(specs.clone());
        let first = publish(&core).await;
        assert_eq!(first.providers.len(), 2);
        for index in 0..2 {
            assert_own_reading(&first.providers[index], index, 0.0);
        }
        assert_eq!(history.history().len(), 2);
        *clock.lock().unwrap() += chrono::Duration::seconds(1);

        let failed = publish(&core).await;
        assert_own_reading(&failed.providers[affected], affected, 0.0);
        assert_eq!(failed.providers[affected].health, ProviderHealth::Error);
        assert_eq!(
            failed.providers[affected].checked_at,
            first.providers[affected].checked_at
        );
        assert_eq!(failed.providers[affected].error_http_status, Some(429));
        assert_own_reading(&failed.providers[other], other, 1.0);
        assert!(core.active_cooldown(IDS[affected]).is_some());
        assert!(core.active_cooldown(IDS[other]).is_none());
        *clock.lock().unwrap() += chrono::Duration::seconds(1);

        let cooling = publish(&core).await;
        assert_eq!(cooling.providers[affected].health, ProviderHealth::Cooldown);
        assert_own_reading(&cooling.providers[affected], affected, 0.0);
        assert_own_reading(&cooling.providers[other], other, 2.0);
        assert_eq!(calls[affected].load(Ordering::SeqCst), 2);

        let mut paused = configs.clone();
        paused[affected].monitored = false;
        assert!(core.set_codex_accounts(&paused));
        *clock.lock().unwrap() += chrono::Duration::seconds(61);
        let frozen = publish(&core).await;
        assert_eq!(frozen.providers[affected].monitoring_paused, Some(true));
        assert_eq!(frozen.providers[other].monitoring_paused, Some(false));
        assert_eq!(
            frozen.providers[affected].checked_at,
            first.providers[affected].checked_at
        );
        assert_own_reading(&frozen.providers[affected], affected, 0.0);
        assert_own_reading(&frozen.providers[other], other, 3.0);
        assert_eq!(calls[affected].load(Ordering::SeqCst), 2);
        for index in 0..2 {
            assert_eq!(
                frozen.providers[index].account_alias.as_deref(),
                Some(["Personal", "Work"][index])
            );
            let observations = history.history_range(Some(IDS[index]), None, None, "24h");
            assert_eq!(observations.len(), if index == affected { 1 } else { 4 });
            for (call, observation) in observations.iter().enumerate() {
                assert_eq!(observation.provider_id, IDS[index]);
                assert_eq!(
                    observation.account.as_deref(),
                    Some(format!("chatgpt:{}", HINTS[index]).as_str())
                );
                assert_eq!(observation.used_percent, [14.0, 81.0][index] + call as f64);
            }
        }

        // Reopen actual serialized cache: each slot hydrates its own last
        // successful reading as stale, and never hydrates a cooldown/credit.
        let reopened = Arc::new(ProviderLastGoodStore::open(
            directory.0.join("last-good.json"),
        ));
        let restarted = Arc::new(self::core(specs, &clock).with_last_good_store(Some(reopened)));
        let stale = restarted.snapshot();
        assert_eq!(stale.providers.len(), 2);
        for index in 0..2 {
            assert_own_reading(
                &stale.providers[index],
                index,
                if index == affected { 0.0 } else { 3.0 },
            );
            assert_eq!(stale.providers[index].health, ProviderHealth::Stale);
            assert_eq!(stale.providers[index].data_freshness, Some("stale"));
            assert!(stale.providers[index].reset_credits.is_none());
            assert!(restarted.active_cooldown(IDS[index]).is_none());
        }
        assert!(crate::history::observations_from_usages(&stale.providers).is_empty());

        assert!(core.set_codex_accounts(&configs));
        *clock.lock().unwrap() += chrono::Duration::seconds(1);
        let resumed = publish(&core).await;
        assert_own_reading(&resumed.providers[affected], affected, 2.0);
        assert_own_reading(&resumed.providers[other], other, 4.0);
        assert_eq!(resumed.providers[affected].health, ProviderHealth::Live);
        assert_eq!(resumed.providers[affected].monitoring_paused, Some(false));
    }
}
