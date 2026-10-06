//! Quota notifications — v0.5 thresholds plus v0.6 recovery: one bounded,
//! low-noise lane, evaluated in Rust next to the runtime's history
//! recording so exactly one evaluation point exists (the main and floating
//! webviews are consumers of snapshots, never notification sources).
//!
//! The model is deliberately minimal — two fixed thresholds, one recovery
//! kind, one dedup map, one JSON file, no framework:
//!
//! - thresholds: NEAR LIMIT at 80% used, CRITICAL LIMIT at 95% used. No
//!   other notification classes exist (no error, stale, prediction, or
//!   digest notifications);
//! - crossing semantics: a threshold notification fires only when the
//!   previous actual observed usage of the window is below the threshold
//!   and the new actual observation is at or above it. A window already
//!   above the threshold at first sight (app start, a new cycle, a
//!   restart) is baseline, never news — no startup spam. While usage stays
//!   above a fired threshold no further notification fires for it;
//! - recovery semantics (v0.6): a RECOVERY notification fires when a
//!   previously constrained window (previous reset cycle peak >= 80%)
//!   enters a genuinely new reset cycle (previous valid `resetAt`, new
//!   valid `resetAt`, new different and later) and current usage is
//!   below 80%. Usage drops without a `resetAt` transition never notify;
//!   undatable windows (missing/invalid `resetAt`) never recover. One
//!   recovery maximum per provider + account + window + new reset cycle;
//! - identity: state is keyed by the logical quota window
//!   `(providerId, account identity, windowLabel)` — the same identity the
//!   history store uses — plus the observed `resetAt` cycle and the
//!   notification kind, so two accounts of one provider never share
//!   notification state and unattributed state never attaches to a proven
//!   account. Keys carry provider-generated display-safe identity tokens
//!   (`key:3456`, `xai:<id>`); no credential or token material is ever
//!   stored or shown;
//! - reset-cycle re-arm: fired thresholds re-arm only when the window
//!   reports a new, valid, different `resetAt` (a new cycle). Recovery
//!   additionally requires the new `resetAt` to be later than the previous
//!   valid one. A window without a usable `resetAt` is undatable and never
//!   re-arms — no time-of-day guessing, no usage-drop heuristics;
//! - eligibility: notifications require a usable current observation, using
//!   exactly the history store's recording rules
//!   (`history::observations_from_usages`): failed, stale, errored
//!   last-good, cooldown-retained, unknown, and malformed windows never
//!   notify and never move the baseline, and no simulated provider exists
//!   in the Rust production registry at all;
//! - burst guard: at most [`MAX_NOTIFICATIONS_PER_CYCLE`] native
//!   notifications per cycle, ordered most severe first (critical, then
//!   near-limit, then recovery), then by usage, then by identity —
//!   deterministic. Windows whose notification did not fit the cap are not
//!   marked as fired, so a later genuine crossing can still notify them.
//!
//! Persistence is one bounded JSON file (`quota-notifications-v1.json`) in
//! the app-data directory, written temp-then-rename like the history store.
//! It holds the enabled flag and the per-window dedup state, so restarting
//! the app cannot re-notify the current cycle. v0.5 state files load safely
//! (new peak/prev fields default to empty; no migration screen). A corrupt
//! or foreign-version file degrades to empty and is rewritten sanitized;
//! the map is bounded to the most recently updated
//! [`NOTIFICATION_STATE_MAX_WINDOWS`] windows.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::history::observations_from_usages;
use crate::runtime::ProviderUsageDto;

/// NEAR LIMIT fires when a usable window reaches this share of used quota.
pub const NEAR_LIMIT_THRESHOLD: f64 = 80.0;
/// CRITICAL LIMIT fires when a usable window reaches this share of used quota.
pub const CRITICAL_THRESHOLD: f64 = 95.0;
/// Native notifications delivered per cycle at most (burst guard). Any
/// further qualifying windows stay unmarked and wait for a later crossing.
pub const MAX_NOTIFICATIONS_PER_CYCLE: usize = 3;
/// Hard cap on tracked windows: the most recently updated entries survive.
pub const NOTIFICATION_STATE_MAX_WINDOWS: usize = 64;
/// Versioned file name, alongside the other app-data artifacts.
pub const NOTIFICATIONS_FILE_NAME: &str = "quota-notifications-v1.json";

const BLOB_VERSION: u32 = 1;

/// Notification kinds, ordered by ascending severity: recovery is the least
/// severe so the shared burst guard deterministically orders critical first,
/// then near-limit, then recovery. Serialized values for the two threshold
/// kinds are unchanged (v0.5 state files keep loading).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThresholdKind {
    Recovery,
    NearLimit,
    Critical,
}

impl ThresholdKind {
    fn threshold(self) -> f64 {
        match self {
            ThresholdKind::NearLimit => NEAR_LIMIT_THRESHOLD,
            ThresholdKind::Critical => CRITICAL_THRESHOLD,
            // Recovery has no threshold; infinity keeps it out of any
            // accidental crossing check (finite usage can never reach it).
            ThresholdKind::Recovery => f64::INFINITY,
        }
    }

    /// Threshold kinds only, ascending — evaluation order (the highest
    /// crossed wins). Recovery is evaluated separately on reset evidence.
    const ALL: [ThresholdKind; 2] = [ThresholdKind::NearLimit, ThresholdKind::Critical];
}

/// One notification to deliver. Carries only display-safe metadata; the
/// account identity is present solely for deterministic ordering and is
/// never part of the rendered copy.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaNotification {
    pub kind: ThresholdKind,
    pub provider_id: String,
    pub provider_name: String,
    pub window_label: String,
    /// The actual observed usage that crossed the threshold (0–100).
    pub used_percent: f64,
    /// Provider-generated display-safe identity token (`key:3456`,
    /// `xai:<id>`) — never a credential; never rendered.
    pub account: Option<String>,
    /// Canonical `resetAt` of the observed cycle, when the provider sent a
    /// parseable one.
    pub reset_at: Option<String>,
}

/// Dedup/re-arm state of one logical window
/// `(providerId, account, windowLabel)` for its current reset cycle.
/// v0.6 adds three additive, backward-compatible fields for recovery
/// (missing in v0.5 files -> None): the current cycle peak, the previous
/// cycle peak, and the previous cycle reset. Together they prove a genuine
/// new quota cycle without a second store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowState {
    provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    window_label: String,
    /// Canonical `resetAt` of the cycle these `fired` marks belong to.
    /// `None` (or an unparseable stored value) means undatable: no re-arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reset_at: Option<String>,
    /// Last actual observed usage — the crossing baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_percent: Option<f64>,
    /// Thresholds/recovery already notified for in the current cycle.
    #[serde(default)]
    fired: Vec<ThresholdKind>,
    /// Peak eligible usage observed in the current reset cycle. Drives the
    /// next cycle's recovery eligibility once this cycle ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cycle_peak: Option<f64>,
    /// Peak eligible usage of the previous reset cycle. Set only on a
    /// forward valid-reset transition; preserved for the whole new cycle so
    /// a later drop in the same new cycle can still recover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prev_peak: Option<f64>,
    /// Canonical `resetAt` of the previous cycle. Set alongside
    /// `prev_peak`; required valid for recovery evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prev_reset_at: Option<String>,
    /// LRU stamp for the bounded map.
    updated_at_ms: i64,
}

type WindowKey = (String, Option<String>, String);

#[derive(Default)]
struct LaneState {
    enabled: bool,
    windows: HashMap<WindowKey, WindowState>,
}

/// The persisted envelope: `{ version, enabled, windows: [...] }`. Only
/// safe display metadata and the two fixed thresholds' dedup state — the
/// schema cannot carry secrets.
#[derive(Serialize, Deserialize)]
struct StateBlob {
    version: u32,
    enabled: bool,
    windows: Vec<WindowState>,
}

/// The notification lane: one evaluation point per runtime, bounded state
/// behind a mutex, one JSON file, and an injected delivery sink (the native
/// Tauri notifier in production; a collector in tests).
pub struct NotificationLane {
    state: Mutex<LaneState>,
    path: PathBuf,
    sink: Box<dyn Fn(&[QuotaNotification]) + Send + Sync>,
}

impl NotificationLane {
    /// Opens (and loads/heals) the lane at the given app-data path.
    pub fn open(path: PathBuf, sink: Box<dyn Fn(&[QuotaNotification]) + Send + Sync>) -> Self {
        let lane = Self {
            state: Mutex::new(LaneState::default()),
            path,
            sink,
        };
        lane.load_from_disk();
        lane
    }

    // ---- load / self-heal ----

    /// Loads the persisted blob. A missing file is a healthy disabled lane;
    /// corruption or a foreign version degrades to empty (with salvage of
    /// structurally valid entries) and the sanitized state is written back.
    /// Never crashes startup.
    fn load_from_disk(&self) {
        let Ok(raw) = fs::read_to_string(&self.path) else {
            return;
        };
        let (enabled, windows, healthy) = parse_blob(&raw);
        let mut state = self.state.lock().unwrap();
        state.enabled = enabled;
        state.windows = windows;
        if !healthy {
            drop(state);
            self.persist();
        }
    }

    // ---- settings ----

    /// The toggle pushed from the settings drawer (the persisted TS setting
    /// is forwarded on attach and on every change, like the refresh
    /// interval). Persisted here too, so the scheduler's immediate startup
    /// cycle — which can run before any webview attaches — respects the
    /// user's choice.
    pub fn set_enabled(&self, enabled: bool) {
        let changed = {
            let mut state = self.state.lock().unwrap();
            if state.enabled == enabled {
                false
            } else {
                state.enabled = enabled;
                true
            }
        };
        if changed {
            self.persist();
        }
    }

    /// Test-only read of the persisted toggle.
    #[cfg(test)]
    pub fn is_enabled(&self) -> bool {
        self.state.lock().unwrap().enabled
    }

    /// Safe aggregate state for diagnostics. Returns no window identities or
    /// notification payloads: only bounded counts needed by support.
    pub fn diagnostic_counts(&self) -> (bool, usize, usize) {
        let state = self.state.lock().unwrap();
        (
            state.enabled,
            state.windows.len(),
            state
                .windows
                .values()
                .filter(|window| !window.fired.is_empty())
                .count(),
        )
    }

    // ---- evaluation ----

    /// Evaluates one completed runtime cycle. Eligible observations update
    /// the crossing baseline and the peak tracking regardless of the toggle
    /// (so re-enabling never retro-spams an already-crossed threshold or an
    /// old recovery); emissions require the toggle on, a genuine crossing
    /// or a proven reset recovery, an unfired kind, and room under the
    /// per-cycle cap. Delivered notifications are persisted as fired before
    /// the sink is called, so a crash between the two cannot duplicate them.
    pub fn process(&self, usages: &[ProviderUsageDto], now: DateTime<Utc>) -> Vec<QuotaNotification> {
        let now_ms = now.timestamp_millis();
        // One eligibility definition, shared with history recording.
        let observations = observations_from_usages(usages);
        if observations.is_empty() {
            return Vec::new();
        }
        let names: HashMap<&str, &str> = usages
            .iter()
            .map(|usage| (usage.id.as_str(), usage.name.as_str()))
            .collect();

        struct Pending {
            key: WindowKey,
            notification: QuotaNotification,
            /// Kinds to mark fired once this notification is delivered
            /// (both threshold kinds when one cycle jumped straight past
            /// them; a single recovery kind otherwise).
            kinds: Vec<ThresholdKind>,
        }
        let mut pending: Vec<Pending> = Vec::new();
        let mut changed = false;

        {
            let mut state = self.state.lock().unwrap();
            let enabled = state.enabled;
            for observation in observations {
                let key = (
                    observation.provider_id.clone(),
                    observation.account.clone(),
                    observation.window_label.clone(),
                );
                let entry = state
                    .windows
                    .entry(key.clone())
                    .or_insert_with(|| WindowState {
                        provider_id: observation.provider_id.clone(),
                        account: observation.account.clone(),
                        window_label: observation.window_label.clone(),
                        reset_at: None,
                        last_percent: None,
                        fired: Vec::new(),
                        cycle_peak: None,
                        prev_peak: None,
                        prev_reset_at: None,
                        updated_at_ms: now_ms,
                    });

                // ---- reset-cycle tracking ----
                // Threshold re-arm keeps its v0.5 rule: any new, valid,
                // different `resetAt` is a new cycle (fired marks void, first
                // observation becomes baseline). Recovery additionally needs
                // forward evidence (new later than previous valid) plus a
                // constrained previous peak; both are captured here so a
                // later drop in the same new cycle can still recover.
                let new_reset_ms = parse_epoch_ms(observation.reset_at.as_deref());
                let current_reset_ms = entry
                    .reset_at
                    .as_deref()
                    .and_then(|value| parse_epoch_ms(Some(value)));
                let is_fresh = entry.reset_at.is_none()
                    && entry.last_percent.is_none()
                    && entry.cycle_peak.is_none()
                    && entry.prev_peak.is_none()
                    && entry.fired.is_empty();
                // True only when this very observation carries a forward
                // reset transition (previous valid, new valid, different and
                // later). Recovery is evaluated ONLY here — never on later
                // drops in the same cycle (no usage-drop heuristic).
                let mut just_reset_forward = false;
                if let Some(new_reset) = new_reset_ms {
                    match current_reset_ms {
                        Some(current) if current != new_reset => {
                            if new_reset > current {
                                // Genuine forward reset: remember the
                                // just-ended cycle peak (falling back to the
                                // last baseline for pre-v0.6 state that never
                                // tracked peaks) as recovery evidence.
                                let ended_peak = entry.cycle_peak.or(entry.last_percent);
                                entry.prev_peak = ended_peak;
                                entry.prev_reset_at = entry.reset_at.clone();
                                just_reset_forward = true;
                            } else {
                                // Backward reset (clock skew): keep threshold
                                // re-arm but void recovery evidence — a new
                                // cycle must be later, never earlier.
                                entry.prev_peak = None;
                                entry.prev_reset_at = None;
                            }
                            entry.fired.clear();
                            entry.last_percent = None;
                            entry.reset_at = Some(canonical_timestamp(new_reset));
                            entry.cycle_peak = Some(observation.used_percent);
                            changed = true;
                        }
                        None => {
                            // First observation or undatable -> datable. No
                            // previous valid reset exists, so no recovery
                            // evidence (first sight is always baseline).
                            if !is_fresh {
                                entry.fired.clear();
                            }
                            entry.last_percent = None;
                            entry.reset_at = Some(canonical_timestamp(new_reset));
                            entry.cycle_peak = Some(observation.used_percent);
                            // prev_* stays None: no proven previous cycle.
                            changed = true;
                        }
                        _ => {
                            // Same cycle: advance the peak, keep prev evidence
                            // for diagnostics (recovery is NOT evaluated here).
                            entry.cycle_peak = Some(
                                entry
                                    .cycle_peak
                                    .map_or(observation.used_percent, |peak| {
                                        peak.max(observation.used_percent)
                                    }),
                            );
                        }
                    }
                } else {
                    // Undatable observation: no reset identity, no recovery.
                    // Still track the peak so a later datable reset has a
                    // complete previous-cycle picture once it becomes dated.
                    entry.cycle_peak = Some(
                        entry
                            .cycle_peak
                            .map_or(observation.used_percent, |peak| {
                                peak.max(observation.used_percent)
                            }),
                    );
                }

                // ---- recovery detection (v0.6, conservative) ----
                // Fires ONLY on the reset-transition observation itself:
                // previous valid reset, new valid reset later, previous peak
                // >= 80%, current < 80%, recovery unfired. A new cycle that
                // starts high (e.g. 87%) does NOT recover later in the same
                // cycle when usage drops — that would be a usage-drop
                // heuristic, explicitly out of scope for v0.6.
                let recovery_eligible = just_reset_forward
                    && !entry.fired.contains(&ThresholdKind::Recovery)
                    && observation.used_percent < NEAR_LIMIT_THRESHOLD
                    && matches!(entry.prev_peak, Some(peak) if peak >= NEAR_LIMIT_THRESHOLD);
                if recovery_eligible && enabled {
                    let notification = QuotaNotification {
                        kind: ThresholdKind::Recovery,
                        provider_id: observation.provider_id.clone(),
                        provider_name: names
                            .get(observation.provider_id.as_str())
                            .map(|name| (*name).to_string())
                            .unwrap_or_else(|| observation.provider_id.clone()),
                        window_label: observation.window_label.clone(),
                        used_percent: observation.used_percent,
                        account: observation.account.clone(),
                        reset_at: entry.reset_at.clone(),
                    };
                    pending.push(Pending {
                        key: key.clone(),
                        notification,
                        kinds: vec![ThresholdKind::Recovery],
                    });
                }

                // Crossing detection against the previous actual
                // observation. A first observation (`last_percent: None`)
                // is always baseline: no crossing, no notification.
                // Unchanged v0.5 behavior.
                let mut kinds: Vec<ThresholdKind> = ThresholdKind::ALL
                    .iter()
                    .copied()
                    .filter(|kind| {
                        matches!(entry.last_percent, Some(previous) if previous < kind.threshold())
                            && observation.used_percent >= kind.threshold()
                            && !entry.fired.contains(kind)
                    })
                    .collect();
                // Recovery and threshold crossings are mutually exclusive for
                // a single observation (< 80 vs >= 80), so at most one
                // pending per window per cycle still holds. Prefer the
                // threshold path when both somehow qualify (defensive).
                let recovery_pending = pending.last().map_or(false, |p| {
                    p.key == key && p.notification.kind == ThresholdKind::Recovery
                });
                if !kinds.is_empty() && enabled && !recovery_pending {
                    // At most one notification per window per cycle: the
                    // highest severity crossed (70% -> 97% is a critical,
                    // not near + critical; both kinds are marked fired).
                    let kind = *kinds.last().unwrap();
                    let notification = QuotaNotification {
                        kind,
                        provider_id: observation.provider_id.clone(),
                        provider_name: names
                            .get(observation.provider_id.as_str())
                            .map(|name| (*name).to_string())
                            .unwrap_or_else(|| observation.provider_id.clone()),
                        window_label: observation.window_label.clone(),
                        used_percent: observation.used_percent,
                        account: observation.account.clone(),
                        reset_at: entry.reset_at.clone(),
                    };
                    pending.push(Pending {
                        key,
                        notification,
                        kinds,
                    });
                } else {
                    // Suppressed, disabled, or recovery already pending for
                    // this window: no threshold kinds carried forward.
                    kinds.clear();
                }

                // The baseline always advances — suppressed, capped, and
                // disabled cycles included — so a re-enabled lane only ever
                // fires on a fresh crossing and never retro-fires an old
                // recovery.
                if entry.last_percent != Some(observation.used_percent) {
                    entry.last_percent = Some(observation.used_percent);
                    changed = true;
                }
                if entry.updated_at_ms != now_ms {
                    entry.updated_at_ms = now_ms;
                    changed = true;
                }
                // cycle_peak already advanced above; ensure undatable-first
                // and same-cycle paths persist even when the match arms above
                // did not mark changed (peak growth alone is a change).
                // (changed is already true in reset arms; same-cycle peak
                // growth is covered by last_percent/updated_at changes in
                // practice, but keep the state truthful regardless.)
            }

            if !pending.is_empty() {
                // Burst guard: most severe first, then highest usage, then
                // identity order — deterministic.
                pending.sort_by(|a, b| {
                    b.notification
                        .kind
                        .cmp(&a.notification.kind)
                        .then_with(|| {
                            b.notification
                                .used_percent
                                .partial_cmp(&a.notification.used_percent)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .then_with(|| a.key.cmp(&b.key))
                });
                let delivered = pending
                    .drain(..)
                    .take(MAX_NOTIFICATIONS_PER_CYCLE)
                    .collect::<Vec<_>>();
                for delivered in &delivered {
                    if let Some(entry) = state.windows.get_mut(&delivered.key) {
                        for kind in &delivered.kinds {
                            if !entry.fired.contains(kind) {
                                entry.fired.push(*kind);
                            }
                        }
                    }
                }
                changed = true;
                let notifications: Vec<QuotaNotification> = delivered
                    .into_iter()
                    .map(|delivered| delivered.notification)
                    .collect();
                drop(state);
                // Fired-before-deliver: a crash between persist and the
                // native call costs one missed notification, never a
                // duplicate one.
                if changed {
                    self.prune_and_persist();
                }
                (self.sink)(&notifications);
                return notifications;
            }
        }

        if changed {
            self.prune_and_persist();
        }
        Vec::new()
    }

    // ---- persistence ----

    /// Bounds the map to the most recently updated windows, then writes the
    /// state file. Filesystem I/O never holds the state lock, mirroring the
    /// history store.
    fn prune_and_persist(&self) {
        {
            let mut state = self.state.lock().unwrap();
            if state.windows.len() > NOTIFICATION_STATE_MAX_WINDOWS {
                let mut ranked: Vec<(i64, WindowKey)> = state
                    .windows
                    .iter()
                    .map(|(key, entry)| (entry.updated_at_ms, key.clone()))
                    .collect();
                ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                for (_, key) in ranked.into_iter().skip(NOTIFICATION_STATE_MAX_WINDOWS) {
                    state.windows.remove(&key);
                }
            }
        }
        self.persist();
    }

    /// Serializes the bounded state and replaces the file atomically-ish:
    /// temp file in the same directory, then rename (Windows `MoveFileEx`
    /// semantics replace the existing file).
    fn persist(&self) {
        let (enabled, mut windows) = {
            let state = self.state.lock().unwrap();
            (state.enabled, state.windows.values().cloned().collect::<Vec<_>>())
        };
        windows.sort_by(|a, b| {
            a.provider_id
                .cmp(&b.provider_id)
                .then_with(|| a.account.cmp(&b.account))
                .then_with(|| a.window_label.cmp(&b.window_label))
        });
        let blob = StateBlob {
            version: BLOB_VERSION,
            enabled,
            windows,
        };
        let Ok(json) = serde_json::to_string(&blob) else {
            return;
        };
        let temp_path = self.path.with_extension("json.tmp");
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&temp_path, json).and_then(|()| fs::rename(&temp_path, &self.path));
    }
}

/// Parses the persisted blob into `(enabled, windows, healthy)`. Invalid
/// entries are dropped individually while structurally valid ones are
/// salvaged (v0.5 files without the v0.6 peak/prev fields salvage cleanly
/// via serde defaults); a corrupt document or a foreign version degrades to
/// a disabled empty lane (history-style: an unknown layout must not blend
/// in).
fn parse_blob(raw: &str) -> (bool, HashMap<WindowKey, WindowState>, bool) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (false, HashMap::new(), false);
    };
    let Some(object) = parsed.as_object() else {
        return (false, HashMap::new(), false);
    };
    if object.get("version").and_then(serde_json::Value::as_u64) != Some(BLOB_VERSION as u64) {
        return (false, HashMap::new(), false);
    }
    let enabled = object
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut windows = HashMap::new();
    let mut healthy = true;
    if let Some(entries) = object.get("windows").and_then(serde_json::Value::as_array) {
        for entry in entries {
            let salvaged = serde_json::from_value::<WindowState>(entry.clone()).ok();
            match salvaged {
                Some(state)
                    if !state.provider_id.trim().is_empty()
                        && !state.window_label.trim().is_empty()
                        && state.last_percent.map_or(true, |percent| percent.is_finite())
                        && state.cycle_peak.map_or(true, |percent| percent.is_finite())
                        && state.prev_peak.map_or(true, |percent| percent.is_finite()) =>
                {
                    windows.insert(
                        (state.provider_id.clone(), state.account.clone(), state.window_label.clone()),
                        state,
                    );
                }
                _ => healthy = false,
            }
        }
    } else {
        healthy = false;
    }
    (enabled, windows, healthy)
}

// ---------- timestamp helpers (history-store parity) ----------

fn parse_epoch_ms(value: Option<&str>) -> Option<i64> {
    DateTime::parse_from_rfc3339(value?)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc).timestamp_millis())
}

fn canonical_timestamp(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(|| DateTime::<Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

// ---------- native delivery ----------

/// Renders and shows the Windows notifications for one cycle's deliveries.
/// Best-effort: a failed toast never fails the runtime cycle. Copy is
/// compact and factual — no recommendations, no raw account identities.
pub fn deliver_native(app: &AppHandle, notifications: &[QuotaNotification]) {
    for notification in notifications {
        let (title, body) = notification_copy(
            &notification.provider_name,
            &notification.window_label,
            notification.kind,
            notification.used_percent,
            notification.reset_at.as_deref(),
        );
        let _ = app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }
}

/// Factual, compact copy for all notification kinds. Recovery copy names the
/// reset and the current usage; never recommendations, never raw accounts.
fn notification_copy(
    provider_name: &str,
    window_label: &str,
    kind: ThresholdKind,
    used_percent: f64,
    reset_at: Option<&str>,
) -> (String, String) {
    match kind {
        ThresholdKind::Recovery => (
            format!("{provider_name} quota available again"),
            format!(
                "{window_label} reset · {}% used",
                used_percent.round() as i64
            ),
        ),
        ThresholdKind::Critical => (
            format!(
                "{provider_name} quota at {}%",
                used_percent.round() as i64
            ),
            format!("{window_label} · near limit"),
        ),
        ThresholdKind::NearLimit => (
            format!(
                "{provider_name} quota at {}%",
                used_percent.round() as i64
            ),
            match resets_in(reset_at) {
                Some(remaining) => {
                    format!("{window_label} · resets in {remaining}")
                }
                None => window_label.to_string(),
            },
        ),
    }
}

/// Humanized remaining time until a reset: `2d 3h`, `4h 12m`, or `35m`.
/// `None` for missing/unparseable/past resets (the copy then omits it).
fn resets_in(reset_at: Option<&str>) -> Option<String> {
    resets_in_from(reset_at, Utc::now())
}

fn resets_in_from(reset_at: Option<&str>, now: DateTime<Utc>) -> Option<String> {
    let reset = DateTime::parse_from_rfc3339(reset_at?)
        .ok()?
        .with_timezone(&Utc);
    let remaining = reset - now;
    if remaining <= chrono::Duration::zero() {
        return None;
    }
    let days = remaining.num_days();
    let hours = remaining.num_hours();
    let minutes = remaining.num_minutes();
    Some(if days >= 1 {
        format!("{days}d {}h", hours - days * 24)
    } else if hours >= 1 {
        format!("{hours}h {}m", minutes - hours * 60)
    } else {
        format!("{minutes}m")
    })
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{AccountAttributionDto, UsageLimitDto};
    use chrono::TimeZone;
    use std::sync::{Arc, Mutex};

    const NOW_MS: i64 = 1_790_596_800_000; // == Date.parse("2026-09-28T12:00:00.000Z")
    const NOW: &str = "2026-09-28T12:00:00.000Z";
    const RESET_LATER: &str = "2026-09-28T16:00:00.000Z";
    const RESET_NEXT: &str = "2026-10-05T16:00:00.000Z";

    fn fixed_now() -> DateTime<Utc> {
        Utc.timestamp_millis_opt(NOW_MS).unwrap()
    }

    /// Minimal temp-dir helper (no new dependencies): a unique directory
    /// under the system temp root, removed on drop.
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

            /// A second handle to the same directory (drops remove twice;
            /// the second removal silently no-ops).
            pub fn retain(&self) -> TempDir {
                TempDir {
                    path: self.path.clone(),
                }
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        pub fn temp_dir(tag: &str) -> TempDir {
            let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "rate-limits-notifications-test-{}-{}-{}",
                tag,
                std::process::id(),
                unique
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }
    }

    struct Harness {
        dir: tempdir::TempDir,
        lane: NotificationLane,
        delivered: Arc<Mutex<Vec<QuotaNotification>>>,
    }

    /// A fresh lane in a fresh temp dir. The lane defaults to disabled —
    /// the same opt-in posture as the other interruptive setting,
    /// autostart — so disabled-suppression tests need no extra setup.
    fn harness(tag: &str) -> Harness {
        let dir = tempdir::temp_dir(tag);
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let lane = NotificationLane::open(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            Box::new(move |notes| {
                sink.lock().unwrap().extend_from_slice(notes);
            }),
        );
        Harness {
            dir,
            lane,
            delivered,
        }
    }

    /// A fresh lane with the quota-notifications toggle already on.
    fn enabled_harness(tag: &str) -> Harness {
        let harness = harness(tag);
        harness.lane.set_enabled(true);
        harness
    }

    impl Harness {
        /// One completed runtime cycle at the fixed clock.
        fn cycle(&self, usages: &[ProviderUsageDto]) -> Vec<QuotaNotification> {
            self.lane.process(usages, fixed_now())
        }

        fn deliveries(&self) -> Vec<QuotaNotification> {
            self.delivered.lock().unwrap().clone()
        }

        /// Reopens the lane from the same persisted file (an app restart),
        /// with a fresh sink.
        fn reopen(&self) -> Harness {
            let delivered = Arc::new(Mutex::new(Vec::new()));
            let sink = delivered.clone();
            let lane = NotificationLane::open(
                self.dir.path().join(NOTIFICATIONS_FILE_NAME),
                Box::new(move |notes| {
                    sink.lock().unwrap().extend_from_slice(notes);
                }),
            );
            Harness {
                dir: self.dir.retain(),
                lane,
                delivered,
            }
        }

        fn persisted_blob(&self) -> serde_json::Value {
            let raw = std::fs::read_to_string(self.dir.path().join(NOTIFICATIONS_FILE_NAME))
                .expect("notification state file exists after a cycle");
            serde_json::from_str(&raw).expect("persisted state is valid JSON")
        }
    }

    fn usage(percent: f64) -> ProviderUsageDto {
        ProviderUsageDto {
            id: "openai-codex".to_string(),
            name: "OpenAI / Codex".to_string(),
            status: "ok",
            health: crate::runtime::ProviderHealth::Live,
            checked_at: NOW.to_string(),
            limits: vec![UsageLimitDto {
                label: "Weekly credits".to_string(),
                used_percent: percent,
                reset_at: Some(RESET_LATER.to_string()),
            }],
            account: None,
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
        }
    }

    fn named_usage(id: &str, name: &str, percent: f64) -> ProviderUsageDto {
        ProviderUsageDto {
            id: id.to_string(),
            name: name.to_string(),
            status: "ok",
            health: crate::runtime::ProviderHealth::Live,
            checked_at: NOW.to_string(),
            limits: vec![UsageLimitDto {
                label: "Weekly credits".to_string(),
                used_percent: percent,
                reset_at: Some(RESET_LATER.to_string()),
            }],
            account: None,
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
        }
    }

    fn with_reset_at(mut usage: ProviderUsageDto, reset_at: &str) -> ProviderUsageDto {
        usage.limits[0].reset_at = Some(reset_at.to_string());
        usage
    }

    fn with_account(mut usage: ProviderUsageDto, identity: &str) -> ProviderUsageDto {
        usage.account = Some(AccountAttributionDto {
            label: format!("key ••{}", &identity[identity.len() - 4..]),
            note: None,
            identity: Some(identity.to_string()),
        });
        usage
    }

    fn with_status(mut usage: ProviderUsageDto, status: &'static str) -> ProviderUsageDto {
        usage.status = status;
        usage.health = match status {
            "ok" => crate::runtime::ProviderHealth::Live,
            "stale" => crate::runtime::ProviderHealth::Stale,
            "unknown" => crate::runtime::ProviderHealth::Unknown,
            _ => crate::runtime::ProviderHealth::Error,
        };
        usage
    }

    fn with_freshness(mut usage: ProviderUsageDto, freshness: &'static str) -> ProviderUsageDto {
        usage.data_freshness = Some(freshness);
        usage
    }

    // 1. 79 -> 81 triggers exactly one near-limit notification
    #[test]
    fn crossing_the_near_threshold_fires_once() {
        let harness = enabled_harness("near-crossing");
        assert!(harness.cycle(&[usage(79.0)]).is_empty(), "below threshold: baseline only");
        let delivered = harness.cycle(&[usage(81.0)]);
        assert_eq!(delivered.len(), 1);
        let note = &delivered[0];
        assert_eq!(note.kind, ThresholdKind::NearLimit);
        assert_eq!(note.provider_id, "openai-codex");
        assert_eq!(note.provider_name, "OpenAI / Codex");
        assert_eq!(note.window_label, "Weekly credits");
        assert_eq!(note.used_percent, 81.0);
        assert_eq!(note.reset_at.as_deref(), Some(RESET_LATER));
        assert_eq!(harness.deliveries().len(), 1, "the sink saw the same cycle");
    }

    // 2. 81 -> 82 does not re-trigger while above the threshold
    #[test]
    fn staying_above_the_threshold_does_not_retrigger() {
        let harness = enabled_harness("near-no-retrigger");
        harness.cycle(&[usage(79.0)]);
        harness.cycle(&[usage(81.0)]);
        harness.cycle(&[usage(82.0)]);
        harness.cycle(&[usage(84.0)]);
        assert_eq!(harness.deliveries().len(), 1);
    }

    // 3. 94 -> 96 triggers a critical notification
    #[test]
    fn crossing_the_critical_threshold_fires_critical() {
        let harness = enabled_harness("critical-crossing");
        harness.cycle(&[usage(94.0)]);
        let delivered = harness.cycle(&[usage(96.0)]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].kind, ThresholdKind::Critical);
        assert_eq!(delivered[0].used_percent, 96.0);
    }

    // 4. 70 -> 97 emits critical only (never near + critical), and neither
    // threshold of that cycle fires again afterwards
    #[test]
    fn jumping_past_both_thresholds_emits_the_higher_severity_only() {
        let harness = enabled_harness("jump-both");
        harness.cycle(&[usage(70.0)]);
        let delivered = harness.cycle(&[usage(97.0)]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].kind, ThresholdKind::Critical);

        // Both kinds are marked fired for this cycle: dipping back under
        // and crossing again must stay silent until a new reset cycle.
        harness.cycle(&[usage(70.0)]);
        assert!(harness.cycle(&[usage(81.0)]).is_empty());
        assert_eq!(harness.deliveries().len(), 1);
    }

    // 5. a new resetAt re-arms notifications
    #[test]
    fn a_new_reset_cycle_re_arms_notifications() {
        let harness = enabled_harness("re-arm");
        harness.cycle(&[with_reset_at(usage(79.0), RESET_LATER)]);
        assert_eq!(harness.cycle(&[with_reset_at(usage(81.0), RESET_LATER)]).len(), 1);

        // New cycle: its first observation is baseline (even above the
        // threshold), then a genuine crossing fires again.
        assert!(harness
            .cycle(&[with_reset_at(usage(85.0), RESET_NEXT)])
            .is_empty());
        assert!(harness
            .cycle(&[with_reset_at(usage(79.0), RESET_NEXT)])
            .is_empty());
        let delivered = harness.cycle(&[with_reset_at(usage(81.0), RESET_NEXT)]);
        assert_eq!(delivered.len(), 1, "the same threshold fired again in the new cycle");
        assert_eq!(harness.deliveries().len(), 2);
    }

    // 6. the same resetAt never re-arms
    #[test]
    fn the_same_reset_cycle_never_re_arms() {
        let harness = enabled_harness("no-re-arm");
        harness.cycle(&[usage(79.0)]);
        harness.cycle(&[usage(81.0)]);
        harness.cycle(&[usage(70.0)]);
        assert!(harness.cycle(&[usage(81.0)]).is_empty(), "still the same cycle");
        assert_eq!(harness.deliveries().len(), 1);
    }

    // 6b. a window without a usable resetAt is undatable and never re-arms
    #[test]
    fn undatable_windows_never_re_arm() {
        let harness = enabled_harness("undatable");
        let mut no_reset = usage(79.0);
        no_reset.limits[0].reset_at = None;
        harness.cycle(&[no_reset.clone()]);
        let mut crossing = no_reset.clone();
        crossing.limits[0].used_percent = 81.0;
        let delivered = harness.cycle(&[crossing.clone()]);
        assert_eq!(delivered.len(), 1, "baseline 79 -> 81 still crosses");
        let mut dropped = no_reset.clone();
        dropped.limits[0].used_percent = 70.0;
        harness.cycle(&[dropped]);
        let mut again = no_reset;
        again.limits[0].used_percent = 81.0;
        assert!(
            harness.cycle(&[again]).is_empty(),
            "no reset identity: no time-of-day guessing, no re-arm"
        );
    }

    // 7. account A and B do not share notification state; unattributed
    // state never attaches to a proven account
    #[test]
    fn accounts_have_independent_notification_state() {
        let harness = enabled_harness("accounts");
        let a = with_account(usage(79.0), "key:aaaa");
        harness.cycle(&[a]);

        let a = with_account(usage(81.0), "key:aaaa");
        let b = with_account(usage(82.0), "key:bbbb");
        let delivered = harness.cycle(&[a, b]);
        assert_eq!(delivered.len(), 1, "B's first observation is baseline, not a crossing");
        assert_eq!(delivered[0].account.as_deref(), Some("key:aaaa"));

        // B crosses on its own later and fires independently.
        let b = with_account(usage(79.0), "key:bbbb");
        harness.cycle(&[b]);
        let b = with_account(usage(81.0), "key:bbbb");
        let delivered = harness.cycle(&[b]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].account.as_deref(), Some("key:bbbb"));
        assert_eq!(harness.deliveries().len(), 2);

        // An unattributed window is a third identity entirely.
        let mut unattributed = usage(79.0);
        unattributed.id = "zai".to_string();
        harness.cycle(&[unattributed]);
        let mut attributed_now = usage(81.0);
        attributed_now.id = "zai".to_string();
        attributed_now.account = Some(AccountAttributionDto {
            label: "key ••3456".to_string(),
            note: None,
            identity: Some("key:3456".to_string()),
        });
        let delivered = harness.cycle(&[attributed_now]);
        assert!(
            delivered.is_empty(),
            "proven attribution must not inherit the unattributed baseline"
        );
    }

    // 8. stale usage does not notify (and never moves the baseline)
    #[test]
    fn stale_usage_does_not_notify() {
        let harness = enabled_harness("stale");
        harness.cycle(&[usage(79.0)]);
        // Both stale shapes: a stale status and a stale freshness verdict.
        assert!(harness
            .cycle(&[with_status(usage(95.0), "stale")])
            .is_empty());
        assert!(harness
            .cycle(&[with_freshness(usage(96.0), "stale")])
            .is_empty());
        let delivered = harness.cycle(&[usage(81.0)]);
        assert_eq!(delivered.len(), 1, "the stale observations never moved the baseline");
        assert_eq!(delivered[0].kind, ThresholdKind::NearLimit);
    }

    // 9. error-with-retained-last-good does not notify
    #[test]
    fn error_retained_last_good_does_not_notify() {
        let harness = enabled_harness("retained");
        harness.cycle(&[usage(79.0)]);
        let retained = with_status(usage(95.0), "error");
        assert!(harness.cycle(&[retained]).is_empty());
        let delivered = harness.cycle(&[usage(81.0)]);
        assert_eq!(delivered.len(), 1, "the errored cycle never moved the baseline");
    }

    // 10. unavailable (bare error) and unknown entries do not notify
    #[test]
    fn unavailable_and_unknown_do_not_notify() {
        let harness = enabled_harness("unavailable");
        let mut bare = usage(95.0);
        bare.limits.clear();
        let bare = with_status(bare, "error");
        let unknown = with_status(usage(96.0), "unknown");
        assert!(harness.cycle(&[bare, unknown]).is_empty());
        let blob = harness.persisted_blob();
        assert!(blob["windows"].as_array().unwrap().is_empty());
    }

    // 11. simulated providers cannot notify: the production registry that
    // feeds the lane carries none
    #[test]
    fn the_production_registry_feeding_notifications_has_no_simulated_providers() {
        let ids: Vec<&str> = crate::runtime::production_specs()
            .iter()
            .map(|spec| spec.kind.id())
            .collect();
        assert_eq!(
            ids,
            ["openai-codex", "zai", "opencode-go", "antigravity", "grok"]
        );
        let serialized = serde_json::to_string(&ids).unwrap();
        assert!(!serialized.contains("mock"), "no mock adapters in the notification path");
        assert!(!serialized.contains("simulated"), "no simulated providers in the notification path");
    }

    // 12. malformed windows do not notify and leave no state behind
    #[test]
    fn malformed_windows_do_not_notify() {
        let harness = enabled_harness("malformed");
        let mut blank_label = usage(85.0);
        blank_label.limits[0].label = "   ".to_string();
        let mut not_finite = usage(f64::NAN);
        not_finite.limits[0].label = "Broken".to_string();
        let good = usage(79.0);
        assert!(harness.cycle(&[blank_label, not_finite, good]).is_empty());
        let blob = harness.persisted_blob();
        let windows = blob["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 1, "only the well-formed window is tracked");
        assert_eq!(windows[0]["windowLabel"], "Weekly credits");
    }

    // 13. cooldown-retained usage does not notify (full runtime path)
    #[tokio::test]
    async fn runtime_cycles_drive_the_lane_and_cooldown_retention_stays_silent() {
        use crate::runtime::{ProviderFailure, ProviderKind, ProviderSpec, RuntimeCore};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = tempdir::temp_dir("runtime-lane");
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let lane = Arc::new(NotificationLane::open(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            Box::new(move |notes| {
                sink.lock().unwrap().extend_from_slice(notes);
            }),
        ));
        lane.set_enabled(true);

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_fetch = calls.clone();
        let spec = ProviderSpec {
            kind: ProviderKind::Codex,
            fetch: Arc::new(move || {
                let nth = calls_fetch.fetch_add(1, Ordering::SeqCst) + 1;
                let outcome = match nth {
                    1 => Ok(usage(79.0)),
                    2 => Ok(usage(81.0)),
                    _ => Err(ProviderFailure {
                        code: "unexpected_response".to_string(),
                        message: "rate limited".to_string(),
                        http_status: Some(429),
                        transient: Some(true),
                        retry_after_ms: Some(60_000),
                        identity: None,
                        transport_timeout: false,
                    }),
                };
                Box::pin(async move { outcome }) as crate::runtime::FetchFuture
            }),
        };
        let core = Arc::new(
            RuntimeCore::with_injections(vec![spec], 5, Box::new(|| 0), Box::new(fixed_now))
                .with_notification_lane(Some(lane)),
        );
        core.run_cycle().await; // 79: baseline
        core.run_cycle().await; // 81: near-limit fires
        core.run_cycle().await; // 429 + Retry-After: retained last-good, errored
        core.run_cycle().await; // cooldown skip: still the retained error
        assert_eq!(calls.load(Ordering::SeqCst), 3, "the cooldown skipped the fourth fetch");
        assert_eq!(
            delivered.lock().unwrap().len(),
            1,
            "exactly the genuine crossing notified"
        );
        assert_eq!(core.snapshot().providers[0].status, "error");
    }

    // 14. restart: persisted state does not duplicate the current-cycle
    // notification, and the enabled toggle persists
    #[test]
    fn restart_persisted_state_does_not_duplicate_and_keeps_the_toggle() {
        let harness = enabled_harness("restart");
        harness.cycle(&[usage(79.0)]);
        harness.cycle(&[usage(81.0)]);

        let reopened = harness.reopen();
        assert!(reopened.lane.is_enabled(), "the toggle survives the restart");
        assert!(reopened.cycle(&[usage(82.0)]).is_empty(), "same cycle: no duplicate");
        assert!(reopened.deliveries().is_empty());
    }

    // 15. a corrupt notification-state file self-heals safely
    #[test]
    fn corrupt_state_file_self_heals() {
        let dir = tempdir::temp_dir("corrupt-state");
        std::fs::write(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            "{not json at all",
        )
        .unwrap();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let lane = NotificationLane::open(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            Box::new(move |notes| {
                sink.lock().unwrap().extend_from_slice(notes);
            }),
        );
        lane.set_enabled(true);
        let mut broken = usage(79.0);
        broken.limits[0].used_percent = 0.0; // nothing crossing yet
        lane.process(&[broken], fixed_now());
        let mut crossing = usage(79.0);
        crossing.limits[0].used_percent = 81.0;
        let notes = lane.process(std::slice::from_ref(&crossing), fixed_now());
        assert_eq!(notes.len(), 1, "the lane works after corruption");

        // The healed state was written back: a restart does not re-notify.
        let reopened_deliveries = Arc::new(Mutex::new(Vec::new()));
        let reopened_sink = reopened_deliveries.clone();
        let reopened = NotificationLane::open(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            Box::new(move |notes| {
                reopened_sink.lock().unwrap().extend_from_slice(notes);
            }),
        );
        reopened.process(&[crossing], fixed_now());
        assert!(reopened_deliveries.lock().unwrap().is_empty());

        // A foreign version degrades to a disabled empty lane, too.
        std::fs::write(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            serde_json::json!({"version": 99, "enabled": true, "windows": []}).to_string(),
        )
        .unwrap();
        let foreign = NotificationLane::open(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            Box::new(|_| {}),
        );
        assert!(!foreign.is_enabled(), "unknown layout never blends in");
    }

    // 16. notification state is bounded: the oldest entries are pruned
    #[test]
    fn state_is_bounded_and_prunes_the_oldest_windows() {
        let harness = enabled_harness("bounded");
        // One hundred baseline updates in one cycle: far past the cap. All
        // entries share the LRU stamp, so the documented identity tie-break
        // keeps the lowest keys.
        let flood: Vec<ProviderUsageDto> = (0..100)
            .map(|index| named_usage(&format!("provider-{index:03}"), "Provider", 10.0))
            .collect();
        harness.cycle(&flood);
        let blob = harness.persisted_blob();
        let windows = blob["windows"].as_array().unwrap();
        assert_eq!(
            windows.len(),
            NOTIFICATION_STATE_MAX_WINDOWS,
            "the map is bounded to {} entries",
            NOTIFICATION_STATE_MAX_WINDOWS
        );
        let ids: Vec<&str> = windows
            .iter()
            .map(|entry| entry["providerId"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"provider-000"), "the lowest-identity entries survive");
        assert!(!ids.contains(&"provider-099"), "the overflow entries are pruned");
    }

    // 17. the per-cycle burst cap is enforced
    #[test]
    fn the_burst_cap_limits_notifications_per_cycle() {
        let harness = enabled_harness("burst");
        let before: Vec<ProviderUsageDto> = (0..5)
            .map(|index| named_usage(&format!("provider-{index}"), "Provider", 94.0))
            .collect();
        harness.cycle(&before);
        let after: Vec<ProviderUsageDto> = (0..5)
            .map(|index| named_usage(&format!("provider-{index}"), "Provider", 96.0))
            .collect();
        let delivered = harness.cycle(&after);
        assert_eq!(delivered.len(), MAX_NOTIFICATIONS_PER_CYCLE);
        assert!(delivered
            .iter()
            .all(|note| note.kind == ThresholdKind::Critical));
        // Only the delivered windows are marked fired; the capped-out ones
        // stay unmarked for a later genuine crossing.
        let fired: Vec<usize> = harness.persisted_blob()["windows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| !entry["fired"].as_array().unwrap().is_empty())
            .map(|entry| {
                let id = entry["providerId"].as_str().unwrap();
                id.trim_start_matches("provider-").parse().unwrap()
            })
            .collect();
        assert_eq!(fired.len(), MAX_NOTIFICATIONS_PER_CYCLE);
    }

    // 18. severity and ordering are deterministic (and the cap cuts last)
    #[test]
    fn severity_and_ordering_are_deterministic() {
        let harness = enabled_harness("ordering");
        let before = vec![
            named_usage("p1", "One", 94.0),
            named_usage("p2", "Two", 79.0),
            named_usage("p3", "Three", 79.0),
            named_usage("p4", "Four", 79.0),
        ];
        harness.cycle(&before);
        let after = vec![
            named_usage("p1", "One", 96.0), // critical, 96
            named_usage("p2", "Two", 85.0), // near, 85
            named_usage("p3", "Three", 85.0), // near, 85 (identity tie-break)
            named_usage("p4", "Four", 81.0), // near, 81 — beyond the cap
        ];
        let delivered = harness.cycle(&after);
        let rendered: Vec<(ThresholdKind, &str)> = delivered
            .iter()
            .map(|note| (note.kind, note.provider_id.as_str()))
            .collect();
        assert_eq!(
            rendered,
            vec![
                (ThresholdKind::Critical, "p1"),
                (ThresholdKind::NearLimit, "p2"),
                (ThresholdKind::NearLimit, "p3"),
            ],
            "critical first, then highest usage, then identity order; the cap drops p4"
        );
    }

    // 19. the disabled toggle suppresses delivery
    #[test]
    fn the_disabled_toggle_suppresses_delivery() {
        let harness = harness("disabled"); // default off, like autostart
        assert!(harness.cycle(&[usage(79.0)]).is_empty());
        assert!(harness.cycle(&[usage(81.0)]).is_empty());
        assert!(harness.cycle(&[usage(96.0)]).is_empty());
        assert!(harness.deliveries().is_empty());
        assert!(!harness.lane.is_enabled());
    }

    // 20. re-enabling does not retroactively spam already-crossed
    // thresholds; only a fresh crossing notifies
    #[test]
    fn re_enabling_does_not_retro_spam_already_crossed_thresholds() {
        let harness = harness("re-enable");
        harness.cycle(&[usage(79.0)]);
        harness.cycle(&[usage(85.0)]); // near crossed while disabled
        harness.cycle(&[usage(96.0)]); // critical crossed while disabled
        harness.lane.set_enabled(true);
        assert!(
            harness.cycle(&[usage(97.0)]).is_empty(),
            "already-crossed thresholds stay silent after re-enabling"
        );
        // A genuine crossing after re-enabling still fires.
        harness.cycle(&[usage(70.0)]);
        let delivered = harness.cycle(&[usage(81.0)]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].kind, ThresholdKind::NearLimit);
        assert_eq!(harness.deliveries().len(), 1);
    }

    // wire parity: the notification payload is stable, display-safe JSON
    #[test]
    fn notification_payload_serializes_to_the_documented_shape() {
        let note = QuotaNotification {
            kind: ThresholdKind::NearLimit,
            provider_id: "opencode-go".to_string(),
            provider_name: "OpenCode Go".to_string(),
            window_label: "5-hour".to_string(),
            used_percent: 81.0,
            account: Some("key:3456".to_string()),
            reset_at: Some(RESET_LATER.to_string()),
        };
        let wire = serde_json::to_value(&note).unwrap();
        assert_eq!(wire["kind"], "nearLimit");
        assert_eq!(wire["providerName"], "OpenCode Go");
        assert_eq!(wire["usedPercent"], 81.0);
        let critical = QuotaNotification {
            kind: ThresholdKind::Critical,
            ..note
        };
        assert_eq!(serde_json::to_value(&critical).unwrap()["kind"], "critical");
    }

    // copy support: the resets-in humanizer
    #[test]
    fn resets_in_humanizes_remaining_time() {
        let in_four_hours = (fixed_now() + chrono::Duration::hours(4) + chrono::Duration::minutes(12))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        assert_eq!(
            resets_in_from(Some(&in_four_hours), fixed_now()),
            Some("4h 12m".to_string())
        );
        let in_two_days = (fixed_now() + chrono::Duration::days(2) + chrono::Duration::hours(3))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        assert_eq!(
            resets_in_from(Some(&in_two_days), fixed_now()),
            Some("2d 3h".to_string())
        );
        let in_minutes = (fixed_now() + chrono::Duration::minutes(35))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        assert_eq!(
            resets_in_from(Some(&in_minutes), fixed_now()),
            Some("35m".to_string())
        );
        let past = (fixed_now() - chrono::Duration::minutes(1))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        assert_eq!(resets_in_from(Some(&past), fixed_now()), None, "a past reset renders no countdown");
        assert_eq!(resets_in(Some("not-a-timestamp")), None);
        assert_eq!(resets_in(None), None);
    }

    // ---------- v0.6 recovery ----------
    const RESET_EARLIER: &str = "2026-09-28T08:00:00.000Z";

    fn with_label(mut usage: ProviderUsageDto, label: &str) -> ProviderUsageDto {
        usage.limits[0].label = label.to_string();
        usage
    }

    // 1. 84% -> new reset cycle 3% triggers recovery
    #[test]
    fn recovery_fires_when_constrained_cycle_resets_to_low_usage() {
        let harness = enabled_harness("recovery-basic");
        harness.cycle(&[with_reset_at(usage(84.0), RESET_LATER)]);
        let delivered = harness.cycle(&[with_reset_at(usage(3.0), RESET_NEXT)]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].kind, ThresholdKind::Recovery);
        assert_eq!(delivered[0].used_percent, 3.0);
        assert_eq!(delivered[0].reset_at.as_deref(), Some(RESET_NEXT));
    }

    // 2. 97% -> new reset 12% triggers recovery
    #[test]
    fn recovery_fires_for_critical_then_low_new_cycle() {
        let harness = enabled_harness("recovery-critical");
        harness.cycle(&[with_reset_at(usage(97.0), RESET_LATER)]);
        let delivered = harness.cycle(&[with_reset_at(usage(12.0), RESET_NEXT)]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].kind, ThresholdKind::Recovery);
    }

    // 3. 79% -> new cycle 3% does not trigger (previous not constrained)
    #[test]
    fn recovery_requires_constrained_previous_cycle() {
        let harness = enabled_harness("recovery-prev-threshold");
        harness.cycle(&[with_reset_at(usage(79.0), RESET_LATER)]);
        assert!(harness
            .cycle(&[with_reset_at(usage(3.0), RESET_NEXT)])
            .is_empty());
    }

    // 4. 96% -> same resetAt 4% does not trigger (no reset evidence)
    #[test]
    fn recovery_requires_reset_transition_not_just_a_drop() {
        let harness = enabled_harness("recovery-no-reset");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        assert!(harness
            .cycle(&[with_reset_at(usage(4.0), RESET_LATER)])
            .is_empty());
    }

    // 5. 96% -> new resetAt 87% does not trigger (still constrained)
    #[test]
    fn recovery_requires_new_cycle_below_near_limit() {
        let harness = enabled_harness("recovery-still-high");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        assert!(harness
            .cycle(&[with_reset_at(usage(87.0), RESET_NEXT)])
            .is_empty());
    }

    // 6. recovery fires once per new cycle
    #[test]
    fn recovery_fires_once_per_new_cycle() {
        let harness = enabled_harness("recovery-once");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        assert_eq!(
            harness
                .cycle(&[with_reset_at(usage(4.0), RESET_NEXT)])
                .len(),
            1
        );
        assert!(harness
            .cycle(&[with_reset_at(usage(5.0), RESET_NEXT)])
            .is_empty());
        assert!(harness
            .cycle(&[with_reset_at(usage(6.0), RESET_NEXT)])
            .is_empty());
        assert_eq!(harness.deliveries().len(), 1);
    }

    // 7. restart does not duplicate same-cycle recovery
    #[test]
    fn recovery_restart_does_not_duplicate() {
        let harness = enabled_harness("recovery-restart");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        harness.cycle(&[with_reset_at(usage(4.0), RESET_NEXT)]);
        let reopened = harness.reopen();
        assert!(reopened.lane.is_enabled());
        assert!(reopened
            .cycle(&[with_reset_at(usage(5.0), RESET_NEXT)])
            .is_empty());
        assert!(reopened.deliveries().is_empty());
    }

    // 8. next genuine reset can notify again
    #[test]
    fn recovery_re_arms_on_next_genuine_reset() {
        let harness = enabled_harness("recovery-rearm");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        assert_eq!(
            harness
                .cycle(&[with_reset_at(usage(4.0), RESET_NEXT)])
                .len(),
            1
        );
        // New cycle B peaks low (4-30): next reset must NOT notify.
        harness.cycle(&[with_reset_at(usage(30.0), RESET_NEXT)]);
        const RESET_THIRD: &str = "2026-10-12T16:00:00.000Z";
        assert!(harness
            .cycle(&[with_reset_at(usage(5.0), RESET_THIRD)])
            .is_empty());
        // Constrain cycle C (90), then reset to D: notifies again.
        harness.cycle(&[with_reset_at(usage(90.0), RESET_THIRD)]);
        const RESET_FOURTH: &str = "2026-10-19T16:00:00.000Z";
        let delivered = harness.cycle(&[with_reset_at(usage(6.0), RESET_FOURTH)]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].kind, ThresholdKind::Recovery);
    }

    // 9. account A/B isolated
    #[test]
    fn recovery_account_isolation() {
        let harness = enabled_harness("recovery-accounts");
        harness.cycle(&[with_account(with_reset_at(usage(96.0), RESET_LATER), "key:aaaa")]);
        // B never constrained: its reset must not notify, and must not
        // inherit A's constrained peak.
        harness.cycle(&[with_account(with_reset_at(usage(10.0), RESET_LATER), "key:bbbb")]);
        let delivered = harness.cycle(&[
            with_account(with_reset_at(usage(4.0), RESET_NEXT), "key:aaaa"),
            with_account(with_reset_at(usage(5.0), RESET_NEXT), "key:bbbb"),
        ]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].account.as_deref(), Some("key:aaaa"));
        assert_eq!(delivered[0].kind, ThresholdKind::Recovery);
    }

    // 10. window A/B isolated
    #[test]
    fn recovery_window_isolation() {
        let harness = enabled_harness("recovery-windows");
        harness.cycle(&[with_label(with_reset_at(usage(96.0), RESET_LATER), "Weekly credits")]);
        harness.cycle(&[with_label(with_reset_at(usage(10.0), RESET_LATER), "5-hour")]);
        let delivered = harness.cycle(&[
            with_label(with_reset_at(usage(4.0), RESET_NEXT), "Weekly credits"),
            with_label(with_reset_at(usage(5.0), RESET_NEXT), "5-hour"),
        ]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].window_label, "Weekly credits");
    }

    // 11. stale new-cycle data does not notify
    #[test]
    fn recovery_stale_does_not_notify() {
        let harness = enabled_harness("recovery-stale");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        let stale = with_status(with_reset_at(usage(4.0), RESET_NEXT), "stale");
        assert!(harness.cycle(&[stale]).is_empty());
        // Baseline never moved: a genuine low observation still recovers.
        let delivered = harness.cycle(&[with_reset_at(usage(4.0), RESET_NEXT)]);
        assert_eq!(delivered.len(), 1);
    }

    // 12. error-retained-last-good does not notify
    #[test]
    fn recovery_error_retained_does_not_notify() {
        let harness = enabled_harness("recovery-error");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        let retained = with_status(with_reset_at(usage(4.0), RESET_NEXT), "error");
        assert!(harness.cycle(&[retained]).is_empty());
        let delivered = harness.cycle(&[with_reset_at(usage(4.0), RESET_NEXT)]);
        assert_eq!(delivered.len(), 1);
    }

    // 13. cooldown projection does not notify (error-shaped skip)
    #[test]
    fn recovery_cooldown_retained_does_not_notify() {
        let harness = enabled_harness("recovery-cooldown");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        // Canonical cooldown projection (v0.6 runtime status contract):
        // health `cooldown` with the legacy status derived as "error" —
        // exactly what the runtime's cooldown skip surfaces.
        let mut cooldown = with_reset_at(usage(4.0), RESET_NEXT);
        cooldown.health = crate::runtime::ProviderHealth::Cooldown;
        cooldown.status = crate::runtime::ProviderHealth::Cooldown.legacy_status();
        cooldown.data_freshness = None;
        assert!(harness.cycle(&[cooldown]).is_empty());
        assert!(harness.deliveries().is_empty());
    }

    // 14. unavailable (bare error, unknown) does not notify
    #[test]
    fn recovery_unavailable_does_not_notify() {
        let harness = enabled_harness("recovery-unavail");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        let mut bare = with_reset_at(usage(4.0), RESET_NEXT);
        bare.limits.clear();
        let bare = with_status(bare, "error");
        let unknown = with_status(with_reset_at(usage(4.0), RESET_NEXT), "unknown");
        assert!(harness.cycle(&[bare, unknown]).is_empty());
    }

    // 15. malformed resetAt does not notify
    #[test]
    fn recovery_malformed_reset_does_not_notify() {
        let harness = enabled_harness("recovery-malformed-reset");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        assert!(harness
            .cycle(&[with_reset_at(usage(4.0), "not-a-timestamp")])
            .is_empty());
    }

    // 16. missing resetAt does not notify
    #[test]
    fn recovery_missing_reset_does_not_notify() {
        let harness = enabled_harness("recovery-missing-reset");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        let mut no_reset = usage(4.0);
        no_reset.limits[0].reset_at = None;
        assert!(harness.cycle(&[no_reset]).is_empty());
    }

    // 17. first observation after app install does not notify
    #[test]
    fn recovery_first_observation_never_notifies() {
        let harness = enabled_harness("recovery-first");
        assert!(harness
            .cycle(&[with_reset_at(usage(3.0), RESET_LATER)])
            .is_empty());
    }

    // 18. first observation after state corruption does not notify
    #[test]
    fn recovery_after_corruption_starts_from_baseline() {
        let dir = tempdir::temp_dir("recovery-corrupt");
        std::fs::write(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            "{not json at all",
        )
        .unwrap();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let lane = NotificationLane::open(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            Box::new(move |notes| {
                sink.lock().unwrap().extend_from_slice(notes);
            }),
        );
        lane.set_enabled(true);
        // Single low observation with a valid reset: first sight, baseline.
        assert!(lane
            .process(
                &[with_reset_at(usage(3.0), RESET_LATER)],
                fixed_now()
            )
            .is_empty());
    }

    // 19. toggle OFF suppresses recovery
    #[test]
    fn recovery_disabled_toggle_suppresses() {
        let harness = harness("recovery-disabled");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        assert!(harness
            .cycle(&[with_reset_at(usage(4.0), RESET_NEXT)])
            .is_empty());
        assert!(harness.deliveries().is_empty());
    }

    // 20. re-enable does not retroactively fire old recovery
    #[test]
    fn recovery_re_enable_does_not_retro_fire() {
        let harness = harness("recovery-reenable");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        // Reset happened while disabled: opportunity consumed silently.
        harness.cycle(&[with_reset_at(usage(4.0), RESET_NEXT)]);
        harness.lane.set_enabled(true);
        assert!(harness
            .cycle(&[with_reset_at(usage(5.0), RESET_NEXT)])
            .is_empty());
        assert!(harness.deliveries().is_empty());
        // A fresh constrained cycle followed by a genuine reset still works.
        harness.cycle(&[with_reset_at(usage(90.0), RESET_NEXT)]);
        const RESET_R: &str = "2026-10-20T16:00:00.000Z";
        let delivered = harness.cycle(&[with_reset_at(usage(4.0), RESET_R)]);
        assert_eq!(delivered.len(), 1);
    }

    // 21. near/critical behavior unchanged with recovery fields present
    #[test]
    fn recovery_does_not_change_threshold_crossings() {
        let harness = enabled_harness("recovery-thresholds-intact");
        harness.cycle(&[usage(79.0)]);
        let near = harness.cycle(&[usage(81.0)]);
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].kind, ThresholdKind::NearLimit);
        harness.cycle(&[usage(82.0)]);
        let critical = harness.cycle(&[usage(96.0)]);
        assert_eq!(critical.len(), 1);
        assert_eq!(critical[0].kind, ThresholdKind::Critical);
    }

    // 22. full lifecycle: near -> critical -> recovery -> near in next cycle
    #[test]
    fn recovery_full_lifecycle() {
        let harness = enabled_harness("recovery-lifecycle");
        harness.cycle(&[with_reset_at(usage(79.0), RESET_LATER)]);
        let near = harness.cycle(&[with_reset_at(usage(82.0), RESET_LATER)]);
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].kind, ThresholdKind::NearLimit);
        let critical = harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        assert_eq!(critical.len(), 1);
        assert_eq!(critical[0].kind, ThresholdKind::Critical);
        assert!(harness
            .cycle(&[with_reset_at(usage(96.0), RESET_LATER)])
            .is_empty());
        let recovery = harness.cycle(&[with_reset_at(usage(4.0), RESET_NEXT)]);
        assert_eq!(recovery.len(), 1);
        assert_eq!(recovery[0].kind, ThresholdKind::Recovery);
        assert!(harness
            .cycle(&[with_reset_at(usage(30.0), RESET_NEXT)])
            .is_empty());
        let near_again = harness.cycle(&[with_reset_at(usage(81.0), RESET_NEXT)]);
        assert_eq!(near_again.len(), 1);
        assert_eq!(near_again[0].kind, ThresholdKind::NearLimit);
    }

    // 23. burst cap shared with threshold notifications (recovery lowest priority)
    #[test]
    fn recovery_shares_burst_cap() {
        let harness = enabled_harness("recovery-burst");
        // Four windows constrained in cycle A.
        let before: Vec<ProviderUsageDto> = (0..4)
            .map(|i| with_reset_at(named_usage(&format!("provider-{i}"), "Provider", 96.0), RESET_LATER))
            .collect();
        harness.cycle(&before);
        // All four reset low: only 3 recoveries fit the cap.
        let after: Vec<ProviderUsageDto> = (0..4)
            .map(|i| with_reset_at(named_usage(&format!("provider-{i}"), "Provider", 4.0), RESET_NEXT))
            .collect();
        let delivered = harness.cycle(&after);
        assert_eq!(delivered.len(), MAX_NOTIFICATIONS_PER_CYCLE);
        assert!(delivered
            .iter()
            .all(|n| n.kind == ThresholdKind::Recovery));
    }

    // 24. deterministic severity/order: critical, near, recovery
    #[test]
    fn recovery_burst_ordering_is_deterministic() {
        let harness = enabled_harness("recovery-order");
        // p1: threshold critical crossing (94->96, same reset).
        // p2: threshold near crossing (79->85, same reset).
        // p3: recovery (96 old cycle -> 4 new cycle).
        // p4: second recovery (lower priority, beyond cap).
        harness.cycle(&[
            with_reset_at(named_usage("p1", "One", 94.0), RESET_LATER),
            with_reset_at(named_usage("p2", "Two", 79.0), RESET_LATER),
            with_reset_at(named_usage("p3", "Three", 96.0), RESET_LATER),
            with_reset_at(named_usage("p4", "Four", 97.0), RESET_LATER),
        ]);
        let delivered = harness.cycle(&[
            with_reset_at(named_usage("p1", "One", 96.0), RESET_LATER),
            with_reset_at(named_usage("p2", "Two", 85.0), RESET_LATER),
            with_reset_at(named_usage("p3", "Three", 4.0), RESET_NEXT),
            with_reset_at(named_usage("p4", "Four", 5.0), RESET_NEXT),
        ]);
        assert_eq!(delivered.len(), 3);
        assert_eq!(delivered[0].kind, ThresholdKind::Critical);
        assert_eq!(delivered[0].provider_id, "p1");
        assert_eq!(delivered[1].kind, ThresholdKind::NearLimit);
        assert_eq!(delivered[1].provider_id, "p2");
        // Among recoveries the higher usage wins deterministically, so p4
        // (5%) outranks p3 (4%); the cap drops p3.
        assert_eq!(delivered[2].kind, ThresholdKind::Recovery);
        assert_eq!(delivered[2].provider_id, "p4");
    }

    // 25. persisted state remains bounded with recovery traffic
    #[test]
    fn recovery_state_remains_bounded() {
        let harness = enabled_harness("recovery-bounded");
        let flood: Vec<ProviderUsageDto> = (0..100)
            .map(|i| with_reset_at(named_usage(&format!("provider-{i:03}"), "Provider", 96.0), RESET_LATER))
            .collect();
        harness.cycle(&flood);
        let flood_next: Vec<ProviderUsageDto> = (0..100)
            .map(|i| with_reset_at(named_usage(&format!("provider-{i:03}"), "Provider", 4.0), RESET_NEXT))
            .collect();
        harness.cycle(&flood_next);
        let blob = harness.persisted_blob();
        let windows = blob["windows"].as_array().unwrap();
        assert_eq!(windows.len(), NOTIFICATION_STATE_MAX_WINDOWS);
    }

    // 26. legacy v0.5 notification-state file loads safely and still recovers
    #[test]
    fn legacy_v05_state_loads_and_recovers_via_last_percent_fallback() {
        let dir = tempdir::temp_dir("recovery-legacy");
        // v0.5 shape: no cycle_peak/prev_peak/prev_reset_at, only thresholds.
        let legacy = serde_json::json!({
            "version": 1,
            "enabled": true,
            "windows": [{
                "providerId": "openai-codex",
                "windowLabel": "Weekly credits",
                "resetAt": RESET_LATER,
                "lastPercent": 96.0,
                "fired": ["critical"],
                "updatedAtMs": NOW_MS
            }]
        });
        std::fs::write(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            legacy.to_string(),
        )
        .unwrap();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let lane = NotificationLane::open(
            dir.path().join(NOTIFICATIONS_FILE_NAME),
            Box::new(move |notes| {
                sink.lock().unwrap().extend_from_slice(notes);
            }),
        );
        assert!(lane.is_enabled());
        // New genuine reset to low usage: prev peak falls back to lastPercent
        // (96 >= 80), so recovery fires even though v0.5 never tracked peaks.
        let notes = lane.process(
            &[with_reset_at(usage(4.0), RESET_NEXT)],
            fixed_now(),
        );
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, ThresholdKind::Recovery);
    }

    // backward reset (earlier resetAt) re-arms thresholds but never recovers
    #[test]
    fn recovery_backward_reset_never_notifies() {
        let harness = enabled_harness("recovery-backward");
        harness.cycle(&[with_reset_at(usage(96.0), RESET_LATER)]);
        assert!(harness
            .cycle(&[with_reset_at(usage(4.0), RESET_EARLIER)])
            .is_empty());
    }

    // recovery copy is factual and compact
    #[test]
    fn recovery_copy_is_factual() {
        let (title, body) = notification_copy("Grok", "Weekly credits", ThresholdKind::Recovery, 4.0, Some(RESET_NEXT));
        assert_eq!(title, "Grok quota available again");
        assert_eq!(body, "Weekly credits reset · 4% used");
        assert!(!title.contains("safe"));
        assert!(!body.contains("Switch back"));
    }

    // secret audit: persisted state carries no credential material
    #[test]
    fn recovery_persisted_state_carries_no_secrets() {
        let harness = enabled_harness("recovery-secrets");
        harness.cycle(&[with_account(with_reset_at(usage(96.0), RESET_LATER), "key:3456")]);
        harness.cycle(&[with_account(with_reset_at(usage(4.0), RESET_NEXT), "key:3456")]);
        let raw = std::fs::read_to_string(harness.dir.path().join(NOTIFICATIONS_FILE_NAME)).unwrap();
        let lower = raw.to_lowercase();
        for forbidden in ["bearer", "jwt", "api_key", "apikey", "cookie", "credential", "secret", "password"] {
            assert!(!lower.contains(forbidden), "persisted state must not contain {forbidden}");
        }
        assert!(raw.contains("key:3456"), "masked identity token is preserved");
    }
}
