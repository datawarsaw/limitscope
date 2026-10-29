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
//! - Identity safety: Codex cache entries are retained only when their account identity is the current
//!   `chatgpt-v2:` form; legacy identities are rejected at load, sanitization, and write time.
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
    is_codex_provider_id, AccountAttributionDto, ApiWalletDto, ProviderHealth, ProviderKind, ProviderSpec,
    ProviderUsageDto, UsageLimitDto, WalletBalanceDto, WalletHealth, WalletSpec,
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

/// One persisted prepaid balance. Kept in its own list: no limits, no
/// percentages, and a malformed wallet entry never invalidates providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedWalletState {
    pub wallet_id: String,
    pub wallet_name: String,
    /// The wallet's balances, one entry per currency, in source order.
    /// Currencies are never merged, summed or converted.
    ///
    /// `None` means the entry predates the currency-aware schema and
    /// carries only the legacy USD field; `Some(vec![])` is an explicit
    /// statement that the wallet holds no balance. The two are different
    /// facts and are never conflated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balances: Option<Vec<PersistedWalletBalance>>,
    /// The source's own availability verdict, when it reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    /// The single USD amount, when the wallet holds exactly one USD balance.
    /// Absent for a wallet in another currency or a multi-currency wallet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Deterministic full-credential fingerprint that produced the balance.
    /// Absent on entries written before the P1 isolation fix, and therefore
    /// unverifiable: hydration drops them rather than presenting another
    /// account's money.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_binding: Option<String>,
    pub last_successful_at: String,
}

/// One persisted currency balance inside a wallet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedWalletBalance {
    /// ISO currency code exactly as the source reported it.
    pub currency: String,
    /// The remaining amount in `currency`.
    pub amount: f64,
    /// The source's own breakdown of `amount`, when it reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted: Option<f64>,
    /// The topped-up part of `amount`, when the source reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topped_up: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedLastGoodEnvelope {
    pub version: u32,
    pub providers: Vec<PersistedProviderState>,
    /// Optional for schema v1 compatibility: an envelope written before
    /// wallets existed loads with an empty wallet list. The schema version
    /// stays 1 because the provider payload is unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wallets: Vec<PersistedWalletState>,
}

pub struct ProviderLastGoodStore {
    path: PathBuf,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    state: Mutex<HashMap<String, PersistedProviderState>>,
    wallets: Mutex<HashMap<String, PersistedWalletState>>,
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
            wallets: Mutex::new(HashMap::new()),
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
        let (providers, wallets, healthy) = parse_envelope(&raw);
        let mut state = self.state.lock().unwrap();
        *state = providers;
        let mut wallet_state = self.wallets.lock().unwrap();
        *wallet_state = wallets;
        if !healthy {
            let _ = self.persist_locked(&state, &wallet_state);
        }
    }

    #[allow(dead_code)]
    pub fn persist(&self) -> Result<(), std::io::Error> {
        let state = self.state.lock().unwrap();
        let wallets = self.wallets.lock().unwrap();
        self.persist_locked(&state, &wallets)
    }

    fn persist_locked(
        &self,
        state: &HashMap<String, PersistedProviderState>,
        wallets: &HashMap<String, PersistedWalletState>,
    ) -> Result<(), std::io::Error> {
        let mut providers: Vec<PersistedProviderState> = state.values().cloned().collect();
        providers.sort_by(|a, b| a.provider_id.cmp(&b.provider_id));
        let mut wallet_entries: Vec<PersistedWalletState> = wallets.values().cloned().collect();
        wallet_entries.sort_by(|a, b| a.wallet_id.cmp(&b.wallet_id));
        let envelope = PersistedLastGoodEnvelope {
            version: SCHEMA_VERSION,
            providers,
            wallets: wallet_entries,
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
        let mut wallets = self.wallets.lock().unwrap();
        let had_entries = !state.is_empty();
        state.clear();
        let had_wallets = !wallets.is_empty();
        wallets.clear();
        let had_file = self.path.exists();
        let _ = fs::remove_file(&self.path);
        let temp_path = self.path.with_extension("json.tmp");
        let had_temp = temp_path.exists();
        if had_temp {
            let _ = fs::remove_file(&temp_path);
        }
        had_entries || had_wallets || had_file || had_temp
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
            if kind.is_manual_only() {
                continue;
            }
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
                    reset_text: None,
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
            zcode_plans: None,
            grok_bot: None,
            account_alias: None,
            monitoring_paused: None,
                fallback_failure: None,
            };

            usages.push(dto.clone());
            last_good.insert(kind.id(), dto);
        }

        (usages, last_good)
    }

    /// Hydrates persisted wallet balances for registered wallet specs. A
    /// hydrated balance is always `stale`: it was observed before this
    /// process started and cannot be presented as live. Unknown wallet ids,
    /// expired entries, and malformed amounts are dropped independently of
    /// provider hydration.
    ///
    /// Identity is a hard gate: a persisted entry is surfaced only when both
    /// the entry and the currently stored credential carry a binding and the
    /// two are exactly equal. Missing, blank, or mismatched bindings are
    /// rejected and removed from the in-memory wallet map. Eviction from the
    /// file is best effort, but it is not the security guarantee: every
    /// hydration revalidates, so an entry that survives a failed rewrite is
    /// still rejected on the next restart.
    pub fn hydrate_wallets(
        &self,
        specs: &[WalletSpec],
        now: DateTime<Utc>,
    ) -> (Vec<ApiWalletDto>, HashMap<&'static str, ApiWalletDto>) {
        let mut state = self.wallets.lock().unwrap();
        let mut wallets = Vec::new();
        let mut last_good = HashMap::new();
        let mut rejected = false;
        for spec in specs {
            let Some(persisted) = state.get(spec.id).cloned() else {
                continue;
            };
            let Ok(success_time) = DateTime::parse_from_rfc3339(&persisted.last_successful_at)
            else {
                continue;
            };
            let age_secs = (now - success_time.with_timezone(&Utc)).num_seconds();
            if age_secs > MAX_CACHE_AGE_SECS || age_secs < -FUTURE_TOLERANCE_SECS {
                continue;
            }
            if !persisted_balances_are_valid(&persisted) {
                continue;
            }
            // Fail closed: the balance belongs to a credential, and only an
            // exact match with the credential currently on disk proves that
            // it is still the same source.
            let current = (spec.current_credential_binding)();
            if !crate::wallet::credential_bindings_match(
                persisted.credential_binding.as_deref(),
                current.as_deref(),
            ) {
                state.remove(spec.id);
                rejected = true;
                continue;
            }
            let dto = ApiWalletDto {
                id: spec.id.to_string(),
                name: if persisted.wallet_name.trim().is_empty() {
                    spec.name.to_string()
                } else {
                    persisted.wallet_name.clone()
                },
                health: WalletHealth::Stale,
                checked_at: Some(persisted.last_successful_at.clone()),
                balances: effective_persisted_balances(&persisted)
                    .iter()
                    .map(|balance| WalletBalanceDto {
                        currency: balance.currency.clone(),
                        amount: balance.amount,
                        granted: balance.granted,
                        topped_up: balance.topped_up,
                    })
                    .collect(),
                available: persisted.available,
                remaining_usd: persisted.remaining_usd,
                identity: persisted.identity.clone(),
                credential_binding: persisted.credential_binding.clone(),
                error: None,
                error_category: None,
                error_http_status: None,
            };
            wallets.push(dto.clone());
            last_good.insert(spec.id, dto);
        }
        drop(state);
        if rejected {
            let providers = self.state.lock().unwrap();
            let wallets = self.wallets.lock().unwrap();
            let _ = self.persist_locked(&providers, &wallets);
        }
        (wallets, last_good)
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
        if kind.is_manual_only() {
            return;
        }
        if !provider_identity_is_current(
            kind.id(),
            dto.account
                .as_ref()
                .and_then(|account| account.identity.as_deref()),
        ) {
            return;
        }
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
        let wallets = self.wallets.lock().unwrap();
        lock.insert(kind.id().to_string(), state);
        let _ = self.persist_locked(&lock, &wallets);
    }

    /// Drops one provider's cached entry.
    ///
    /// The runtime calls this when a Codex slot's observed identity moves:
    /// the cached reading belongs to the previous session's user, so it must
    /// not be restored under the new one. The entry is removed from memory
    /// and from disk in the same step, so a restart cannot resurrect it
    /// either. Providers whose identity did not move keep their entry.
    pub fn evict_provider(&self, provider_id: &str) {
        let mut lock = self.state.lock().unwrap();
        lock.remove(provider_id);
        // The file is rewritten even when the in-memory map held no entry:
        // the persisted file can be out of step with the memory (a failed
        // write, a repair at load time), and a foreign reading must not
        // survive on disk either.
        let wallets = self.wallets.lock().unwrap();
        let _ = self.persist_locked(&lock, &wallets);
    }

    /// Records a genuine live wallet observation. An error, a cooldown skip,
    /// or a hydrated stale entry never overwrites the cached balance.
    pub fn record_wallet_success(&self, dto: &ApiWalletDto) {
        if dto.health != WalletHealth::Live {
            return;
        }
        if dto.balances.is_empty() {
            return;
        }
        if !dto.balances.iter().all(|balance| {
            !balance.currency.trim().is_empty()
                && balance.amount.is_finite()
                && balance.amount >= 0.0
        }) {
            return;
        }
        // The USD convenience amount must agree with the persisted balances:
        // it is derived from them, never an independent fact.
        let derived_usd = if dto.balances.len() == 1 && dto.balances[0].currency == "USD" {
            Some(dto.balances[0].amount)
        } else {
            None
        };
        if derived_usd != dto.remaining_usd {
            return;
        }
        // An observation without a credential binding cannot be proven to
        // belong to any account. Refuse to persist it: the cache would be
        // unverifiable at the next cold start.
        let Some(credential_binding) = dto.credential_binding.clone() else {
            return;
        };
        if credential_binding.trim().is_empty() {
            return;
        }
        let now_iso = (self.now)().to_rfc3339_opts(SecondsFormat::Millis, true);
        let state = PersistedWalletState {
            wallet_id: dto.id.clone(),
            wallet_name: dto.name.clone(),
            balances: dto
                .balances
                .iter()
                .map(|balance| PersistedWalletBalance {
                    currency: balance.currency.clone(),
                    amount: balance.amount,
                    granted: balance.granted,
                    topped_up: balance.topped_up,
                })
                .collect::<Vec<_>>()
                .into(),
            available: dto.available,
            remaining_usd: dto.remaining_usd,
            identity: dto.identity.clone(),
            credential_binding: Some(credential_binding),
            last_successful_at: now_iso,
        };
        let providers = self.state.lock().unwrap();
        let mut wallets = self.wallets.lock().unwrap();
        wallets.insert(dto.id.clone(), state);
        let _ = self.persist_locked(&providers, &wallets);
    }

    /// Removes one wallet's persisted last-good balance and rewrites the
    /// envelope. Used when the runtime observes a credential change.
    ///
    /// Best effort by design: if the rewrite fails, the old entry can stay on
    /// disk, but hydration compares it with the then-current credential and
    /// rejects it, so it still cannot surface after a restart.
    pub fn discard_wallet(&self, wallet_id: &str) {
        let providers = self.state.lock().unwrap();
        let mut wallets = self.wallets.lock().unwrap();
        wallets.remove(wallet_id);
        // Rewrite even when the in-memory map no longer holds the entry: the
        // disk copy is the one that can come back after a restart, and a
        // previous write may have failed. This is a rare, rejection-only path.
        let _ = self.persist_locked(&providers, &wallets);
    }

    #[cfg(test)]
    pub fn get_persisted(&self, provider_id: &str) -> Option<PersistedProviderState> {
        self.state.lock().unwrap().get(provider_id).cloned()
    }

    #[cfg(test)]
    pub fn get_persisted_wallet(&self, wallet_id: &str) -> Option<PersistedWalletState> {
        self.wallets.lock().unwrap().get(wallet_id).cloned()
    }
}

fn parse_envelope(
    raw: &str,
) -> (
    HashMap<String, PersistedProviderState>,
    HashMap<String, PersistedWalletState>,
    bool,
) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (HashMap::new(), HashMap::new(), false);
    };
    let Some(object) = parsed.as_object() else {
        return (HashMap::new(), HashMap::new(), false);
    };
    if object.get("version").and_then(serde_json::Value::as_u64) != Some(SCHEMA_VERSION as u64) {
        return (HashMap::new(), HashMap::new(), false);
    }
    let Some(providers_val) = object.get("providers").and_then(serde_json::Value::as_array) else {
        return (HashMap::new(), HashMap::new(), false);
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
    // Wallet entries are parsed independently: an absent `wallets` key is an
    // old (valid) envelope, and one bad wallet entry is dropped without
    // touching any provider entry.
    let mut wallets = HashMap::new();
    if let Some(wallets_val) = object.get("wallets") {
        if let Some(entries) = wallets_val.as_array() {
            for entry in entries {
                match serde_json::from_value::<PersistedWalletState>(entry.clone()) {
                    Ok(state) if is_valid_persisted_wallet(&state) => {
                        wallets.insert(state.wallet_id.clone(), normalize_persisted_wallet(state));
                    }
                    _ => healthy = false,
                }
            }
        } else {
            healthy = false;
        }
    }
    (map, wallets, healthy)
}

fn is_valid_persisted_wallet(state: &PersistedWalletState) -> bool {
    if state.wallet_id.trim().is_empty() || state.wallet_name.trim().is_empty() {
        return false;
    }
    if !persisted_balances_are_valid(state) {
        return false;
    }
    DateTime::parse_from_rfc3339(&state.last_successful_at).is_ok()
}

/// Brings a persisted wallet into the canonical currency-aware shape.
///
/// An entry written before the schema carried only the single USD amount.
/// It is migrated into one explicit USD balance, so the in-memory state
/// always states its currency. The migration is a faithful restatement: the
/// old field was explicitly USD, so it invents nothing.
fn normalize_persisted_wallet(mut state: PersistedWalletState) -> PersistedWalletState {
    if state.balances.is_none() {
        state.balances = Some(effective_persisted_balances(&state));
    }
    state
}

/// Whether a persisted wallet carries at least one trustworthy amount.
///
/// Every persisted amount must be finite, non-negative and in a named
/// currency, and the USD convenience amount must agree with the balances it
/// was derived from. A wallet that claims a USD amount it cannot show in its
/// balances is not verifiable and is refused.
fn persisted_balances_are_valid(state: &PersistedWalletState) -> bool {
    let balances = effective_persisted_balances(state);
    if balances.is_empty() {
        return false;
    }
    if !balances.iter().all(|balance| {
        !balance.currency.trim().is_empty()
            && balance.amount.is_finite()
            && balance.amount >= 0.0
    }) {
        return false;
    }
    let derived_usd = if balances.len() == 1 && balances[0].currency == "USD" {
        Some(balances[0].amount)
    } else {
        None
    };
    derived_usd == state.remaining_usd
}

/// The balances a persisted wallet actually carries.
///
/// An envelope written before the currency-aware schema has no `balances`
/// list, only the single USD amount. That entry is migrated into one USD
/// balance: the old field was explicitly USD, so the migration states
/// nothing the file did not already say. A wallet that carries a `balances`
/// list is never migrated — the list is authoritative, even when it is empty.
fn effective_persisted_balances(state: &PersistedWalletState) -> Vec<PersistedWalletBalance> {
    if let Some(balances) = &state.balances {
        return balances.clone();
    }
    match state.remaining_usd {
        Some(amount) if amount.is_finite() && amount >= 0.0 => {
            vec![PersistedWalletBalance {
                currency: "USD".to_string(),
                amount,
                granted: None,
                topped_up: None,
            }]
        }
        _ => Vec::new(),
    }
}

fn is_valid_persisted_state(state: &PersistedProviderState) -> bool {
    if state.provider_id.trim().is_empty() || state.provider_name.trim().is_empty() {
        return false;
    }
    if !provider_identity_is_current(
        &state.provider_id,
        state
            .account
            .as_ref()
            .and_then(|account| account.identity.as_deref()),
    ) {
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
    if !provider_identity_is_current(
        &state.provider_id,
        state
            .account
            .as_ref()
            .and_then(|account| account.identity.as_deref()),
    ) {
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

/// The cache accepts only identities minted by the current Codex binding.
/// Other providers retain their existing attribution rules.
fn provider_identity_is_current(provider_id: &str, identity: Option<&str>) -> bool {
    if !is_codex_provider_id(provider_id) {
        return true;
    }
    let Some(suffix) = identity.and_then(|value| value.strip_prefix("chatgpt-v2:")) else {
        return false;
    };
    suffix.len() == 64
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

    fn codex_identity(hex_digit: char) -> String {
        format!("chatgpt-v2:{}", hex_digit.to_string().repeat(64))
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
        let identity = if is_codex_provider_id(kind.id()) {
            codex_identity('a')
        } else {
            "key:1234".to_string()
        };
        ProviderUsageDto {
            id: kind.id().to_string(),
            name: kind.name().to_string(),
            health: ProviderHealth::Live,
            status: ProviderHealth::Live.legacy_status(),
            checked_at: "2026-09-29T11:55:00.000Z".to_string(),
            limits: vec![UsageLimitDto {
                label: "5-hour limit".to_string(),
                used_percent: percent,
                reset_text: None,
                reset_at: reset_at.map(str::to_string),
            }],
            account: Some(AccountAttributionDto {
                label: "Account A".to_string(),
                note: None,
                identity: Some(identity),
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
        assert!(raw.contains(&codex_identity('a')));

        let persisted = store.get_persisted("openai-codex").expect("should be stored in memory");
        assert_eq!(persisted.limits[0].used_percent, 42.0);
    }

    #[test]
    fn load_drops_both_legacy_codex_entries_and_repairs_disk() {
        let dir = tempdir::temp_dir("legacy-codex-invalidation");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": SCHEMA_VERSION,
            "providers": [
                {
                    "providerId": "openai-codex",
                    "providerName": "OpenAI / Codex",
                    "account": {
                        "label": "Legacy primary",
                        "identity": format!("chatgpt:{}", "a".repeat(64))
                    },
                    "limits": [{ "label": "5-hour limit", "usedPercent": 40.0 }],
                    "lastSuccessfulAt": "2026-09-29T11:00:00Z"
                },
                {
                    "providerId": "openai-codex:secondary",
                    "providerName": "OpenAI / Codex",
                    "account": {
                        "label": "Legacy secondary",
                        "identity": format!("chatgpt-v2:{}", "A".repeat(64))
                    },
                    "limits": [{ "label": "5-hour limit", "usedPercent": 60.0 }],
                    "lastSuccessfulAt": "2026-09-29T11:00:00Z"
                },
                {
                    "providerId": "zai",
                    "providerName": "Z.ai",
                    "account": { "label": "Z.ai key", "identity": "key:zai" },
                    "limits": [{ "label": "Daily", "usedPercent": 35.0 }],
                    "lastSuccessfulAt": "2026-09-29T11:00:00Z"
                }
            ]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));

        assert!(store.get_persisted("openai-codex").is_none());
        assert!(store.get_persisted("openai-codex:secondary").is_none());
        assert_eq!(
            store
                .get_persisted("zai")
                .expect("another provider survives invalidation")
                .limits[0]
                .used_percent,
            35.0
        );

        let repaired: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let provider_ids: Vec<&str> = repaired["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|provider| provider["providerId"].as_str().unwrap())
            .collect();
        assert_eq!(provider_ids, vec!["zai"], "load removes legacy rows from disk");

        let reopened = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        assert!(reopened.get_persisted("openai-codex").is_none());
        assert!(reopened.get_persisted("openai-codex:secondary").is_none());
        assert!(reopened.get_persisted("zai").is_some());
    }

    #[test]
    fn record_success_requires_current_codex_identity_and_preserves_valid_entries() {
        let dir = tempdir::temp_dir("codex-record-identity");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));

        store.record_success(
            ProviderKind::Zai,
            &sample_usage(ProviderKind::Zai, 30.0, None),
        );

        let mut legacy = sample_usage(ProviderKind::Codex, 99.0, None);
        legacy.account = Some(AccountAttributionDto {
            label: "Legacy primary".to_string(),
            note: None,
            identity: Some(format!("chatgpt:{}", "a".repeat(64))),
        });
        store.record_success(ProviderKind::Codex, &legacy);

        let mut legacy_secondary = sample_usage(ProviderKind::CodexSecondary, 98.0, None);
        legacy_secondary.account = Some(AccountAttributionDto {
            label: "Legacy secondary".to_string(),
            note: None,
            identity: Some(format!("chatgpt-v2:{}", "A".repeat(64))),
        });
        store.record_success(ProviderKind::CodexSecondary, &legacy_secondary);

        assert!(store.get_persisted("openai-codex").is_none());
        assert!(store.get_persisted("openai-codex:secondary").is_none());
        assert!(store.get_persisted("zai").is_some());

        let mut primary = sample_usage(ProviderKind::Codex, 40.0, None);
        primary.account.as_mut().unwrap().identity = Some(codex_identity('a'));
        store.record_success(ProviderKind::Codex, &primary);

        let mut secondary = sample_usage(ProviderKind::CodexSecondary, 60.0, None);
        secondary.account.as_mut().unwrap().identity = Some(codex_identity('b'));
        store.record_success(ProviderKind::CodexSecondary, &secondary);

        let reopened = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        assert_eq!(
            reopened
                .get_persisted("openai-codex")
                .expect("a current Codex identity is accepted")
                .limits[0]
                .used_percent,
            40.0
        );
        assert_eq!(
            reopened
                .get_persisted("openai-codex:secondary")
                .expect("the second Codex identity is accepted")
                .account
                .unwrap()
                .identity
                .as_deref(),
            Some(codex_identity('b').as_str())
        );
        assert!(reopened.get_persisted("zai").is_some());
    }

    #[test]
    fn sanitize_cannot_revive_a_legacy_codex_identity() {
        for (provider_id, identity) in [
            ("openai-codex", format!("chatgpt:{}", "a".repeat(64))),
            ("openai-codex:secondary", format!("chatgpt-v2:{}", "A".repeat(64))),
        ] {
            let state = PersistedProviderState {
                provider_id: provider_id.to_string(),
                provider_name: "OpenAI / Codex".to_string(),
                account: Some(PersistedAccountAttribution {
                    label: "Legacy account".to_string(),
                    note: None,
                    identity: Some(identity),
                }),
                limits: vec![
                    PersistedQuotaLimit {
                        label: "Valid window".to_string(),
                        used_percent: 50.0,
                        reset_at: None,
                    },
                    PersistedQuotaLimit {
                        label: String::new(),
                        used_percent: 90.0,
                        reset_at: None,
                    },
                ],
                last_successful_at: "2026-09-29T11:00:00Z".to_string(),
                source_updated_at: None,
            };

            assert!(!is_valid_persisted_state(&state));
            assert!(
                sanitize_provider_state(&state).is_none(),
                "a repairable window must not revive an invalid Codex identity"
            );
        }
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

    #[test]
    fn hydrated_state_never_carries_zcode_plan_observations() {
        // ZCode plan observations are fresh-live-observation-only: even a
        // DTO that held plans persists and hydrates without them, so a cold
        // start always re-observes before a balance reads as available.
        use crate::zcode_plans::{ZCodePlan, ZCodePlanBalance, ZCodePlansObservation};
        let dir = tempdir::temp_dir("no-plan-hydration");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        let mut live = sample_usage(ProviderKind::Zai, 50.0, Some("2026-09-29T16:00:00Z"));
        live.zcode_plans = Some(ZCodePlansObservation {
            plans: vec![ZCodePlan {
                plan_id: "plan-trust-build".to_string(),
                user_plan_id: None,
                name: Some("ZCode Trust Build".to_string()),
                status: "active".to_string(),
                ends_at: None,
                balances: vec![ZCodePlanBalance {
                    user_plan_id: None,
                    entitlement_id: None,
                    bucket_id: None,
                    model: Some("GLM-5.3-Flash".to_string()),
                    meter: None,
                    unit: Some("token".to_string()),
                    limit: Some(100.0),
                    used: None,
                    remaining: Some(94.8),
                    period: None,
                    period_end: None,
                    expires_at: None,
                }],
            }],
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
            provider.zcode_plans, None,
            "plan observations must be re-observed live, never hydrated"
        );
    }

    #[test]
    fn hydrated_state_never_carries_grok_bot_observations() {
        // Grok Bot observations are fresh-live-observation-only: even a DTO
        // that held one persists and hydrates without it, so a cold start
        // always re-observes before a supplemental percentage reads as
        // current.
        use crate::cursor_grok_bot::GrokBotUsage;
        let dir = tempdir::temp_dir("no-grok-bot-hydration");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = Arc::new(ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now)));

        let mut live = sample_usage(ProviderKind::Grok, 50.0, Some("2026-09-29T16:00:00Z"));
        live.grok_bot = Some(GrokBotUsage {
            plan_name: Some("X Premium+".to_string()),
            plan_id: Some("x-premium-plus".to_string()),
            cursor_plan_name: Some("Free".to_string()),
            used_percent: 17.66,
            period_start: Some("2026-10-04T09:12:03.000Z".to_string()),
            reset_at: Some("2026-10-11T09:12:03.000Z".to_string()),
            has_available_usage: Some(true),
            on_demand_enabled: Some(false),
            observed_at: fixed_now(),
        });
        store.record_success(ProviderKind::Grok, &live);

        let specs = vec![mock_spec(
            ProviderKind::Grok,
            Ok(sample_usage(ProviderKind::Grok, 50.0, None)),
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
            provider.grok_bot, None,
            "Grok Bot observations must be re-observed live, never hydrated"
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
            identity: Some(codex_identity('a')),
            credential_binding: None,
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
            credential_binding: None,
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
            identity: Some(codex_identity('a')),
        });
        store.record_success(ProviderKind::Codex, &usage_a);

        let mut usage_b = sample_usage(ProviderKind::Codex, 80.0, Some("2026-09-29T16:00:00Z"));
        let identity_b = codex_identity('b');
        usage_b.account = Some(AccountAttributionDto {
            label: "Account B".to_string(),
            note: None,
            identity: Some(identity_b.clone()),
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
        assert_eq!(acc.identity.as_deref(), Some(identity_b.as_str()));
        assert_eq!(snapshot.providers[0].limits[0].used_percent, 80.0);

        let persisted = store.get_persisted("openai-codex").unwrap();
        assert_eq!(
            persisted.account.unwrap().identity.as_deref(),
            Some(identity_b.as_str())
        );
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
        let proven_identity = codex_identity('c');
        usage_proven.account = Some(AccountAttributionDto {
            label: "Proven User".to_string(),
            note: None,
            identity: Some(proven_identity.clone()),
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

        // Before cycle: the unattributed legacy write was refused, so there is
        // no cached Codex snapshot to expose.
        assert!(store.get_persisted("openai-codex").is_none());
        assert!(core.snapshot().providers.is_empty());

        core.run_cycle().await;

        // After cycle: only the proven account observation is present.
        let snapshot = core.snapshot();
        assert_eq!(
            snapshot.providers[0]
                .account
                .as_ref()
                .unwrap()
                .identity
                .as_deref(),
            Some(proven_identity.as_str())
        );
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
                "account": { "label": "Account A", "identity": codex_identity('a') },
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
                "account": { "label": "Account A", "identity": codex_identity('a') },
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
                    "account": { "label": "Account A", "identity": codex_identity('a') },
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
                "account": { "label": "Account A", "identity": codex_identity('a') },
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
                    identity: Some(codex_identity('a')),
                }),
                limits: vec![PersistedQuotaLimit {
                    label: "5-hour limit".to_string(),
                    used_percent: 50.0,
                    reset_at: Some("2026-09-29T15:00:00.000Z".to_string()),
                }],
                last_successful_at: "2026-09-29T12:00:00.000Z".to_string(),
                source_updated_at: None,
            }],
            wallets: Vec::new(),
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
                reset_text: None,
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

    // ---- persisted prepaid wallets (API wallets phase 1) ----

    const TEST_WALLET_BINDING: &str = "key:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_WALLET_BINDING: &str =
        "key:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn mock_wallet_spec(id: &'static str, name: &'static str) -> WalletSpec {
        mock_wallet_spec_with_binding(id, name, Some(TEST_WALLET_BINDING))
    }

    fn mock_wallet_spec_with_binding(
        id: &'static str,
        name: &'static str,
        binding: Option<&'static str>,
    ) -> WalletSpec {
        WalletSpec {
            id,
            name,
            monitored_by_default: true,
            current_credential_binding: Arc::new(move || binding.map(str::to_string)),
            fetch: Arc::new(|| {
                Box::pin(async {
                    Ok(crate::wallet::tests::usd_balance(0.0))
                }) as crate::runtime::WalletFetchFuture
            }),
        }
    }

    fn wallet_dto(
        health: WalletHealth,
        remaining_usd: Option<f64>,
        identity: Option<&str>,
    ) -> ApiWalletDto {
        wallet_dto_with_binding(health, remaining_usd, identity, Some(TEST_WALLET_BINDING))
    }

    fn wallet_dto_with_binding(
        health: WalletHealth,
        remaining_usd: Option<f64>,
        identity: Option<&str>,
        binding: Option<&str>,
    ) -> ApiWalletDto {
        ApiWalletDto {
            id: "openrouter".to_string(),
            name: "OpenRouter".to_string(),
            health,
            checked_at: (health == WalletHealth::Live)
                .then(|| "2026-09-29T12:00:00.000Z".to_string()),
            balances: remaining_usd
                .map(|amount| {
                    vec![WalletBalanceDto {
                        currency: "USD".to_string(),
                        amount,
                        granted: None,
                        topped_up: None,
                    }]
                })
                .unwrap_or_default(),
            available: None,
            remaining_usd,
            identity: identity.map(str::to_string),
            credential_binding: binding.map(str::to_string),
            error: None,
            error_category: None,
            error_http_status: None,
        }
    }

    // 21. an envelope written before wallets existed still loads
    #[test]
    fn an_envelope_without_wallets_loads_with_an_empty_wallet_list() {
        let dir = tempdir::temp_dir("wallet-old-envelope");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": SCHEMA_VERSION,
            "providers": [{
                "providerId": "zai",
                "providerName": "Z.ai",
                "limits": [{
                    "label": "Daily",
                    "usedPercent": 20.0,
                    "resetAt": "2026-09-29T16:00:00Z"
                }],
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        let specs = vec![mock_spec(
            ProviderKind::Zai,
            Ok(sample_usage(ProviderKind::Zai, 20.0, None)),
        )];
        let (usages, _) = store.hydrate(&specs, fixed_now());
        assert_eq!(usages.len(), 1, "provider hydration is untouched");

        let wallet_specs = vec![mock_wallet_spec("openrouter", "OpenRouter")];
        let (wallets, last_good) = store.hydrate_wallets(&wallet_specs, fixed_now());
        assert!(wallets.is_empty(), "an old envelope has no wallet entries");
        assert!(last_good.is_empty());
    }

    // 22. one malformed wallet entry never invalidates providers
    #[test]
    fn a_malformed_wallet_entry_never_invalidates_providers() {
        let dir = tempdir::temp_dir("wallet-malformed");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": SCHEMA_VERSION,
            "providers": [{
                "providerId": "zai",
                "providerName": "Z.ai",
                "limits": [{
                    "label": "Daily",
                    "usedPercent": 35.0,
                    "resetAt": "2026-09-29T16:00:00Z"
                }],
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }],
            "wallets": [{
                "walletId": "openrouter",
                "walletName": "OpenRouter",
                "balances": [{ "currency": "USD", "amount": "not-a-number" }],
                "remainingUsd": "not-a-number",
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        assert!(store.get_persisted_wallet("openrouter").is_none());
        assert!(
            store.get_persisted("zai").is_some(),
            "the sibling provider entry survives"
        );
        let specs = vec![mock_spec(
            ProviderKind::Zai,
            Ok(sample_usage(ProviderKind::Zai, 35.0, None)),
        )];
        let (usages, _) = store.hydrate(&specs, fixed_now());
        assert_eq!(usages.len(), 1);
    }

    // 23. a persisted balance hydrates as stale and expires after a week
    #[test]
    fn a_persisted_balance_hydrates_as_stale_and_expires_after_seven_days() {
        let dir = tempdir::temp_dir("wallet-hydrate-stale");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        store.record_wallet_success(&wallet_dto(
            WalletHealth::Live,
            Some(4.25),
            Some("key:A1"),
        ));
        let persisted = store
            .get_persisted_wallet("openrouter")
            .expect("the live observation is cached");
        assert_eq!(persisted.remaining_usd, Some(4.25));
        let balances = persisted.balances.as_ref().expect("a written balance");
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].currency, "USD");
        assert_eq!(balances[0].amount, 4.25);

        let specs = vec![mock_wallet_spec("openrouter", "OpenRouter")];
        let (wallets, last_good) = store.hydrate_wallets(&specs, fixed_now());
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].health, WalletHealth::Stale);
        assert_eq!(wallets[0].remaining_usd, Some(4.25));
        assert_eq!(
            wallets[0].checked_at.as_deref(),
            Some("2026-09-29T12:00:00.000Z"),
            "hydration keeps the original observation stamp"
        );
        assert_eq!(
            last_good.get("openrouter").and_then(|dto| dto.remaining_usd),
            Some(4.25)
        );

        let eight_days = fixed_now() + chrono::Duration::days(8);
        let (expired, expired_last_good) = store.hydrate_wallets(&specs, eight_days);
        assert!(expired.is_empty(), "a week-old balance is not last-good");
        assert!(expired_last_good.is_empty());

        // A cold start through the runtime exposes the entry as stale.
        let reopened = Arc::new(ProviderLastGoodStore::open_with_clock(
            path,
            Box::new(fixed_now),
        ));
        let core = RuntimeCore::with_injections(vec![], 5, Box::new(|| 0), Box::new(fixed_now))
            .with_last_good_store(Some(reopened))
            .with_wallets(specs);
        let snapshot = core.snapshot();
        assert_eq!(snapshot.wallets.len(), 1);
        assert_eq!(snapshot.wallets[0].health, WalletHealth::Stale);
        assert_eq!(snapshot.wallets[0].remaining_usd, Some(4.25));
    }

    // 24. only genuine live observations are written
    #[test]
    fn only_live_wallet_observations_are_persisted() {
        let dir = tempdir::temp_dir("wallet-write-policy");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));

        store.record_wallet_success(&wallet_dto(WalletHealth::Error, Some(9.0), None));
        store.record_wallet_success(&wallet_dto(WalletHealth::Stale, Some(9.0), None));
        store.record_wallet_success(&wallet_dto(WalletHealth::Cooldown, Some(9.0), None));
        store.record_wallet_success(&wallet_dto(WalletHealth::Live, None, None));
        assert!(
            store.get_persisted_wallet("openrouter").is_none(),
            "errors, stale projections, and balance-less states never write"
        );

        store.record_wallet_success(&wallet_dto(WalletHealth::Live, Some(0.0), Some("key:A1")));
        let persisted = store
            .get_persisted_wallet("openrouter")
            .expect("a live zero is a real observation");
        assert_eq!(persisted.remaining_usd, Some(0.0));
        assert_eq!(persisted.identity.as_deref(), Some("key:A1"));

        assert!(store.clear(), "the cache had entries to clear");
        assert!(store.get_persisted_wallet("openrouter").is_none());
    }

    // 24b. an envelope written before the currency-aware schema carries only
    // the single USD amount. That entry is migrated into one USD balance
    // rather than dropped: the old field was explicitly USD, so the
    // migration states nothing the file did not already say.
    #[test]
    fn a_legacy_usd_only_wallet_entry_is_migrated_into_one_usd_balance() {
        let dir = tempdir::temp_dir("wallet-legacy-usd");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": SCHEMA_VERSION,
            "providers": [],
            "wallets": [{
                "walletId": "openrouter",
                "walletName": "OpenRouter",
                "remainingUsd": 7.5,
                "identity": "key:A1",
                "credentialBinding": TEST_WALLET_BINDING,
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        let persisted = store
            .get_persisted_wallet("openrouter")
            .expect("a legacy USD entry is migratable");
        let balances = persisted.balances.as_ref().expect("a migrated balance");
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].currency, "USD");
        assert_eq!(balances[0].amount, 7.5);
        assert_eq!(persisted.remaining_usd, Some(7.5));
    }

    // 24c. a wallet that carries an explicit balances list is never
    // migrated: the list is authoritative, even when it is empty.
    #[test]
    fn an_explicit_empty_balance_list_is_never_migrated_from_the_legacy_field() {
        let dir = tempdir::temp_dir("wallet-empty-balances");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": SCHEMA_VERSION,
            "providers": [],
            "wallets": [{
                "walletId": "openrouter",
                "walletName": "OpenRouter",
                "balances": [],
                "remainingUsd": 7.5,
                "identity": "key:A1",
                "credentialBinding": TEST_WALLET_BINDING,
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        assert!(
            store.get_persisted_wallet("openrouter").is_none(),
            "an explicit empty balance list states no balance"
        );
    }

    // 25. a persisted balance from another credential is removed before it
    // can be served, and a second restart cannot resurrect it.
    #[test]
    fn a_persisted_balance_for_another_credential_is_removed_for_good() {
        let dir = tempdir::temp_dir("wallet-credential-mismatch");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);

        let writer = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        writer.record_wallet_success(&wallet_dto(
            WalletHealth::Live,
            Some(4.25),
            Some("key:A1"),
        ));
        assert!(writer.get_persisted_wallet("openrouter").is_some());

        let first_restart = ProviderLastGoodStore::open_with_clock(
            path.clone(),
            Box::new(fixed_now),
        );
        let other = vec![mock_wallet_spec_with_binding(
            "openrouter",
            "OpenRouter",
            Some(OTHER_WALLET_BINDING),
        )];
        let (wallets, last_good) = first_restart.hydrate_wallets(&other, fixed_now());
        assert!(
            wallets.is_empty(),
            "credential B must not inherit credential A's balance"
        );
        assert!(last_good.is_empty());
        assert!(
            first_restart.get_persisted_wallet("openrouter").is_none(),
            "the rejected entry is removed from disk, not merely hidden"
        );
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            !contents.contains("openrouter"),
            "the rewritten envelope no longer carries the rejected entry: {contents}"
        );

        let second_restart = ProviderLastGoodStore::open_with_clock(
            path,
            Box::new(fixed_now),
        );
        let (wallets, last_good) = second_restart.hydrate_wallets(&other, fixed_now());
        assert!(wallets.is_empty(), "a later restart cannot resurrect it");
        assert!(last_good.is_empty());
    }

    // 25b. Eviction is a best-effort rewrite. When the old envelope survives
    // on disk, hydration itself must still refuse it: the restart guarantee
    // is revalidation, not the success of the rewrite.
    #[test]
    fn a_stale_envelope_that_survives_eviction_is_rejected_after_restart() {
        let dir = tempdir::temp_dir("wallet-stale-envelope");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);

        let writer = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        writer.record_wallet_success(&wallet_dto(
            WalletHealth::Live,
            Some(9.75),
            Some("key:A1"),
        ));
        let stale_bytes = fs::read(&path).unwrap();

        let other = vec![mock_wallet_spec_with_binding(
            "openrouter",
            "OpenRouter",
            Some(OTHER_WALLET_BINDING),
        )];
        let first_restart =
            ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        let (wallets, _) = first_restart.hydrate_wallets(&other, fixed_now());
        assert!(wallets.is_empty());

        // Simulate a failed eviction rewrite: the rejected envelope is still
        // exactly what a restart would load.
        fs::write(&path, stale_bytes).unwrap();

        let second_restart =
            ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        let (wallets, last_good) = second_restart.hydrate_wallets(&other, fixed_now());
        assert!(
            wallets.is_empty(),
            "revalidation, not the rewrite, is the restart guarantee"
        );
        assert!(last_good.is_empty());

        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            !contents.contains("openrouter"),
            "the successful hydration rewrite clears the stale entry: {contents}"
        );
    }

    // 25c. The same guarantee under a real failed rewrite: the temp sibling
    // a successful persist writes through is occupied by a directory, so
    // persist_locked cannot write. The old envelope survives untouched, and
    // hydration must still reject it.
    #[test]
    fn a_failed_eviction_rewrite_is_still_rejected_after_restart() {
        let dir = tempdir::temp_dir("wallet-failed-eviction");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);

        let writer = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        writer.record_wallet_success(&wallet_dto(
            WalletHealth::Live,
            Some(6.5),
            Some("key:A1"),
        ));
        let stale_bytes = fs::read(&path).unwrap();

        // Block the envelope rewrite exactly where persist_locked writes.
        let temp_path = path.with_extension("json.tmp");
        fs::create_dir(&temp_path).unwrap();

        let other = vec![mock_wallet_spec_with_binding(
            "openrouter",
            "OpenRouter",
            Some(OTHER_WALLET_BINDING),
        )];
        let first_restart =
            ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        let (wallets, last_good) = first_restart.hydrate_wallets(&other, fixed_now());
        assert!(wallets.is_empty());
        assert!(last_good.is_empty());
        assert!(
            first_restart.get_persisted_wallet("openrouter").is_none(),
            "the in-memory entry is dropped even when the rewrite fails"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            stale_bytes,
            "the rewrite really failed: the old envelope is untouched"
        );

        fs::remove_dir(&temp_path).unwrap();
        let second_restart =
            ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        let (wallets, last_good) = second_restart.hydrate_wallets(&other, fixed_now());
        assert!(
            wallets.is_empty(),
            "a later restart rejects the surviving stale envelope"
        );
        assert!(last_good.is_empty());
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            !contents.contains("openrouter"),
            "the next healthy rewrite clears the rejected entry: {contents}"
        );
    }

    // 26. entries written before the isolation fix carry no binding. They
    // are unverifiable by construction and must be rejected.
    #[test]
    fn a_legacy_wallet_entry_without_a_binding_is_rejected_and_removed() {
        let dir = tempdir::temp_dir("wallet-legacy-binding");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let envelope = serde_json::json!({
            "version": SCHEMA_VERSION,
            "providers": [],
            "wallets": [{
                "walletId": "openrouter",
                "walletName": "OpenRouter",
                "balances": [{ "currency": "USD", "amount": 7.5 }],
                "remainingUsd": 7.5,
                "identity": "key:A1",
                "lastSuccessfulAt": "2026-09-29T11:00:00Z"
            }]
        });
        fs::write(&path, envelope.to_string()).unwrap();

        let store = ProviderLastGoodStore::open_with_clock(
            path.clone(),
            Box::new(fixed_now),
        );
        let specs = vec![mock_wallet_spec("openrouter", "OpenRouter")];
        let (wallets, last_good) = store.hydrate_wallets(&specs, fixed_now());
        assert!(wallets.is_empty(), "a missing binding is never a match");
        assert!(last_good.is_empty());
        assert!(store.get_persisted_wallet("openrouter").is_none());

        let reopened = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        assert!(reopened.get_persisted_wallet("openrouter").is_none());
    }

    // 27. an unreadable or ambiguous current credential cannot prove that a
    // persisted balance belongs to it, so it is rejected and removed.
    #[test]
    fn a_persisted_balance_is_rejected_when_the_current_credential_is_unavailable() {
        let dir = tempdir::temp_dir("wallet-current-binding-missing");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = ProviderLastGoodStore::open_with_clock(path.clone(), Box::new(fixed_now));
        store.record_wallet_success(&wallet_dto(WalletHealth::Live, Some(3.0), None));

        let specs = vec![mock_wallet_spec_with_binding(
            "openrouter",
            "OpenRouter",
            None,
        )];
        let (wallets, last_good) = store.hydrate_wallets(&specs, fixed_now());
        assert!(wallets.is_empty());
        assert!(last_good.is_empty());
        assert!(store.get_persisted_wallet("openrouter").is_none());

        let reopened = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));
        assert!(reopened.get_persisted_wallet("openrouter").is_none());
    }

    // 28. a live observation that cannot be bound to a credential is not
    // written at all: otherwise it would become unverifiable after restart.
    #[test]
    fn an_unbound_live_observation_is_never_persisted() {
        let dir = tempdir::temp_dir("wallet-unbound-write");
        let path = dir.path().join(LAST_GOOD_FILE_NAME);
        let store = ProviderLastGoodStore::open_with_clock(path, Box::new(fixed_now));

        store.record_wallet_success(&wallet_dto_with_binding(
            WalletHealth::Live,
            Some(5.0),
            Some("key:A1"),
            None,
        ));
        assert!(store.get_persisted_wallet("openrouter").is_none());

        store.record_wallet_success(&wallet_dto_with_binding(
            WalletHealth::Live,
            Some(5.0),
            Some("key:A1"),
            Some("   "),
        ));
        assert!(store.get_persisted_wallet("openrouter").is_none());
    }
}


