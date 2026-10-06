//! Redacted runtime diagnostics and support-bundle export.
//!
//! This module is the only diagnostics serialization boundary. It receives
//! normalized runtime DTOs and typed settings, constructs a dedicated safe
//! bundle schema, sanitizes every free-form string, and never accepts or
//! serializes provider response payloads. The webview supplies only a small,
/// whitelisted settings record and cannot choose a filesystem path.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::history::QuotaObservation;
use crate::runtime::{ProviderUsageDto, RuntimeHandle};

pub const DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;
const MAX_SAFE_TEXT: usize = 512;
const MAX_BUNDLE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct ErrorDiagnosticSource {
    pub code: String,
    pub message: String,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderDiagnosticSource {
    pub usage: ProviderUsageDto,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<ErrorDiagnosticSource>,
    pub last_error_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeDiagnosticSource {
    pub snapshot_seq: u64,
    pub last_cycle_started_at: Option<DateTime<Utc>>,
    pub last_cycle_completed_at: Option<DateTime<Utc>>,
    pub last_usable_data_at: Option<DateTime<Utc>>,
    pub refresh_interval_minutes: u64,
    pub cycle_in_flight: bool,
    pub follow_up_pending: bool,
    pub providers: Vec<ProviderDiagnosticSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticsSettingsInput {
    pub theme: String,
    pub launch_at_startup: bool,
    pub notification_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppDiagnostics {
    product_name: String,
    version: String,
    platform: &'static str,
    architecture: &'static str,
    app_identifier: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentDiagnostics {
    os_family: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    os_version: Option<String>,
    app_data_health: &'static str,
    runtime_provider_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeDiagnostics {
    snapshot_seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_cycle_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_cycle_completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_usable_data_at: Option<String>,
    refresh_interval_minutes: u64,
    scheduler_state: &'static str,
    follow_up_pending: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderWindowDiagnostics {
    label: String,
    used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorDiagnostics {
    category: &'static str,
    code: String,
    message: String,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDiagnostics {
    provider_id: String,
    /// Canonical normalized health (v0.6 runtime status contract) — the
    /// authoritative state word in the bundle.
    health: String,
    /// Legacy status projection, kept for bundle-reader continuity; always
    /// derived from the same health value, never independent.
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    freshness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    masked_account_identity: Option<String>,
    windows: Vec<ProviderWindowDiagnostics>,
    /// Optional Codex-only banked reset-credit observation (v0.7): counts
    /// and observation time only — never raw payloads, identifiers, or
    /// redemption semantics.
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_credits: Option<ResetCreditDiagnostics>,
    cooldown_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cooldown_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_success_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<ErrorDiagnostics>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetCreditDiagnostics {
    banked_credits: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    currently_applicable: Option<u32>,
    checked_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryWindowCount {
    provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    window_label: String,
    count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryDiagnostics {
    revision: u64,
    total_observations: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    oldest_observation_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    newest_observation_at: Option<String>,
    observation_counts: Vec<HistoryWindowCount>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationDiagnostics {
    enabled: bool,
    dedup_entry_count: usize,
    active_current_cycle_state_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsDiagnostics {
    theme: String,
    refresh_interval_minutes: u64,
    launch_at_startup: bool,
    notification_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsBundle {
    schema_version: u32,
    generated_at: String,
    app: AppDiagnostics,
    environment: EnvironmentDiagnostics,
    runtime: RuntimeDiagnostics,
    providers: Vec<ProviderDiagnostics>,
    history: HistoryDiagnostics,
    notifications: NotificationDiagnostics,
    settings: SettingsDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDiagnosticsResult {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

fn sanitizer_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]+",
            r"(?i)\b(?:authorization|proxy-authorization|cookie|set-cookie|refresh[_-]?token|access[_-]?token|api[_-]?key|client[_-]?secret|session[_-]?id|password|passwd|private[_-]?key|credential|secret)\b\s*[:=]?\s*[^\r\n,;]+",
            r"\bsk-[A-Za-z0-9_-]{4,}",
            r"\beyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b",
            r"(?i)(?:[A-Z]:[\\/]|\\\\)[^\r\n,;]+",
            r"(?i)/(?:home|users|root)/[^\s,;]+",
            r"[A-Za-z0-9_+/.=-]{32,}",
            r"(?i)\b(?:authorization|proxy-authorization|cookie|set-cookie|refresh[_-]?token|access[_-]?token|api[_-]?key|client[_-]?secret|session[_-]?id|password|passwd|private[_-]?key|credential|secret|bearer|basic)\b",
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).expect("diagnostics sanitizer regex"))
        .collect()
    })
}

/// Explicit last-line defense for free-form normalized error text. Typed
/// fields are already narrow; this removes credential shapes and sensitive
/// labels even when a provider error message embeds them.
pub(crate) fn sanitize_text(value: &str) -> String {
    let mut sanitized = value
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .collect::<String>();
    for pattern in sanitizer_patterns() {
        sanitized = pattern.replace_all(&sanitized, "[REDACTED]").into_owned();
    }
    sanitized
        .chars()
        .take(MAX_SAFE_TEXT)
        .collect::<String>()
        .trim()
        .to_string()
}

fn safe_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn safe_optional_timestamp(value: Option<&str>) -> Option<String> {
    let parsed = DateTime::parse_from_rfc3339(value?).ok()?;
    Some(
        parsed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

fn safe_theme(value: &str) -> String {
    match value {
        "graphite" | "glass" | "oled" => value.to_string(),
        _ => "unknown".to_string(),
    }
}

fn error_category(failure: &ErrorDiagnosticSource) -> &'static str {
    if failure.code.starts_with("credential_") {
        "credential"
    } else if failure.http_status == Some(429) {
        "rateLimit"
    } else if failure
        .http_status
        .map(|status| (500..=599).contains(&status))
        .unwrap_or(false)
    {
        "server"
    } else if failure.code == "network" {
        "network"
    } else {
        "provider"
    }
}

/// Provider error text may quote an upstream response, so the export never
/// carries it. Category/code/status are retained separately; the message is a
/// fixed local sentence with no provider-derived content.
fn safe_error_message(untrusted_provider_text: &str) -> &'static str {
    // Explicitly consume and discard upstream text; it is diagnostic input,
    // never exportable output.
    let _ = untrusted_provider_text;
    "Provider refresh failed."
}

fn normalize_provider(source: ProviderDiagnosticSource, now: DateTime<Utc>) -> ProviderDiagnostics {
    let ProviderUsageDto {
        id,
        health,
        status,
        limits,
        account,
        data_freshness,
        reset_credits,
        ..
    } = source.usage;
    let windows = limits
        .into_iter()
        .filter(|window| window.used_percent.is_finite())
        .map(|window| ProviderWindowDiagnostics {
            label: sanitize_text(&window.label),
            used_percent: window.used_percent.clamp(0.0, 100.0),
            reset_at: safe_optional_timestamp(window.reset_at.as_deref()),
        })
        .filter(|window| !window.label.is_empty())
        .collect();
    let cooldown_until = source.cooldown_until.filter(|until| now < *until);
    let last_error = source.last_error.map(|failure| ErrorDiagnostics {
        category: error_category(&failure),
        code: sanitize_text(&failure.code),
        message: safe_error_message(&failure.message).to_string(),
        timestamp: source
            .last_error_at
            .map(safe_timestamp)
            .unwrap_or_else(|| "unknown".to_string()),
        http_status: failure.http_status,
    });
    ProviderDiagnostics {
        provider_id: sanitize_text(&id),
        health: health.as_str().to_string(),
        status: sanitize_text(status),
        freshness: data_freshness.map(sanitize_text),
        masked_account_identity: account.map(|account| sanitize_text(&account.label)),
        windows,
        reset_credits: reset_credits.map(|credits| ResetCreditDiagnostics {
            banked_credits: credits.banked_credits,
            currently_applicable: credits.currently_applicable,
            checked_at: safe_optional_timestamp(Some(&credits.checked_at))
                .unwrap_or_else(|| "unknown".to_string()),
        }),
        cooldown_active: cooldown_until.is_some(),
        cooldown_until: cooldown_until.map(safe_timestamp),
        last_success_at: source.last_success_at.map(safe_timestamp),
        last_error,
    }
}

fn history_summary(revision: u64, observations: Vec<QuotaObservation>) -> HistoryDiagnostics {
    let mut counts: BTreeMap<(String, Option<String>, String), usize> = BTreeMap::new();
    let mut oldest = None;
    let mut newest = None;
    for observation in observations {
        let observed_at = safe_optional_timestamp(Some(&observation.observed_at));
        if let Some(timestamp) = &observed_at {
            if oldest.as_ref().map_or(true, |current| timestamp < current) {
                oldest = Some(timestamp.clone());
            }
            if newest.as_ref().map_or(true, |current| timestamp > current) {
                newest = Some(timestamp.clone());
            }
        }
        *counts
            .entry((
                sanitize_text(&observation.provider_id),
                observation.account.map(|account| sanitize_text(&account)),
                sanitize_text(&observation.window_label),
            ))
            .or_default() += 1;
    }
    let total_observations = counts.values().sum();
    HistoryDiagnostics {
        revision,
        total_observations,
        oldest_observation_at: oldest,
        newest_observation_at: newest,
        observation_counts: counts
            .into_iter()
            .map(
                |((provider_id, account, window_label), count)| HistoryWindowCount {
                    provider_id,
                    account,
                    window_label,
                    count,
                },
            )
            .collect(),
    }
}

fn build_bundle(
    app: &AppHandle,
    handle: &RuntimeHandle,
    settings: DiagnosticsSettingsInput,
    generated_at: DateTime<Utc>,
) -> DiagnosticsBundle {
    let source = handle.diagnostic_source(generated_at);
    let history = handle
        .history_store()
        .map(|store| (store.revision(), store.history()))
        .unwrap_or((0, Vec::new()));
    let notification_counts = handle.notification_counts().unwrap_or((false, 0, 0));
    let app_data_health =
        if handle.history_store().is_some() && handle.notification_counts().is_some() {
            "available"
        } else {
            "degraded"
        };
    let scheduler_state = if source.follow_up_pending {
        "followUpPending"
    } else if source.cycle_in_flight {
        "cycleInFlight"
    } else {
        "idle"
    };
    let provider_count = source.providers.len();
    DiagnosticsBundle {
        schema_version: DIAGNOSTICS_SCHEMA_VERSION,
        generated_at: safe_timestamp(generated_at),
        app: AppDiagnostics {
            product_name: sanitize_text(
                app.config()
                    .product_name
                    .as_deref()
                    .unwrap_or(app.package_info().name.as_str()),
            ),
            version: sanitize_text(app.package_info().version.to_string().as_str()),
            platform: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            app_identifier: sanitize_text(&app.config().identifier),
        },
        environment: EnvironmentDiagnostics {
            os_family: std::env::consts::OS,
            os_version: None,
            app_data_health,
            runtime_provider_count: provider_count,
        },
        runtime: RuntimeDiagnostics {
            snapshot_seq: source.snapshot_seq,
            last_cycle_started_at: source.last_cycle_started_at.map(safe_timestamp),
            last_cycle_completed_at: source.last_cycle_completed_at.map(safe_timestamp),
            last_usable_data_at: source.last_usable_data_at.map(safe_timestamp),
            refresh_interval_minutes: source.refresh_interval_minutes,
            scheduler_state,
            follow_up_pending: source.follow_up_pending,
        },
        providers: source
            .providers
            .into_iter()
            .map(|provider| normalize_provider(provider, generated_at))
            .collect(),
        history: history_summary(history.0, history.1),
        notifications: NotificationDiagnostics {
            enabled: notification_counts.0,
            dedup_entry_count: notification_counts.1,
            active_current_cycle_state_count: notification_counts.2,
        },
        settings: SettingsDiagnostics {
            theme: safe_theme(&settings.theme),
            refresh_interval_minutes: source.refresh_interval_minutes,
            launch_at_startup: settings.launch_at_startup,
            notification_enabled: settings.notification_enabled,
        },
    }
}

fn export_file_name(now: DateTime<Utc>) -> String {
    format!("runtime-diagnostics-{}.json", now.format("%Y%m%dT%H%M%SZ"))
}

fn write_bundle(path: &Path, bundle: &DiagnosticsBundle) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(bundle).map_err(|error| sanitize_text(&error.to_string()))?;
    if json.len() > MAX_BUNDLE_BYTES {
        return Err("Diagnostics bundle exceeded its bounded output size".to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| sanitize_text(&error.to_string()))?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, &json).map_err(|error| sanitize_text(&error.to_string()))?;
    fs::rename(&temp, path).map_err(|error| sanitize_text(&error.to_string()))
}

#[tauri::command]
pub async fn export_diagnostics(
    app: AppHandle,
    handle: tauri::State<'_, RuntimeHandle>,
    settings: DiagnosticsSettingsInput,
) -> Result<ExportDiagnosticsResult, String> {
    let generated_at = Utc::now();
    let bundle = build_bundle(&app, &handle, settings, generated_at);
    let file_name = export_file_name(generated_at);
    let selected = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(&file_name)
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(ExportDiagnosticsResult {
            status: "cancelled",
            file_name: None,
        });
    };
    let path = selected
        .into_path()
        .map_err(|_| "The selected diagnostics destination is unavailable".to_string())?;
    write_bundle(&path, &bundle)?;
    Ok(ExportDiagnosticsResult {
        status: "saved",
        file_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .map(sanitize_text),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{AccountAttributionDto, ProviderHealth, UsageLimitDto};
    use serde_json::Value;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-29T12:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn usage() -> ProviderUsageDto {
        ProviderUsageDto {
            id: "openai-codex".to_string(),
            name: "OpenAI / Codex".to_string(),
            health: ProviderHealth::Live,
            status: ProviderHealth::Live.legacy_status(),
            checked_at: "2026-09-29T11:55:00.000Z".to_string(),
            limits: vec![UsageLimitDto {
                label: "5h".to_string(),
                used_percent: 42.5,
                reset_at: Some("2026-09-29T14:00:00.000Z".to_string()),
            }],
            account: Some(AccountAttributionDto {
                label: "key ••3456".to_string(),
                note: None,
                identity: Some("key:3456".to_string()),
            }),
            plan_type: None,
            error: None,
            error_category: None,
            error_http_status: None,
            source_updated_at: None,
            data_freshness: Some("fresh"),
            reset_credits: None,
            zcode_reset_cards: None,
            zcode_plans: None,
            grok_bot: None,
            fallback_failure: None,
        }
    }

    fn source_with_error(message: &str) -> ProviderDiagnosticSource {
        ProviderDiagnosticSource {
            usage: usage(),
            cooldown_until: None,
            last_success_at: Some(now()),
            last_error: Some(ErrorDiagnosticSource {
                code: "server_error".to_string(),
                message: message.to_string(),
                http_status: Some(503),
            }),
            last_error_at: Some(now()),
        }
    }

    fn fixture_bundle(message: &str) -> DiagnosticsBundle {
        DiagnosticsBundle {
            schema_version: DIAGNOSTICS_SCHEMA_VERSION,
            generated_at: safe_timestamp(now()),
            app: AppDiagnostics {
                product_name: "LimitScope".to_string(),
                version: "0.5.0".to_string(),
                platform: "windows",
                architecture: "x86_64",
                app_identifier: "com.ratelimits.desktop".to_string(),
            },
            environment: EnvironmentDiagnostics {
                os_family: "windows",
                os_version: None,
                app_data_health: "available",
                runtime_provider_count: 1,
            },
            runtime: RuntimeDiagnostics {
                snapshot_seq: 3,
                last_cycle_started_at: Some(safe_timestamp(now())),
                last_cycle_completed_at: Some(safe_timestamp(now())),
                last_usable_data_at: Some(safe_timestamp(now())),
                refresh_interval_minutes: 5,
                scheduler_state: "idle",
                follow_up_pending: false,
            },
            providers: vec![normalize_provider(source_with_error(message), now())],
            history: history_summary(
                2,
                vec![QuotaObservation {
                    provider_id: "openai-codex".into(),
                    account: Some("key:3456".into()),
                    window_label: "5h".into(),
                    used_percent: 42.5,
                    observed_at: "2026-09-29T11:55:00.000Z".into(),
                    reset_at: Some("2026-09-29T14:00:00.000Z".into()),
                }],
            ),
            notifications: NotificationDiagnostics {
                enabled: false,
                dedup_entry_count: 1,
                active_current_cycle_state_count: 0,
            },
            settings: SettingsDiagnostics {
                theme: "graphite".into(),
                refresh_interval_minutes: 5,
                launch_at_startup: false,
                notification_enabled: false,
            },
        }
    }

    fn serialized_with_error(message: &str) -> String {
        let provider = normalize_provider(source_with_error(message), now());
        serde_json::to_string(&provider).unwrap()
    }

    fn assert_secret_removed(secret: &str) {
        let nested = format!("nested failure {{ error: {{ message: '{secret}' }} }}");
        let sanitized = sanitize_text(&nested);
        assert!(!sanitized.contains(secret), "{sanitized}");
        // The hostile input must be actively redacted and the benign carrier
        // must survive, so the pass cannot come from an emptied output.
        assert!(sanitized.contains("[REDACTED]"), "{sanitized}");
        assert!(sanitized.contains("nested failure"), "{sanitized}");
        let serialized = serialized_with_error(&nested);
        assert!(!serialized.contains(secret), "{serialized}");
        assert!(!serialized.contains(&nested), "{serialized}");
        // Positive control: the hostile message actually entered the error
        // path and was replaced by the fixed safe sentence.
        assert!(serialized.contains("Provider refresh failed."), "{serialized}");
    }

    #[test]
    fn schema_contains_required_safe_fields() {
        let value = serde_json::to_value(normalize_provider(
            ProviderDiagnosticSource {
                usage: usage(),
                cooldown_until: Some(now() + chrono::Duration::minutes(1)),
                last_success_at: Some(now()),
                last_error: None,
                last_error_at: None,
            },
            now(),
        ))
        .unwrap();
        for field in [
            "providerId",
            "health",
            "status",
            "freshness",
            "maskedAccountIdentity",
            "windows",
            "cooldownActive",
            "cooldownUntil",
            "lastSuccessAt",
        ] {
            assert!(value.get(field).is_some(), "missing {field}");
        }
        // Canonical health contract: the bundle's health word is the
        // normalized enum value and the legacy status is its projection —
        // the two can never disagree.
        assert_eq!(value["health"], "live");
        assert_eq!(value["status"], "ok");
    }

    #[test]
    fn provider_state_is_normalized() {
        let mut source = source_with_error("safe failure");
        source.usage.limits[0].used_percent = 140.0;
        source.usage.limits[0].reset_at = Some("not-a-date".to_string());
        let value = serde_json::to_value(normalize_provider(source, now())).unwrap();
        assert_eq!(value["windows"][0]["usedPercent"], 100.0);
        assert!(value["windows"][0].get("resetAt").is_none());
        assert_eq!(value["lastError"]["category"], "server");
    }

    #[test]
    fn masked_account_identity_is_allowed() {
        let value =
            serde_json::to_value(normalize_provider(source_with_error("safe"), now())).unwrap();
        assert_eq!(value["maskedAccountIdentity"], "key ••3456");
    }

    #[test]
    fn access_token_is_removed() {
        assert_secret_removed("access_token: ey-secret-value");
    }

    #[test]
    fn refresh_token_is_removed() {
        assert_secret_removed("refresh_token=refresh-secret-value");
    }

    #[test]
    fn jwt_is_removed() {
        assert_secret_removed("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U");
    }

    #[test]
    fn api_key_is_removed() {
        assert_secret_removed("api_key: sk-live-1234567890");
    }

    #[test]
    fn authorization_header_is_removed() {
        assert_secret_removed("Authorization: Bearer abc123secret");
    }

    #[test]
    fn cookie_is_removed() {
        assert_secret_removed("Cookie: session=abc123secret");
    }

    #[test]
    fn nested_secret_shaped_error_is_sanitized() {
        assert_secret_removed("Bearer nested-random-secret-value-1234567890");
    }

    #[test]
    fn long_credential_shaped_value_is_removed() {
        assert_secret_removed(
            "credential-value-with-a-very-long-random-shape-12345678901234567890",
        );
    }

    #[test]
    fn short_session_identifier_is_removed() {
        assert_secret_removed("session_id=abc123");
    }

    #[test]
    fn absolute_path_is_removed() {
        assert_secret_removed("C:\\Users\\alice\\AppData\\Local\\quota.json");
    }

    #[test]
    fn provider_error_text_is_never_serialized() {
        let raw = "upstream body: session_id=abc123 at C:\\Users\\alice\\quota.json";
        let serialized = serialized_with_error(raw);
        assert!(!serialized.contains(raw));
        assert!(!serialized.contains("abc123"));
        assert!(!serialized.contains("alice"));
        assert!(serialized.contains("Provider refresh failed."));
    }

    #[test]
    fn raw_provider_payload_is_never_serialized() {
        #[derive(Serialize)]
        struct RawProviderPayload {
            access_token: &'static str,
            response_body: &'static str,
            exception: &'static str,
        }
        let raw = RawProviderPayload {
            access_token: "raw-access-token",
            response_body: "raw response",
            exception: "raw exception",
        };
        let provider = normalize_provider(source_with_error("normalized only"), now());
        let serialized = serde_json::to_string(&provider).unwrap();
        let raw_serialized = serde_json::to_string(&raw).unwrap();
        // The hostile fixture must itself serialize — the comparison and the
        // absence checks below are only meaningful when the payload is
        // actually present.
        assert!(raw_serialized.contains("raw-access-token"), "{raw_serialized}");
        assert!(!serialized.contains("raw-access-token"));
        assert!(!serialized.contains("raw response"));
        assert!(!serialized.contains("raw exception"));
        assert_ne!(serialized, raw_serialized);
        assert!(serialized.contains("openai-codex"), "provider serialization must be populated: {serialized}");
        assert!(provider.windows.len() == 1);
    }

    #[test]
    fn history_is_exported_as_summary_only() {
        let observations = vec![
            QuotaObservation {
                provider_id: "openai-codex".into(),
                account: Some("key:3456".into()),
                window_label: "5h".into(),
                used_percent: 10.0,
                observed_at: "2026-09-29T11:00:00.000Z".into(),
                reset_at: Some("2026-09-29T14:00:00.000Z".into()),
            },
            QuotaObservation {
                provider_id: "openai-codex".into(),
                account: Some("key:3456".into()),
                window_label: "5h".into(),
                used_percent: 20.0,
                observed_at: "2026-09-29T11:05:00.000Z".into(),
                reset_at: Some("2026-09-29T14:00:00.000Z".into()),
            },
        ];
        let value = serde_json::to_value(history_summary(7, observations)).unwrap();
        assert_eq!(value["revision"], 7);
        assert_eq!(value["totalObservations"], 2);
        assert_eq!(value["observationCounts"].as_array().unwrap().len(), 1);
        assert!(value.get("observations").is_none());
    }

    #[test]
    fn notification_state_is_exported_as_summary_only() {
        let value = serde_json::to_value(NotificationDiagnostics {
            enabled: true,
            dedup_entry_count: 4,
            active_current_cycle_state_count: 2,
        })
        .unwrap();
        assert_eq!(value.as_object().unwrap().len(), 3);
        assert!(value.get("windows").is_none());
        assert!(value.get("payloads").is_none());
    }

    #[test]
    fn corrupt_or_unknown_optional_fields_fail_safely() {
        let mut source = source_with_error("safe");
        source.usage.limits.push(UsageLimitDto {
            label: "".into(),
            used_percent: f64::NAN,
            reset_at: Some("invalid".into()),
        });
        source.cooldown_until = Some(now() - chrono::Duration::minutes(1));
        let value = serde_json::to_value(normalize_provider(source, now())).unwrap();
        assert_eq!(value["windows"].as_array().unwrap().len(), 1);
        assert_eq!(value["cooldownActive"], false);
        assert!(value.get("cooldownUntil").is_none());
    }

    #[test]
    fn output_json_is_valid_and_bounded() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-output/runtime-diagnostics-fixture.json");
        let bundle = fixture_bundle(
            "nested access_token=abc refresh_token=def api_key=ghi Authorization: Bearer jkl Cookie: mno",
        );
        write_bundle(&path, &bundle).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        for field in [
            "schemaVersion",
            "generatedAt",
            "app",
            "environment",
            "runtime",
            "providers",
            "history",
            "notifications",
            "settings",
        ] {
            assert!(parsed.get(field).is_some(), "missing {field}");
        }
        assert_eq!(parsed["schemaVersion"], 1);
        // Fixture-strength controls: the hostile error message must have
        // actually flowed through the bundle (the safe sentence proves the
        // error path ran) and the sanitized provider content must be present
        // — otherwise the secret-term scan below could pass on an empty husk.
        assert!(
            raw.contains("Provider refresh failed."),
            "error path not exercised: {raw}"
        );
        assert!(raw.contains("openai-codex"), "sanitized provider content missing: {raw}");
        assert!(raw.contains("key ••3456"), "masked account label missing: {raw}");
        assert!(raw.len() < MAX_BUNDLE_BYTES);
        let lower = raw.to_lowercase();
        for secret_term in [
            "token",
            "secret",
            "bearer",
            "authorization",
            "cookie",
            "jwt",
            "api_key",
            "refresh_token",
            "access_token",
        ] {
            assert!(!lower.contains(secret_term), "fixture leaked {secret_term}");
        }
    }

    #[test]
    fn schema_version_is_deterministic() {
        assert_eq!(DIAGNOSTICS_SCHEMA_VERSION, 1);
        assert_eq!(
            export_file_name(now()),
            "runtime-diagnostics-20260929T120000Z.json"
        );
    }

    #[test]
    fn reset_credits_project_counts_and_time_only() {
        // v0.7 Codex capability: the bundle carries the banked balance, the
        // applicability count, and the observation time — never raw
        // payloads, identifiers, or redemption semantics.
        use crate::codex::{CodexResetCredits, RESET_CREDITS_SOURCE};
        use crate::runtime::ProviderUsageDto;
        let mut credited: ProviderUsageDto = usage();
        credited.reset_credits = Some(CodexResetCredits {
            banked_credits: 3,
            currently_applicable: Some(0),
            checked_at: "2026-09-29T11:55:00.000Z".to_string(),
            source: RESET_CREDITS_SOURCE,
        });
        let source = ProviderDiagnosticSource {
            usage: credited,
            cooldown_until: None,
            last_success_at: Some(now()),
            last_error: None,
            last_error_at: None,
        };
        let raw = serde_json::to_string(&normalize_provider(source, now())).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let credits = parsed.get("resetCredits").expect("credits must project");
        assert_eq!(credits.get("bankedCredits").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(
            credits.get("currentlyApplicable").and_then(|v| v.as_u64()),
            Some(0)
        );
        assert_eq!(
            credits.get("checkedAt").and_then(|v| v.as_str()),
            Some("2026-09-29T11:55:00.000Z")
        );
        assert!(credits.get("source").is_none(), "source tag stays internal");
        assert!(credits.get("expiresAt").is_none(), "no invented expiry");
    }
}
