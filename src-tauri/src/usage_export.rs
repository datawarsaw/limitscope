//! Local-only usage data export in CSV and JSON formats.
//!
//! Exports exactly the observations retained in the quota history store.
//! Enforces:
//! - No credentials, tokens, or raw provider responses
//! - Sanitization of all free-text fields and error messages
//! - Deterministic ordering by provider -> account -> windowLabel -> observedAt
//! - RFC 4180 CSV and JSON equivalence
//!
//! The destination is a folder chosen through the native folder picker plus a
//! user-supplied file name confirmed in the app. The native Save As dialog is
//! deliberately not used for the final write decision: on Windows it
//! categorically refuses to commit onto an existing read-only file and stays
//! open silently for every flag combination, which left the previous export
//! command pending forever with no way for the app to report the failure.
//! Routing the destination through the app keeps every export outcome
//! (saved, declined, write failure) deterministic and reportable.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::diagnostics::sanitize_text;
use crate::history::{parse_epoch_ms, QuotaObservation};
use crate::runtime::{ProviderKind, RuntimeHandle};

pub const USAGE_EXPORT_SCHEMA_VERSION: u32 = 1;
pub const USAGE_EXPORT_KIND: &str = "limitscope-usage-export";

/// Longest file name accepted for an export, so the composed destination
/// stays well under the Windows path limit.
const MAX_EXPORT_FILE_NAME_BYTES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageExportFormat {
    Csv,
    Json,
}

impl UsageExportFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "csv" => Some(UsageExportFormat::Csv),
            "json" => Some(UsageExportFormat::Json),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            UsageExportFormat::Csv => "csv",
            UsageExportFormat::Json => "json",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportUsageResult {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageExportRow {
    pub provider_id: String,
    pub provider_name: String,
    pub account: Option<String>,
    pub window_label: String,
    pub used_percent: f64,
    pub observed_at: String,
    pub reset_at: Option<String>,
    pub resolution: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRangeCovered {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSemantics {
    pub gap_semantics: String,
    pub resolution_semantics: String,
    pub reset_boundary_rule: String,
    pub thresholds_measure_used: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageExportEnvelope {
    pub schema_version: u32,
    pub kind: String,
    pub exported_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_covered: Option<ExportRangeCovered>,
    pub semantics: ExportSemantics,
    pub rows: Vec<UsageExportRow>,
}

pub(crate) fn escape_csv_cell(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        let escaped = value.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        value.to_string()
    }
}

pub(crate) fn build_usage_export_rows(
    observations: &[QuotaObservation],
    exported_at: DateTime<Utc>,
) -> Vec<UsageExportRow> {
    let mut sorted_obs = observations.to_vec();
    sorted_obs.sort_by(|a, b| {
        a.provider_id
            .cmp(&b.provider_id)
            .then_with(|| {
                a.account
                    .as_deref()
                    .unwrap_or_default()
                    .cmp(b.account.as_deref().unwrap_or_default())
            })
            .then_with(|| a.window_label.cmp(&b.window_label))
            .then_with(|| {
                let t_a = parse_epoch_ms(&a.observed_at).unwrap_or(0);
                let t_b = parse_epoch_ms(&b.observed_at).unwrap_or(0);
                t_a.cmp(&t_b).then_with(|| a.observed_at.cmp(&b.observed_at))
            })
    });

    let exported_at_ms = exported_at.timestamp_millis();
    let detailed_cutoff_ms = exported_at_ms - 24 * 60 * 60 * 1000;

    sorted_obs
        .into_iter()
        .map(|obs| {
            let provider_name = ProviderKind::from_id(&obs.provider_id)
                .map(|k| k.name())
                .unwrap_or("")
                .to_string();
            let observed_ms = parse_epoch_ms(&obs.observed_at).unwrap_or(0);
            let resolution = if observed_ms >= detailed_cutoff_ms {
                "detailed"
            } else {
                "compacted"
            }
            .to_string();

            UsageExportRow {
                provider_id: sanitize_text(&obs.provider_id),
                provider_name,
                account: obs.account.as_deref().map(sanitize_text),
                window_label: sanitize_text(&obs.window_label),
                used_percent: obs.used_percent.clamp(0.0, 100.0),
                observed_at: sanitize_text(&obs.observed_at),
                reset_at: obs.reset_at.as_deref().map(sanitize_text),
                resolution,
            }
        })
        .collect()
}

pub(crate) fn build_usage_export_csv(rows: &[UsageExportRow]) -> String {
    let mut out = String::from(
        "providerId,providerName,account,windowLabel,usedPercent,observedAt,resetAt,resolution\r\n",
    );
    for row in rows {
        out.push_str(&escape_csv_cell(&row.provider_id));
        out.push(',');
        out.push_str(&escape_csv_cell(&row.provider_name));
        out.push(',');
        if let Some(account) = &row.account {
            out.push_str(&escape_csv_cell(account));
        }
        out.push(',');
        out.push_str(&escape_csv_cell(&row.window_label));
        out.push(',');
        out.push_str(&row.used_percent.to_string());
        out.push(',');
        out.push_str(&escape_csv_cell(&row.observed_at));
        out.push(',');
        if let Some(reset_at) = &row.reset_at {
            out.push_str(&escape_csv_cell(reset_at));
        }
        out.push(',');
        out.push_str(&escape_csv_cell(&row.resolution));
        out.push_str("\r\n");
    }
    out
}

pub(crate) fn build_usage_export_json(
    rows: &[UsageExportRow],
    exported_at: DateTime<Utc>,
) -> Result<String, String> {
    let range_covered = if rows.is_empty() {
        None
    } else {
        let from = rows
            .iter(
            )
            .map(|r| &r.observed_at)
            .min_by(|a, b| {
                let t_a = parse_epoch_ms(a).unwrap_or(0);
                let t_b = parse_epoch_ms(b).unwrap_or(0);
                t_a.cmp(&t_b).then_with(|| a.cmp(b))
            })
            .cloned()
            .unwrap_or_default();
        let to = rows
            .iter()
            .map(|r| &r.observed_at)
            .max_by(|a, b| {
                let t_a = parse_epoch_ms(a).unwrap_or(0);
                let t_b = parse_epoch_ms(b).unwrap_or(0);
                t_a.cmp(&t_b).then_with(|| a.cmp(b))
            })
            .cloned()
            .unwrap_or_default();
        Some(ExportRangeCovered { from, to })
    };

    let envelope = UsageExportEnvelope {
        schema_version: USAGE_EXPORT_SCHEMA_VERSION,
        kind: USAGE_EXPORT_KIND.to_string(),
        exported_at: exported_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        range_covered,
        semantics: ExportSemantics {
            gap_semantics: "notObservedNeverZeroFilled".to_string(),
            resolution_semantics: "detailedWithin24hOtherwiseCompacted".to_string(),
            reset_boundary_rule:
                "usedPercentDropOver5PointsOrResetAtForwardMoveOver1Minute".to_string(),
            thresholds_measure_used: true,
        },
        rows: rows.to_vec(),
    };

    serde_json::to_string_pretty(&envelope).map_err(|error| sanitize_text(&error.to_string()))
}

fn write_export(path: &Path, content: &str, format: UsageExportFormat) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| sanitize_text(&error.to_string()))?;
    }
    let ext = match format {
        UsageExportFormat::Csv => "csv.tmp",
        UsageExportFormat::Json => "json.tmp",
    };
    let temp = path.with_extension(ext);
    fs::write(&temp, content).map_err(|error| sanitize_text(&error.to_string()))?;
    if let Err(error) = fs::rename(&temp, path) {
        // The final file is untouched; the staging file must not linger.
        let _ = fs::remove_file(&temp);
        return Err(sanitize_text(&error.to_string()));
    }
    Ok(())
}

/// Validates the user-supplied export file name and returns it with the
/// format extension appended when missing, mirroring the old native save
/// dialog behavior (a typed extension is kept as written).
pub(crate) fn validate_export_file_name(
    raw: &str,
    format: UsageExportFormat,
) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Enter a file name for the export".to_string());
    }
    if trimmed.len() > MAX_EXPORT_FILE_NAME_BYTES {
        return Err("The file name is too long".to_string());
    }
    if trimmed.ends_with('.') || trimmed.ends_with(' ') {
        return Err("The file name cannot end with a dot or a space".to_string());
    }
    if trimmed
        .chars()
        .any(|character| matches!(character, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*') || character.is_control())
    {
        return Err("The file name contains characters that are not allowed".to_string());
    }
    let stem = trimmed.split('.').next().unwrap_or_default();
    const RESERVED_DEVICE_NAMES: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
        "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if RESERVED_DEVICE_NAMES.contains(&stem.to_ascii_lowercase().as_str()) {
        return Err("The file name uses a reserved device name".to_string());
    }
    if !trimmed.contains('.') {
        return Ok(format!("{trimmed}.{}", format.extension()));
    }
    Ok(trimmed.to_string())
}

/// The settled decision for an export destination: either the write may
/// proceed, or an existing destination needs an explicit replace
/// confirmation first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExportDestination {
    Write(PathBuf),
    ConfirmOverwrite { path: PathBuf, file_name: String },
}

pub(crate) fn resolve_export_destination(
    directory: &str,
    file_name: &str,
    format: UsageExportFormat,
    confirm_overwrite: bool,
) -> Result<ExportDestination, String> {
    let name = validate_export_file_name(file_name, format)?;
    let dir = PathBuf::from(directory.trim());
    if !dir.is_dir() {
        return Err("The selected export destination is unavailable".to_string());
    }
    let path = dir.join(&name);
    if path.exists() && !confirm_overwrite {
        return Ok(ExportDestination::ConfirmOverwrite {
            path,
            file_name: name,
        });
    }
    Ok(ExportDestination::Write(path))
}

/// Native folder picker for the export destination. Always settles: the
/// user either picks a folder or cancels (None). There is deliberately no
/// save-file dialog on this path anymore — see the module documentation.
#[tauri::command]
pub async fn pick_usage_export_directory(app: AppHandle) -> Result<Option<String>, String> {
    let selected = app.dialog().file().set_title("Choose export folder").blocking_pick_folder();
    let path = selected.and_then(|selection| selection.into_path().ok());
    Ok(path.map(|path| path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn export_usage_history(
    handle: tauri::State<'_, RuntimeHandle>,
    format: String,
    directory: String,
    file_name: String,
    confirm_overwrite: bool,
) -> Result<ExportUsageResult, String> {
    let export_format =
        UsageExportFormat::parse(&format).ok_or_else(|| "Unsupported export format".to_string())?;
    let Some(store) = handle.history_store() else {
        return Err("History store is unavailable".to_string());
    };
    let exported_at = Utc::now();
    let observations = store.history_range(None, None, None, "7d");
    let rows = build_usage_export_rows(&observations, exported_at);
    let content = match export_format {
        UsageExportFormat::Csv => build_usage_export_csv(&rows),
        UsageExportFormat::Json => build_usage_export_json(&rows, exported_at)?,
    };

    let destination = resolve_export_destination(&directory, &file_name, export_format, confirm_overwrite)?;
    match destination {
        ExportDestination::ConfirmOverwrite { file_name, .. } => Ok(ExportUsageResult {
            status: "confirm-overwrite",
            file_name: Some(file_name),
        }),
        ExportDestination::Write(path) => {
            write_export(&path, &content, export_format)?;
            Ok(ExportUsageResult {
                status: "saved",
                file_name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(sanitize_text),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_report::secret_shaped_material;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &PathBuf {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn temp_test_dir(tag: &str) -> TempDir {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "rate-limits-export-test-{}-{}-{}",
            tag,
            std::process::id(),
            unique
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }

    fn parse_csv_test_rows(csv: &str) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        let mut current_row = Vec::new();
        let mut current_field = String::new();
        let mut in_quotes = false;
        let mut chars = csv.chars().peekable();

        while let Some(c) = chars.next() {
            if in_quotes {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        current_field.push('"');
                    } else {
                        in_quotes = false;
                    }
                } else {
                    current_field.push(c);
                }
            } else {
                match c {
                    '"' => in_quotes = true,
                    ',' => {
                        current_row.push(current_field);
                        current_field = String::new();
                    }
                    '\r' => {
                        if chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                        current_row.push(current_field);
                        current_field = String::new();
                        rows.push(current_row);
                        current_row = Vec::new();
                    }
                    '\n' => {
                        current_row.push(current_field);
                        current_field = String::new();
                        rows.push(current_row);
                        current_row = Vec::new();
                    }
                    _ => current_field.push(c),
                }
            }
        }
        if !current_field.is_empty() || !current_row.is_empty() {
            current_row.push(current_field);
            rows.push(current_row);
        }
        rows
    }

    #[test]
    fn field_allowlist_is_strictly_enforced() {
        let row = UsageExportRow {
            provider_id: "openai-codex".to_string(),
            provider_name: "OpenAI / Codex".to_string(),
            account: Some("chatgpt:1234".to_string()),
            window_label: "5-hour".to_string(),
            used_percent: 50.0,
            observed_at: "2026-09-30T12:00:00.000Z".to_string(),
            reset_at: Some("2026-09-30T17:00:00.000Z".to_string()),
            resolution: "detailed".to_string(),
        };
        let value = serde_json::to_value(&row).unwrap();
        let map = value.as_object().unwrap();
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "account",
                "observedAt",
                "providerId",
                "providerName",
                "resetAt",
                "resolution",
                "usedPercent",
                "windowLabel",
            ]
        );
    }

    #[test]
    fn csv_escaping_matrix() {
        assert_eq!(escape_csv_cell("normal"), "normal");
        assert_eq!(escape_csv_cell("comma,here"), "\"comma,here\"");
        assert_eq!(escape_csv_cell("quote \"here\""), "\"quote \"\"here\"\"\"");
        assert_eq!(escape_csv_cell("line1\nline2"), "\"line1\nline2\"");
        assert_eq!(escape_csv_cell("line1\r\nline2"), "\"line1\r\nline2\"");
        assert_eq!(escape_csv_cell(""), "");
    }

    #[test]
    fn empty_history_produces_valid_csv_and_json() {
        let exported_at = DateTime::parse_from_rfc3339("2026-09-30T12:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);
        let rows = build_usage_export_rows(&[], exported_at);
        assert!(rows.is_empty());

        let csv = build_usage_export_csv(&rows);
        assert_eq!(
            csv,
            "providerId,providerName,account,windowLabel,usedPercent,observedAt,resetAt,resolution\r\n"
        );

        let json_str = build_usage_export_json(&rows, exported_at).unwrap();
        let envelope: UsageExportEnvelope = serde_json::from_str(&json_str).unwrap();
        assert_eq!(envelope.schema_version, 1);
        assert_eq!(envelope.kind, "limitscope-usage-export");
        assert_eq!(envelope.exported_at, "2026-09-30T12:00:00.000Z");
        assert!(envelope.range_covered.is_none());
        assert!(envelope.rows.is_empty());
        assert_eq!(envelope.semantics.gap_semantics, "notObservedNeverZeroFilled");
        assert_eq!(
            envelope.semantics.resolution_semantics,
            "detailedWithin24hOtherwiseCompacted"
        );
        assert_eq!(
            envelope.semantics.reset_boundary_rule,
            "usedPercentDropOver5PointsOrResetAtForwardMoveOver1Minute"
        );
        assert!(envelope.semantics.thresholds_measure_used);
    }

    #[test]
    fn deterministic_ordering_across_providers_accounts_and_windows() {
        let exported_at = DateTime::parse_from_rfc3339("2026-09-30T12:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);

        let observations = vec![
            QuotaObservation {
                provider_id: "zai".to_string(),
                window_label: "monthly".to_string(),
                used_percent: 10.0,
                observed_at: "2026-09-30T10:00:00.000Z".to_string(),
                reset_at: None,
                account: Some("key:9999".to_string()),
            },
            QuotaObservation {
                provider_id: "openai-codex".to_string(),
                window_label: "5-hour".to_string(),
                used_percent: 20.0,
                observed_at: "2026-09-30T11:00:00.000Z".to_string(),
                reset_at: None,
                account: Some("chatgpt:2".to_string()),
            },
            QuotaObservation {
                provider_id: "openai-codex".to_string(),
                window_label: "5-hour".to_string(),
                used_percent: 5.0,
                observed_at: "2026-09-30T09:00:00.000Z".to_string(),
                reset_at: None,
                account: Some("chatgpt:1".to_string()),
            },
            QuotaObservation {
                provider_id: "antigravity".to_string(),
                window_label: "daily".to_string(),
                used_percent: 15.0,
                observed_at: "2026-09-30T08:00:00.000Z".to_string(),
                reset_at: None,
                account: None,
            },
        ];

        let rows = build_usage_export_rows(&observations, exported_at);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].provider_id, "antigravity");
        assert_eq!(rows[1].provider_id, "openai-codex");
        assert_eq!(rows[1].account.as_deref(), Some("chatgpt:1"));
        assert_eq!(rows[2].provider_id, "openai-codex");
        assert_eq!(rows[2].account.as_deref(), Some("chatgpt:2"));
        assert_eq!(rows[3].provider_id, "zai");
    }

    #[test]
    fn unknown_provider_id_yields_empty_name() {
        let exported_at = DateTime::parse_from_rfc3339("2026-09-30T12:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);

        let obs = vec![QuotaObservation {
            provider_id: "custom-unrecognized".to_string(),
            window_label: "test".to_string(),
            used_percent: 50.0,
            observed_at: "2026-09-30T11:00:00.000Z".to_string(),
            reset_at: None,
            account: None,
        }];

        let rows = build_usage_export_rows(&obs, exported_at);
        assert_eq!(rows[0].provider_name, "");
        assert_eq!(rows[0].provider_id, "custom-unrecognized");
    }

    #[test]
    fn credential_shaped_material_is_excluded() {
        let exported_at = DateTime::parse_from_rfc3339("2026-09-30T12:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);

        let hostile_obs = vec![
            QuotaObservation {
                provider_id: "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.e30.t-ae9MIDwtx".to_string(),
                window_label: "sk-abcdef1234567890abcdef1234567890".to_string(),
                used_percent: 50.0,
                observed_at: "2026-09-30T11:00:00.000Z".to_string(),
                reset_at: Some("Authorization: Bearer secret-value-12345678".to_string()),
                account: Some("api_key: secret-value-12345678".to_string()),
            },
        ];

        let rows = build_usage_export_rows(&hostile_obs, exported_at);
        let csv = build_usage_export_csv(&rows);
        let json = build_usage_export_json(&rows, exported_at).unwrap();

        // Fixture-strength controls: the hostile observation must actually
        // reach the export pipeline and be actively redacted, and the benign
        // fields must survive, so the leak checks below cannot pass on an
        // empty export.
        assert_eq!(rows.len(), 1, "the hostile observation was dropped");
        assert!(csv.contains("[REDACTED]"), "csv: {csv}");
        assert!(csv.contains("2026-09-30T11:00:00.000Z"), "csv: {csv}");
        assert!(json.contains("[REDACTED]"), "json: {json}");

        assert!(!csv.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
        assert!(!csv.contains("sk-abcdef1234567890abcdef1234567890"));
        assert!(!csv.contains("secret-value-12345678"));
        assert!(secret_shaped_material(&csv).is_none());

        assert!(!json.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
        assert!(!json.contains("sk-abcdef1234567890abcdef1234567890"));
        assert!(!json.contains("secret-value-12345678"));
        assert!(secret_shaped_material(&json).is_none());
    }

    #[test]
    fn golden_fixture_exact_match_and_csv_json_equivalence() {
        let exported_at = DateTime::parse_from_rfc3339("2026-09-30T12:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);

        let golden_obs = vec![
            QuotaObservation {
                provider_id: "antigravity".to_string(),
                window_label: "daily".to_string(),
                used_percent: 12.5,
                observed_at: "2026-09-30T08:00:00.000Z".to_string(),
                reset_at: Some("2026-10-01T00:00:00.000Z".to_string()),
                account: None,
            },
            QuotaObservation {
                provider_id: "grok".to_string(),
                window_label: "sliding".to_string(),
                used_percent: 85.0,
                observed_at: "2026-09-28T10:00:00.000Z".to_string(),
                reset_at: None,
                account: Some("xai:acct_1".to_string()),
            },
            QuotaObservation {
                provider_id: "openai-codex".to_string(),
                window_label: "5-hour".to_string(),
                used_percent: 0.0,
                observed_at: "2026-09-30T11:30:00.000Z".to_string(),
                reset_at: Some("2026-09-30T16:30:00.000Z".to_string()),
                account: Some("chatgpt:5678".to_string()),
            },
            QuotaObservation {
                provider_id: "opencode-go".to_string(),
                window_label: "label, with \"comma\"".to_string(),
                used_percent: 100.0,
                observed_at: "2026-09-29T00:00:00.000Z".to_string(),
                reset_at: None,
                account: Some("key:3456".to_string()),
            },
            QuotaObservation {
                provider_id: "unknown-vendor".to_string(),
                window_label: "custom".to_string(),
                used_percent: 42.0,
                observed_at: "2026-09-27T12:00:00.000Z".to_string(),
                reset_at: None,
                account: None,
            },
            QuotaObservation {
                provider_id: "zai".to_string(),
                window_label: "monthly".to_string(),
                used_percent: 63.25,
                observed_at: "2026-09-30T10:00:00.000Z".to_string(),
                reset_at: Some("2026-10-31T23:59:59.000Z".to_string()),
                account: Some("key:9012".to_string()),
            },
        ];

        let rows = build_usage_export_rows(&golden_obs, exported_at);
        let csv = build_usage_export_csv(&rows);
        let json_str = build_usage_export_json(&rows, exported_at).unwrap();

        let expected_csv = include_str!("../../fixtures/usage-export/golden.csv");
        let expected_json = include_str!("../../fixtures/usage-export/golden.json");

        assert_eq!(csv, expected_csv);
        let expected_json_val: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        let actual_json_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(actual_json_val, expected_json_val);

        // Assert 1:1 CSV <-> JSON equivalence
        let csv_rows = parse_csv_test_rows(&csv);
        assert_eq!(csv_rows[0], vec![
            "providerId", "providerName", "account", "windowLabel",
            "usedPercent", "observedAt", "resetAt", "resolution"
        ]);
        let data_rows = &csv_rows[1..];
        let envelope: UsageExportEnvelope = serde_json::from_str(&json_str).unwrap();
        assert_eq!(data_rows.len(), envelope.rows.len());

        for (i, (csv_row, json_row)) in data_rows.iter().zip(envelope.rows.iter()).enumerate() {
            assert_eq!(csv_row[0], json_row.provider_id, "row {i} providerId mismatch");
            assert_eq!(csv_row[1], json_row.provider_name, "row {i} providerName mismatch");
            let expected_account = json_row.account.as_deref().unwrap_or("");
            assert_eq!(csv_row[2], expected_account, "row {i} account mismatch");
            assert_eq!(csv_row[3], json_row.window_label, "row {i} windowLabel mismatch");
            let csv_used: f64 = csv_row[4].parse().unwrap();
            assert_eq!(csv_used, json_row.used_percent, "row {i} usedPercent mismatch");
            assert_eq!(csv_row[5], json_row.observed_at, "row {i} observedAt mismatch");
            let expected_reset = json_row.reset_at.as_deref().unwrap_or("");
            assert_eq!(csv_row[6], expected_reset, "row {i} resetAt mismatch");
            assert_eq!(csv_row[7], json_row.resolution, "row {i} resolution mismatch");
        }
    }

    #[test]
    fn empty_golden_fixture_exact_match() {
        let exported_at = DateTime::parse_from_rfc3339("2026-09-30T12:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc);
        let rows = build_usage_export_rows(&[], exported_at);
        let csv = build_usage_export_csv(&rows);
        let json_str = build_usage_export_json(&rows, exported_at).unwrap();

        let expected_csv = include_str!("../../fixtures/usage-export/empty.csv");
        let expected_json = include_str!("../../fixtures/usage-export/empty.json");

        assert_eq!(csv, expected_csv);
        let expected_json_val: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        let actual_json_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(actual_json_val, expected_json_val);
    }

    #[test]
    fn write_export_temp_then_rename_and_error_sanitization() {
        let dir = temp_test_dir("subdir");
        let target_path = dir.path().join("subdir").join("test_export.csv");
        let content = "test,csv\r\n";

        write_export(&target_path, content, UsageExportFormat::Csv).unwrap();
        assert!(target_path.exists());
        assert_eq!(fs::read_to_string(&target_path).unwrap(), content);

        // Temp file should no longer exist
        assert!(!target_path.with_extension("csv.tmp").exists());
    }

    /// Clears the Windows read-only attribute on drop so the temp directory
    /// can be removed even when an assertion fails mid-test.
    struct ReadOnlyFile(PathBuf);

    impl Drop for ReadOnlyFile {
        fn drop(&mut self) {
            if let Ok(metadata) = fs::metadata(&self.0) {
                let mut permissions = metadata.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                let _ = fs::set_permissions(&self.0, permissions);
            }
        }
    }

    #[test]
    fn write_export_failure_on_readonly_target_preserves_and_cleans_up() {
        let dir = temp_test_dir("readonly-target");
        let target_path = dir.path().join("target.csv");
        fs::write(&target_path, "ORIGINAL,NOT,TOUCHED\r\n").unwrap();
        let guard = ReadOnlyFile(target_path.clone());
        let mut permissions = fs::metadata(&target_path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&target_path, permissions).unwrap();

        let error =
            write_export(&target_path, "NEW,CONTENT\r\n", UsageExportFormat::Csv)
                .expect_err("rename onto a read-only target must fail");

        // Deterministic failure receipt: sanitized, no raw provider or path
        // material, and no partial state at the destination.
        assert!(!error.contains("NEW"));
        assert!(secret_shaped_material(&error).is_none());
        assert_eq!(
            fs::read_to_string(&target_path).unwrap(),
            "ORIGINAL,NOT,TOUCHED\r\n"
        );
        assert!(!target_path.with_extension("csv.tmp").exists());
        drop(guard);
    }

    #[test]
    fn write_export_failure_on_directory_target_cleans_up_temp() {
        let dir = temp_test_dir("directory-target");
        let target_path = dir.path().join("occupied");
        fs::create_dir(&target_path).unwrap();

        let error =
            write_export(&target_path, "NEW,CONTENT\r\n", UsageExportFormat::Csv)
                .expect_err("a directory cannot be replaced by a file");

        assert!(!error.contains("NEW"));
        assert!(target_path.is_dir());
        assert!(!target_path.with_extension("csv.tmp").exists());
    }

    #[test]
    fn validate_export_file_name_matrix() {
        // Extension appended only when missing.
        assert_eq!(
            validate_export_file_name("export", UsageExportFormat::Csv).unwrap(),
            "export.csv"
        );
        assert_eq!(
            validate_export_file_name("export", UsageExportFormat::Json).unwrap(),
            "export.json"
        );
        assert_eq!(
            validate_export_file_name("keep.txt", UsageExportFormat::Csv).unwrap(),
            "keep.txt"
        );
        assert_eq!(
            validate_export_file_name("  spaced name.csv  ", UsageExportFormat::Csv).unwrap(),
            "spaced name.csv"
        );

        for rejected in [
            "",
            "   ",
            "..",
            "a/b.csv",
            "a\\b.csv",
            "C:bad.csv",
            "bad<name.csv",
            "bad>name.csv",
            "bad|name.csv",
            "bad?name.csv",
            "bad*name.csv",
            "bad\"name.csv",
            "trailingdot.csv.",
            "con",
            "nul.csv",
            "COM1.csv",
        ] {
            assert!(
                validate_export_file_name(rejected, UsageExportFormat::Csv).is_err(),
                "expected rejection of {rejected:?}"
            );
        }
    }

    #[test]
    fn resolve_export_destination_confirmation_contract() {
        let dir = temp_test_dir("resolve");
        let dir_str = dir.path().to_string_lossy().into_owned();

        // A fresh name never asks for confirmation.
        let fresh = resolve_export_destination(
            &dir_str,
            "fresh.csv",
            UsageExportFormat::Csv,
            false,
        )
        .unwrap();
        assert!(matches!(fresh, ExportDestination::Write(_)));

        // An existing destination asks for confirmation first.
        fs::write(dir.path().join("existing.csv"), "old").unwrap();
        let existing = resolve_export_destination(
            &dir_str,
            "existing.csv",
            UsageExportFormat::Csv,
            false,
        )
        .unwrap();
        assert_eq!(
            existing,
            ExportDestination::ConfirmOverwrite {
                path: dir.path().join("existing.csv"),
                file_name: "existing.csv".to_string(),
            }
        );

        // With the explicit confirmation the write proceeds onto the same
        // path (the write itself is exercised by the read-only tests).
        let confirmed = resolve_export_destination(
            &dir_str,
            "existing.csv",
            UsageExportFormat::Csv,
            true,
        )
        .unwrap();
        assert!(matches!(confirmed, ExportDestination::Write(_)));

        // An unusable directory is rejected before any write.
        let missing = dir.path().join("missing-dir");
        assert!(resolve_export_destination(
            &missing.to_string_lossy(),
            "fresh.csv",
            UsageExportFormat::Csv,
            false,
        )
        .is_err());

        // A file used as a directory is rejected before any write.
        let as_file = dir.path().join("existing.csv");
        assert!(resolve_export_destination(
            &as_file.to_string_lossy(),
            "fresh.csv",
            UsageExportFormat::Csv,
            false,
        )
        .is_err());
    }
}
