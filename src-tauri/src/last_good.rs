//! Persisted normalized last-good provider state for LimitScope v0.6.
//!
//! Owns the bounded on-disk cache `<app-data>/provider-last-good-v1.json`.
//! On cold app start, before the first live provider refresh completes, the
//! runtime hydrates this state so the UI may immediately show the last known
//! provider quota state.
//!
//! Trust rules:
//! - Hydrated state is explicitly marked `status: "stale"`, `data_freshness: "stale"`.
//! - It is NEVER treated as live data or a new observation.
//! - Hydration creates zero history observations and does not increment history revision.
//! - Hydration never triggers notifications.
//! - Prediction engine hides predictions for stale/hydrated providers.
//! - Account isolation is strictly preserved: Account A's cache never attaches to Account B,
//!   and an unattributed cache never attaches to a newly proven account.
//! - Bounded retention: entries older than 7 days ([\`MAX_CACHE_AGE_SECS\`]) are discarded at load time.
//! - Reset safety: quota windows whose `reset_at` is already in the past at load time are omitted.
//! - Registry safety: unknown providers and malformed windows are dropped without invalidating valid entries.
//! - Self-healing: corrupt files or foreign schema versions degrade safely to empty and rewrite cleanly.
//! - Write policy: only genuine successful live provider data updates the cache. Errors, cooldown skips,
//!   stale projections, and unavailable states NEVER overwrite a good cached entry.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::runtime::{
    AccountAttributionDto, ProviderHealth, ProviderKind, ProviderSpec, ProviderUsageDto,
    UsageLimitDto,
};

pub const LAST_GOOD_FILE_NAME: &str = "provider-last-good-v1.json";
pub const SCHEMA_VERSION: u32 = 1;
/// Maximum cache age: 7 days. Stale entries older than this are discarded at hydration time.
pub const MAX_CACHE_AGE_SECS: i64 = 7 * 24 * 3600;
/// Tolerance for clock skew into the future (5 minutes).
const FUTURE_TOLERANCE_SECS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedQuotaLimit {
    pub label: String,
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedAccountAttribution {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedProviderState {
    pub provider_id: String,
    pub provider_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<PersistedAccountAttribution>,
    pub limits: Vec<PersistedQuotaLimit>,
    pub last_successful_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedLastGoodEnvelope {
    pub version: u32,
    pub providers: Vec<PersistedProviderState>,
}

pub struct ProviderLastGoodStore {
    path: PathBuf,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    state: Mutex<HashMap<String, PersistedProviderState>>,
}

impl ProviderLastGoodStore {
    pub fn open(path: PathBuf) -> Self {
        Self::open_with_clock(path, Box::new(Utc::now))
    }

    pub fn open_with_clock(
        path: PathBuf,
        now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    ) -> Self {
        let store = Self {
            path,
            now,
            state: Mutex::new(HashMap::new()),
        };
        store.load_from_disk();
        store
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn load_from_disk(&self) {
        let Ok(raw) = fs::read_to_string(&self.path) else {
            return;
        };
        let (providers, healthy) = parse_envelope(&raw);
        let mut state = self.state.lock().unwrap();
        *state = providers;
        if !healthy {
            let _ = self.persist_locked(&state);
        }
    }

    #[allow(dead_code)]
    pub fn persist(&self) -> Result<(), std::io::Error> {
        let state = self.state.lock().unwrap();
        self.persist_locked(&state)
    }

    fn persist_locked(
        &self,
        state: &HashMap<String, PersistedProviderState>,
    ) -> Result<(), std::io::Error> {
        let mut providers: Vec<PersistedProviderState> = state.values().cloned().collect();
        providers.sort_by(|a, b| a.provider_id.cmp(&b.provider_id));
        let envelope = PersistedLastGoodEnvelope {
            version: SCHEMA_VERSION,
            providers,
        };
        let json = serde_json::to_string_pretty(&envelope)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let temp_path = self.path.with_extension("json.tmp");
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&temp_path, json.as_bytes())
            .and_then(|()| fs::rename(&temp_path, &self.path))
    }

    /// Clears the recovery cache: the store's in-memory map immediately, the
    /// persisted file and interrupted-write temp file best-effort. This is
    /// the owned clear operation for the v0.7 "Provider cache" control.
    ///
    /// Idempotent by construction — a missing cache, an empty cache, and a
    /// cache file that is corrupt all succeed, and none of them is an error.
    /// Only this store's own file is touched: nothing outside the store path
    /// is read, removed, or renamed.
    ///
    /// Returns whether anything was actually held (entries or a file), which
    /// the caller reports as "removed" versus "already empty". The runtime's
    /// current in-session snapshot is deliberately untouched: this clears
    /// what a cold start would hydrate, so the next successful live refresh
    /// repopulates the cache normally instead of resurrecting old entries.
    pub fn clear(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        let had_entries = !state.is_empty();
        state.clear();
        let had_file = self.path.exists();
        let _ = fs::remove_file(&self.path);
        let temp_path = self.path.with_extension("json.tmp");
        let had_temp = temp_path.exists();
        if had_temp {
            let _ = fs::remove_file(&temp_path);
        }
        had_entries || had_file || had_temp
    }

    /// Hydrates safe cached ProviderUsageDto entries for known registered providers.
    ///
    /// Filtering rules:
    /// - Unknown providers not in `specs` are dropped.
    /// - Providers older than [`MAX_CACHE_AGE_SECS`] (or implausibly in the future) are discarded.
    /// - Windows whose `reset_at` is already in the past are omitted.
    /// - If a provider has no surviving windows, it is dropped.
    /// - Hydrated state carries the canonical `health: Stale` (v0.6 runtime
    ///   status contract) with `data_freshness: Some("stale")`; the legacy
    ///   `status` string is derived from the same health value, never set
    ///   independently.
    pub fn hydrate(
        &self,
        specs: &[ProviderSpec],
        now: DateTime<Utc>,
    ) -> (Vec<ProviderUsageDto>, HashMap<&'static str, ProviderUsageDto>) {
        let state = self.state.lock().unwrap();
        let mut usages = Vec::new();
        let mut last_good = HashMap::new();

        for spec in specs {
            let kind = spec.kind;
            let Some(persisted) = state.get(kind.id()) else {
                continue;
            };

            // 1. Check max cache age and clock skew
            let Ok(success_time) = DateTime::parse_from_rfc3339(&persisted.last_successful_at) else {
                continue;
            };
            let success_utc = success_time.with_timezone(&Utc);
            let age_secs = (now - success_utc).num_seconds();
            if age_secs > MAX_CACHE_AGE_SECS || age_secs < -FUTURE_TOLERANCE_SECS {
                continue;
            }

            // 2. Filter quota windows whose resetAt is already in the past
            let mut valid_limits = Vec::new();
            for limit in &persisted.limits {
                if limit.label.trim().is_empty() || !limit.used_percent.is_finite() {
                    continue;
                }
                if let Some(reset_str) = &limit.reset_at {
                    if let Ok(reset_dt) = DateTime::parse_from_rfc3339(reset_str) {
                        if reset_dt.with_timezone(&Utc) <= now {
                            // Reset is in the past: omit window for safety
                            continue;
                        }
                        // Lane B trust gate: a stored bound that fails the
                        // reset plausibility table (e.g. persisted by an
                        // older version) must not re-enter the runtime —
                        // the same drop rule as past/unparseable bounds.
                        if !crate::reset_plausibility::plausible_reset_at(
                            &limit.label,
                            reset_str,
                            now.timestamp_millis(),
                        ) {
                            continue;
                        }
                    } else {
                        // Unparseable reset stamp: malformed window dropped
                        continue;
                    }
                }
                valid_limits.push(UsageLimitDto {
                    label: limit.label.clone(),
                    used_percent: limit.used_percent.clamp(0.0, 100.0),
                    reset_at: limit.reset_at.clone(),
                });
            }

            // If no limits survived, omit the provider
            if valid_limits.is_empty() {
                continue;
            }

            let dto = ProviderUsageDto {
                id: kind.id().to_string(),
                name: persisted.provider_name.clone(),
                // v0.6 runtime status contract: `health` is the canonical
                // state; the legacy `status` string is derived from the same
                // value so the two can never disagree.
                health: ProviderHealth::Stale,
                status: ProviderHealth::Stale.legacy_status(),
                checked_at: persisted.last_successful_at.clone(),
                limits: valid_limits,
                account: persisted.account.as_ref().map(|acc| AccountAttributionDto {
                    label: acc.label.clone(),
                    note: acc.note.clone(),
                    identity: acc.identity.clone(),
                }),
                plan_type: None,
                error: None,
                error_category: None,
                error_http_status: None,
                source_updated_at: persisted.source_updated_at.clone().or_else(|| Some(persisted.last_successful_at.clone())),
                data_freshness: Some("stale"),
                // v0.7 banked credits never hydrate: a credit count is only
                // ever a fresh live observation, so a cold start always
                // re-observes before the capability reads as available.
                reset_credits: None,
            zcode_reset_cards: None,
                fallback_failure: None,
            };

            usages.push(dto.clone());
            last_good.insert(kind.id(), dto);
        }

        (usages, last_good)
    }

    /// Records genuine successful provider data into the persisted cache.
    ///
    /// Write policy (canonical health, v0.6 runtime status contract):
    /// - Must be `health == Live` (the legacy `status == "ok"` projection
    ///   of the same state)
    /// - Must NOT be stale projection (`data_freshness != Some("stale")`)
    /// - Must have non-empty valid limits
    /// - Errors, cooldown skips, and unavailable states are never recorded
    pub fn record_success(&self, kind: ProviderKind, dto: &ProviderUsageDto) {
        if dto.health != ProviderHealth::Live
            || dto.data_freshness == Some("stale")
            || dto.limits.is_empty()
        {
            return;
        }
        for limit in &dto.limits {
            if limit.label.trim().is_empty() || !limit.used_percent.is_finite() {
                return;
            }
        }
        let now_iso = (self.now)().to_rfc3339_opts(SecondsFormat::Millis, true);
        let state = PersistedProviderState {
            provider_id: kind.id().to_string(),
            provider_name: dto.name.clone(),
            account: dto.account.as_ref().map(|acc| PersistedAccountAttribution {
                label: acc.label.clone(),
                note: acc.note.clone(),
                identity: acc.identity.clone(),
            }),
            limits: dto.limits.iter().map(|l| PersistedQuotaLimit {
                label: l.label.clone(),
                used_percent: l.used_percent.clamp(0.0, 100.0),
                reset_at: l.reset_at.clone(),
            }).collect(),
            last_successful_at: now_iso,
            source_updated_at: dto.source_updated_at.clone(),
        };

        let mut lock = self.state.lock().unwrap();
        lock.insert(kind.id().to_string(), state);
        let _ = self.persist_locked(&lock);
    }

    #[cfg(test)]
    pub fn get_persisted(&self, provider_id: &str) -> Option<PersistedProviderState> {
        self.state.lock().unwrap().get(provider_id).cloned()
    }
}

fn parse_envelope(raw: &str) -> (HashMap<String, PersistedProviderState>, bool) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (HashMap::new(), false);
    };
    let Some(object) = parsed.as_object() else {
        return (HashMap::new(), false);
    };
    if object.get("version").and_then(serde_json::Value::as_u64) != Some(SCHEMA_VERSION as u64) {
        return (HashMap::new(), false);
    }
    let Some(providers_val) = object.get("providers").and_then(serde_json::Value::as_array) else {
        return (HashMap::new(), false);
    };
    let mut map = HashMap::new();
    let mut healthy = true;
    for entry in providers_val {
        if let Ok(state) = serde_json::from_value::<PersistedProviderState>(entry.clone()) {
            if is_valid_persisted_state(&state) {
                map.insert(state.provider_id.clone(), state);
            } else if let Some(sanitized) = sanitize_provider_state(&state) {
                map.insert(sanitized.provider_id.clone(), sanitized);
                healthy = false;
            } else {
                healthy = false;
            }
        } else {
            healthy = false;
        }
    }
    (map, healthy)
}

fn is_valid_persisted_state(state: &PersistedProviderState) -> bool {
    if state.provider_id.trim().is_empty() || state.provider_name.trim().is_empty() {
        return false;
    }
    if DateTime::parse_from_rfc3339(&state.last_successful_at).is_err() {
        return false;
    }
    if let Some(src) = &state.source_updated_at {
        if DateTime::parse_from_rfc3339(src).is_err() {
            return false;
        }
    }
    if state.limits.is_empty() {
        return false;
    }
    for limit in &state.limits {
        if limit.label.trim().is_empty() || !limit.used_percent.is_finite() || !(0.0..=100.0).contains(&limit.used_percent) {
            return false;
        }
        if let Some(reset_str) = &limit.reset_at {
            if DateTime::parse_from_rfc3339(reset_str).is_err() {
                return false;
            }
        }
    }
    true
}

fn sanitize_provider_state(state: &PersistedProviderState) -> Option<PersistedProviderState> {
    if state.provider_id.trim().is_empty() || state.provider_name.trim().is_empty() {
        return None;
    }
    if DateTime::parse_from_rfc3339(&state.last_successful_at).is_err() {
        return None;
    }
    let mut valid_limits = Vec::new();
    for limit in &state.limits {
        if limit.label.trim().is_empty() || !limit.used_percent.is_finite() || !(0.0..=100.0).contains(&limit.used_percent) {
            continue;
        }
        if let Some(reset_str) = &limit.reset_at {
            if DateTime::parse_from_rfc3339(reset_str).is_err() {
                continue;
            }
        }
        valid_limits.push(limit.clone());
    }
    if valid_limits.is_empty() {
        return None;
    }
    let mut sanitized = state.clone();
    sanitized.limits = valid_limits;
    Some(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::history::QuotaHistoryStore;
    use crate::notifications::NotificationLane;
    use crate::runtime::{ProviderFailure, ProviderSpec, RuntimeCore};

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
                "rate-limits-last-good-test-{}-{}-{}",
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
            error: None,
            error_category: None,
            error_http_status: None,
            source_updated_at: None,
            data_freshness: None,
            reset_credits: None,
            zcode_reset_cards: None,
            fallback_failure: None,
        }
    }

    // 1. successful provider state persists
    #[test]
    fn successful_provider_state_persists() {
        let dir = tempdir::temp_dir("success-persists");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));

        let usage = sample_usage(ProviderKind::Codex, 42.0, Some("2026-09-29T15:00:00Z"));
        store.record_success(ProviderKind::Codex, &usage);

        let raw = fs::read_to_string(&path).expect("file should exist");
        assert!(raw.contains("openai-codex"));
        assert!(raw.contains("42"));
        assert!(raw.contains("key:1234"));

        let persisted = store.get_persisted("openai-codex").expect("should be stored in memory");
        assert_eq!(persisted.limits[0].used_percent, 42.0);
    }

    // 2. cold start hydrates persisted state
    #[test]
    fn cold_start_hydrates_persisted_state() {
        let dir = tempdir::temp_dir("cold-start");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 50.0, Some("2026-09-29T16:00:00Z")));
        store.record_success(ProviderKind::Zai, &sample_usage(ProviderKind::Zai, 30.0, Some("2026-09-29T16:00:00Z")));

        let specs = vec![
            mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None))),
            mock_spec(ProviderKind::Zai, Ok(sample_usage(ProviderKind::Zai, 30.0, None))),
        ];

        let core = RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        ).with_last_good_store(Some(store));

        let snapshot = core.snapshot();
        assert_eq!(snapshot.providers.len(), 2, "hydrates both providers on cold start");
        assert_eq!(snapshot.providers[0].id, "openai-codex");
        assert_eq!(snapshot.providers[1].id, "zai");
    }

    // 3. hydrated state is stale/cached
    #[test]
    fn hydrated_state_is_stale_cached() {
        let dir = tempdir::temp_dir("stale-cached");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 50.0, Some("2026-09-29T16:00:00Z")));

        let specs = vec![
            mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None))),
        ];

        let core = RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        ).with_last_good_store(Some(store));

        let snapshot = core.snapshot();
        let provider = &snapshot.providers[0];
        assert_eq!(provider.health, ProviderHealth::Stale, "hydrated state must be canonically stale");
        assert_eq!(provider.status, "stale", "hydrated state must be visibly stale");
        assert_eq!(provider.data_freshness, Some("stale"));
        assert!(provider.error.is_none());
        assert_eq!(provider.checked_at, "2026-09-29T12:00:00.000Z");
    }

    #[test]
    fn hydrated_state_never_carries_reset_credits() {
        // v0.7 banked credits are fresh-live-observation-only: even a DTO
        // that held a current balance persists and hydrates without it, so a
        // cold start always re-observes before the capability is available.
        use crate::codex::{CodexResetCredits, RESET_CREDITS_SOURCE};
        let dir = tempdir::temp_dir("no-credit-hydration");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        let mut live = sample_usage(ProviderKind::Codex, 50.0, Some("2026-09-29T16:00:00Z"));
        live.reset_credits = Some(CodexResetCredits {
            banked_credits: 3,
            currently_applicable: Some(0),
            checked_at: "2026-09-29T12:00:00.000Z".to_string(),
            source: RESET_CREDITS_SOURCE,
        });
        store.record_success(ProviderKind::Codex, &live);

        let specs = vec![mock_spec(
            ProviderKind::Codex,
            Ok(sample_usage(ProviderKind::Codex, 50.0, None)),
        )];

        let core = RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        )
        .with_last_good_store(Some(store));

        let snapshot = core.snapshot();
        let provider = &snapshot.providers[0];
        assert_eq!(provider.health, ProviderHealth::Stale);
        assert_eq!(
            provider.reset_credits, None,
            "credits must be re-observed live, never hydrated"
        );
    }

    #[test]
    fn hydrated_state_never_carries_zcode_reset_cards() {
        // ZCode reset cards are fresh-live-observation-only: even a DTO that
        // held cards persists and hydrates without them, so a cold start
        // always re-observes before a grant reads as available.
        use crate::zcode_reset::{ZCodeResetCard, ZCodeResetStatus, ZCodeResetTarget};
        let dir = tempdir::temp_dir("no-card-hydration");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        let mut live = sample_usage(ProviderKind::Zai, 50.0, Some("2026-09-29T16:00:00Z"));
        live.zcode_reset_cards = Some(ZCodeResetStatus {
            five_hour_cards: vec![ZCodeResetCard {
                target: ZCodeResetTarget::FiveHour,
                expires_at: None,
            }],
            weekly_cards: vec![],
            observed_at: fixed_now(),
        });
        store.record_success(ProviderKind::Zai, &live);

        let specs = vec![mock_spec(
            ProviderKind::Zai,
            Ok(sample_usage(ProviderKind::Zai, 50.0, None)),
        )];

        let core = RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        )
        .with_last_good_store(Some(store));

        let snapshot = core.snapshot();
        let provider = &snapshot.providers[0];
        assert_eq!(provider.health, ProviderHealth::Stale);
        assert_eq!(
            provider.zcode_reset_cards, None,
            "cards must be re-observed live, never hydrated"
        );
    }

    // 4. hydration creates no history observation
    #[test]
    fn hydration_creates_no_history_observation() {
        let dir = tempdir::temp_dir("history-no-write");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 50.0, Some("2026-09-29T16:00:00Z")));

        let history_path = dir.path().join("quota-history-v1.json");
        let history = Arc::new(QuotaHistoryStore::open(history_path));

        let specs = vec![
            mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None))),
        ];

        let core = RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        )
        .with_last_good_store(Some(store))
        .with_history_store(Some(history.clone()));

        assert!(history.history().is_empty(), "history must have zero observations after hydration");
        let observations = crate::history::observations_from_usages(&core.snapshot().providers);
        assert!(observations.is_empty(), "hydrated stale usages produce zero observations");
    }

    // 5. hydration does not increment history revision
    #[test]
    fn hydration_does_not_increment_history_revision() {
        let dir = tempdir::temp_dir("history-revision");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 50.0, Some("2026-09-29T16:00:00Z")));

        let history_path = dir.path().join("quota-history-v1.json");
        let history = Arc::new(QuotaHistoryStore::open(history_path));

        let specs = vec![
            mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None))),
        ];

        let core = RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        )
        .with_last_good_store(Some(store))
        .with_history_store(Some(history.clone()));

        assert_eq!(history.revision(), 0);
        assert_eq!(core.snapshot().history_revision, 0);
    }

    // 6. hydration does not trigger notifications
    #[test]
    fn hydration_does_not_trigger_notifications() {
        let dir = tempdir::temp_dir("notifications-silent");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        // 98% would normally trigger a critical notification if live
        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 98.0, Some("2026-09-29T16:00:00Z")));

        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let lane = Arc::new(NotificationLane::open(
            dir.path().join("quota-notifications-v1.json"),
            Box::new(move |notes| {
                sink.lock().unwrap().extend_from_slice(notes);
            }),
        ));
        lane.set_enabled(true);

        let specs = vec![
            mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 98.0, None))),
        ];

        let core = RuntimeCore::with_injections(
            specs,
            5,
            Box::new(|| 0),
            Box::new(fixed_now),
        )
        .with_last_good_store(Some(store))
        .with_notification_lane(Some(lane.clone()));

        let notes = lane.process(&core.snapshot().providers, fixed_now());
        assert!(notes.is_empty(), "hydration must never fire notifications");
        assert!(delivered.lock().unwrap().is_empty());
    }

    // 7. live success replaces cached provider
    #[tokio::test]
    async fn live_success_replaces_cached_provider() {
        let dir = tempdir::temp_dir("live-replaces");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 40.0, Some("2026-09-29T16:00:00Z")));

        let live_dto = sample_usage(ProviderKind::Codex, 65.0, Some("2026-09-29T16:00:00Z"));
        let specs = vec![mock_spec(ProviderKind::Codex, Ok(live_dto))];

        let core = Arc::new(
            RuntimeCore::with_injections(
                specs,
                5,
                Box::new(|| 0),
                Box::new(fixed_now),
            ).with_last_good_store(Some(store.clone())),
        );

        assert_eq!(core.snapshot().providers[0].health, ProviderHealth::Stale);
        assert_eq!(core.snapshot().providers[0].status, "stale");
        assert_eq!(core.snapshot().providers[0].limits[0].used_percent, 40.0);

        core.run_cycle().await;

        let snapshot = core.snapshot();
        assert_eq!(snapshot.providers[0].health, ProviderHealth::Live);
        assert_eq!(snapshot.providers[0].status, "ok", "live success replaces cached state with ok");
        assert_eq!(snapshot.providers[0].limits[0].used_percent, 65.0);

        let persisted = store.get_persisted("openai-codex").unwrap();
        assert_eq!(persisted.limits[0].used_percent, 65.0, "persisted store is updated with live data");
    }

    // 8. provider error does not erase cached last-good
    #[tokio::test]
    async fn provider_error_does_not_erase_cached_last_good() {
        let dir = tempdir::temp_dir("error-retains");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 42.0, Some("2026-09-29T16:00:00Z")));

        let failure = ProviderFailure {
            code: "offline".to_string(),
            message: "No internet".to_string(),
            http_status: None,
            transient: Some(false),
            retry_after_ms: None,
            identity: None,
            transport_timeout: false,
        };
        let specs = vec![mock_spec(ProviderKind::Codex, Err(failure))];

        let core = Arc::new(
            RuntimeCore::with_injections(
                specs,
                5,
                Box::new(|| 0),
                Box::new(fixed_now),
            ).with_last_good_store(Some(store.clone())),
        );

        core.run_cycle().await;

        let snapshot = core.snapshot();
        assert_eq!(snapshot.providers[0].health, ProviderHealth::Error);
        assert_eq!(snapshot.providers[0].status, "error");
        assert_eq!(snapshot.providers[0].limits[0].used_percent, 42.0, "retains last good limits in memory");

        let persisted = store.get_persisted("openai-codex").expect("persisted file must not be erased");
        assert_eq!(persisted.limits[0].used_percent, 42.0, "persisted file retains good data across error");
    }

    // 9. cooldown skip does not overwrite cache
    #[tokio::test]
    async fn cooldown_skip_does_not_overwrite_cache() {
        let dir = tempdir::temp_dir("cooldown-retains");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 70.0, Some("2026-09-29T16:00:00Z")));

        // Server sends 429 with Retry-After 60s
        let failure = ProviderFailure {
            code: "rate_limit".to_string(),
            message: "Too Many Requests".to_string(),
            http_status: Some(429),
            transient: Some(false),
            retry_after_ms: Some(60_000),
            identity: None,
            transport_timeout: false,
        };
        let specs = vec![mock_spec(ProviderKind::Codex, Err(failure))];

        let core = Arc::new(
            RuntimeCore::with_injections(
                specs,
                5,
                Box::new(|| 0),
                Box::new(fixed_now),
            ).with_last_good_store(Some(store.clone())),
        );

        core.run_cycle().await;
        // Second cycle: provider is on active cooldown and skipped
        core.run_cycle().await;

        let persisted = store.get_persisted("openai-codex").unwrap();
        assert_eq!(persisted.limits[0].used_percent, 70.0, "cooldown skip did not overwrite cache");
    }

    // 10. account A/B isolation preserved
    #[tokio::test]
    async fn account_a_b_isolation_preserved() {
        let dir = tempdir::temp_dir("account-isolation");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        let mut usage_a = sample_usage(ProviderKind::Codex, 40.0, Some("2026-09-29T16:00:00Z"));
        usage_a.account = Some(AccountAttributionDto {
            label: "Account A".to_string(),
            note: None,
            identity: Some("key:AAAA".to_string()),
        });
        store.record_success(ProviderKind::Codex, &usage_a);

        let mut usage_b = sample_usage(ProviderKind::Codex, 80.0, Some("2026-09-29T16:00:00Z"));
        usage_b.account = Some(AccountAttributionDto {
            label: "Account B".to_string(),
            note: None,
            identity: Some("key:BBBB".to_string()),
        });

        let specs = vec![mock_spec(ProviderKind::Codex, Ok(usage_b))];
        let core = Arc::new(
            RuntimeCore::with_injections(
                specs,
                5,
                Box::new(|| 0),
                Box::new(fixed_now),
            ).with_last_good_store(Some(store.clone())),
        );

        core.run_cycle().await;

        let snapshot = core.snapshot();
        let acc = snapshot.providers[0].account.as_ref().unwrap();
        assert_eq!(acc.identity.as_deref(), Some("key:BBBB"));
        assert_eq!(snapshot.providers[0].limits[0].used_percent, 80.0);

        let persisted = store.get_persisted("openai-codex").unwrap();
        assert_eq!(persisted.account.unwrap().identity.as_deref(), Some("key:BBBB"));
    }

    // 11. unattributed cache does not attach to proven account
    #[tokio::test]
    async fn unattributed_cache_does_not_attach_to_proven_account() {
        let dir = tempdir::temp_dir("unattributed-cache");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        let mut usage_unattributed = sample_usage(ProviderKind::Codex, 25.0, Some("2026-09-29T16:00:00Z"));
        usage_unattributed.account = None;
        store.record_success(ProviderKind::Codex, &usage_unattributed);

        let mut usage_proven = sample_usage(ProviderKind::Codex, 75.0, Some("2026-09-29T16:00:00Z"));
        usage_proven.account = Some(AccountAttributionDto {
            label: "Proven User".to_string(),
            note: None,
            identity: Some("key:PROVEN".to_string()),
        });

        let specs = vec![mock_spec(ProviderKind::Codex, Ok(usage_proven))];
        let core = Arc::new(
            RuntimeCore::with_injections(
                specs,
                5,
                Box::new(|| 0),
                Box::new(fixed_now),
            ).with_last_good_store(Some(store.clone())),
        );

        // Before cycle: snapshot has unattributed cache
        assert!(core.snapshot().providers[0].account.is_none());

        core.run_cycle().await;

        // After cycle: replaced by proven account; unattributed cache did not attach
        let snapshot = core.snapshot();
        assert_eq!(snapshot.providers[0].account.as_ref().unwrap().identity.as_deref(), Some("key:PROVEN"));
        assert_eq!(snapshot.providers[0].limits[0].used_percent, 75.0);
    }

    // 12. expired file entry discarded
    #[test]
    fn expired_file_entry_discarded() {
        let dir = tempdir::temp_dir("expired-discarded");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);

        // 8 days ago relative to fixed_now (2026-09-29T12:00:00Z) -> 2026-09-21T12:00:00Z
        let old_time = "2026-09-21T11:00:00Z";
        let envelope = serde_json::json!({
            "version": 1,
            "providers": [{
                "providerId": "openai-codex",
                "providerName": "OpenAI / Codex",
                "limits": [{ "label": "5-hour limit", "usedPercent": 50.0 }],
                "lastSuccessfulAt": old_time
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));
        let specs = vec![mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None)))];

        let (usages, last_good) = store.hydrate(&specs, fixed_now());
        assert!(usages.is_empty(), "entry older than 7 days must be discarded");
        assert!(last_good.is_empty());
    }

    // 13. resetAt already in past discarded
    #[test]
    fn reset_at_already_in_past_discarded() {
        let dir = tempdir::temp_dir("past-reset");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);

        // Window 1 reset was 2 hours ago (past); Window 2 reset is in 2 hours (future)
        let envelope = serde_json::json!({
            "version": 1,
            "providers": [{
                "providerId": "openai-codex",
                "providerName": "OpenAI / Codex",
                "limits": [
                    { "label": "Expired Window", "usedPercent": 95.0, "resetAt": "2026-09-29T10:00:00Z" },
                    { "label": "Active Window", "usedPercent": 40.0, "resetAt": "2026-09-29T14:00:00Z" }
                ],
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));
        let specs = vec![mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None)))];

        let (usages, _) = store.hydrate(&specs, fixed_now());
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].limits.len(), 1, "past window omitted");
        assert_eq!(usages[0].limits[0].label, "Active Window");
    }

    // 14. corrupt file self-heals safely
    #[test]
    fn corrupt_file_self_heals_safely() {
        let dir = tempdir::temp_dir("corrupt-heal");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        fs::write(&path, "{not valid json at all").unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        assert!(store.get_persisted("openai-codex").is_none());

        // Saving a valid entry heals the file
        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 33.0, Some("2026-09-29T16:00:00Z")));

        let raw = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["providers"].as_array().unwrap().len(), 1);
    }

    // 15. foreign schema version self-heals safely
    #[test]
    fn foreign_schema_version_self_heals_safely() {
        let dir = tempdir::temp_dir("foreign-version");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": 999,
            "providers": [{
                "providerId": "openai-codex",
                "providerName": "OpenAI / Codex",
                "limits": [{ "label": "5-hour limit", "usedPercent": 50.0 }],
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        assert!(store.get_persisted("openai-codex").is_none(), "foreign version must degrade to empty");
    }

    // 16. unknown provider dropped
    #[test]
    fn unknown_provider_dropped() {
        let dir = tempdir::temp_dir("unknown-provider");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": 1,
            "providers": [
                {
                    "providerId": "openai-codex",
                    "providerName": "OpenAI / Codex",
                    "limits": [{ "label": "5h", "usedPercent": 50.0, "resetAt": "2026-09-29T14:00:00Z" }],
                    "lastSuccessfulAt": "2026-09-29T11:00:00Z"
                },
                {
                    "providerId": "deprecated-bot",
                    "providerName": "Deprecated Bot",
                    "limits": [{ "label": "1d", "usedPercent": 20.0, "resetAt": "2026-09-29T14:00:00Z" }],
                    "lastSuccessfulAt": "2026-09-29T11:00:00Z"
                }
            ]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        let specs = vec![mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None)))];

        let (usages, _) = store.hydrate(&specs, fixed_now());
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].id, "openai-codex", "unknown provider was dropped");
    }

    // 17. malformed window dropped
    #[test]
    fn malformed_window_dropped() {
        let dir = tempdir::temp_dir("malformed-window");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": 1,
            "providers": [{
                "providerId": "openai-codex",
                "providerName": "OpenAI / Codex",
                "limits": [
                    { "label": "", "usedPercent": 50.0, "resetAt": "2026-09-29T14:00:00Z" },
                    { "label": "Valid", "usedPercent": 60.0, "resetAt": "2026-09-29T14:00:00Z" },
                    { "label": "Bad Reset", "usedPercent": 70.0, "resetAt": "not-a-date" }
                ],
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        let specs = vec![mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None)))];

        let (usages, _) = store.hydrate(&specs, fixed_now());
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].limits.len(), 1);
        assert_eq!(usages[0].limits[0].label, "Valid");
    }

    // 18. one bad provider does not drop all valid providers
    #[test]
    fn one_bad_provider_does_not_drop_all_valid_providers() {
        let dir = tempdir::temp_dir("one-bad-provider");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": 1,
            "providers": [
                {
                    "providerId": "",
                    "providerName": "Bad",
                    "limits": [],
                    "lastSuccessfulAt": "invalid-time"
                },
                {
                    "providerId": "zai",
                    "providerName": "Z.ai",
                    "limits": [{ "label": "Daily", "usedPercent": 35.0, "resetAt": "2026-09-29T14:00:00Z" }],
                    "lastSuccessfulAt": "2026-09-29T11:00:00Z"
                }
            ]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        let specs = vec![mock_spec(ProviderKind::Zai, Ok(sample_usage(ProviderKind::Zai, 35.0, None)))];

        let (usages, _) = store.hydrate(&specs, fixed_now());
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].id, "zai");
    }

    // 19. temp/rename persistence path works
    #[test]
    fn temp_rename_persistence_path_works() {
        let dir = tempdir::temp_dir("temp-rename");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));

        store.record_success(ProviderKind::Codex, &sample_usage(ProviderKind::Codex, 44.0, Some("2026-09-29T16:00:00Z")));

        assert!(path.exists(), "target file exists");
        assert!(!path.with_extension("json.tmp").exists(), "temp file was cleaned up by rename");

        let reopened = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        assert_eq!(reopened.get_persisted("openai-codex").unwrap().limits[0].used_percent, 44.0);
    }

    // 20. persisted schema contains no secret-bearing fields
    #[test]
    fn persisted_schema_contains_no_secret_bearing_fields() {
        let envelope = PersistedLastGoodEnvelope {
            version: SCHEMA_VERSION,
            providers: vec![PersistedProviderState {
                provider_id: "openai-codex".to_string(),
                provider_name: "OpenAI / Codex".to_string(),
                account: Some(PersistedAccountAttribution {
                    label: "Masked Key".to_string(),
                    note: Some("Account note".to_string()),
                    identity: Some("key:1234".to_string()),
                }),
                limits: vec![PersistedQuotaLimit {
                    label: "5-hour limit".to_string(),
                    used_percent: 50.0,
                    reset_at: Some("2026-09-29T15:00:00.000Z".to_string()),
                }],
                last_successful_at: "2026-09-29T12:00:00.000Z".to_string(),
                source_updated_at: None,
            }],
        };

        let json = serde_json::to_string_pretty(&envelope).unwrap();
        let forbidden = [
            "token",
            "secret",
            "bearer",
            "authorization",
            "cookie",
            "api_key",
            "apikey",
            "jwt",
        ];

        let lower = json.to_lowercase();
        for term in forbidden {
            assert!(
                !lower.contains(term),
                "persisted schema must not contain forbidden secret term: {term}"
            );
        }
    }

    // Lane B: hydration re-applies the reset plausibility check to stored
    // bounds, so a bound persisted by an older version cannot re-enter the
    // runtime. The implausible window is dropped whole (the same rule as a
    // past or unparseable bound); a plausible sibling window survives.
    #[test]
    fn hydrate_drops_windows_whose_stored_bound_fails_plausibility() {
        let dir = tempdir::temp_dir("hydrate-implausible-bound");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(
            path,
            Box::new(fixed_now),
        ));

        // "Weekly" claims a reset 30 days out — beyond the 15-day horizon.
        let mut implausible = sample_usage(ProviderKind::Codex, 50.0, Some("2026-10-29T12:00:00Z"));
        implausible.limits[0].label = "Weekly".to_string();
        store.record_success(ProviderKind::Codex, &implausible);
        // A sibling provider with a plausible bound (7 days out) hydrates.
        let plausible = sample_usage(ProviderKind::Zai, 30.0, Some("2026-10-06T12:00:00Z"));
        let plausible_usage = ProviderUsageDto {
            limits: vec![UsageLimitDto {
                label: "Weekly".to_string(),
                used_percent: plausible.limits[0].used_percent,
                reset_at: plausible.limits[0].reset_at.clone(),
            }],
            ..plausible
        };
        store.record_success(ProviderKind::Zai, &plausible_usage);

        let specs = vec![
            mock_spec(ProviderKind::Codex, Ok(sample_usage(ProviderKind::Codex, 50.0, None))),
            mock_spec(ProviderKind::Zai, Ok(sample_usage(ProviderKind::Zai, 30.0, None))),
        ];
        let core = RuntimeCore::with_injections(specs, 5, Box::new(|| 0), Box::new(fixed_now))
            .with_last_good_store(Some(store));

        let snapshot = core.snapshot();
        assert_eq!(
            snapshot.providers.len(),
            1,
            "the implausible window drops whole; the window-less provider is omitted"
        );
        assert_eq!(snapshot.providers[0].id, "zai", "only the plausible sibling hydrates");
        assert_eq!(snapshot.providers[0].health, ProviderHealth::Stale);
    }
}


