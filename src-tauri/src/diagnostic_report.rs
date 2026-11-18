//! Operator-facing redacted provider diagnostic (test-only module).
//!
//! The unified acceptance command (`scripts/provider-quota-diagnostic.ps1`)
//! runs the single ignored probe below (`five_provider_diagnostic_report`)
//! through `cargo test`: it invokes the production fetch paths exactly
//! as the runtime does — the same read-only credential resolution, the same
//! normalization, no inference, no quota spend, no credential rotation — and
//! prints one redacted row per provider.
//!
//! Redaction contract: a row carries only normalized, already-safe fields —
//! the canonical health, the account attribution the cards themselves render
//! (masked hints), window labels/percent/resets, and a normalized failure
//! category. Structured error *messages* are deliberately NOT emitted (they
//! are display-safe but may embed transport detail); the report speaks in
//! codes. `secret_shaped_material` is the same scan the PowerShell wrapper
//! applies to the captured output before declaring it clean.

use chrono::{SecondsFormat, Utc};

use crate::runtime::{failure_health, production_specs, ProviderFailure, ProviderUsageDto};

/// Renders one provider's redacted report row. Pure so the redaction
/// contract is unit-testable; the live probe below only prints it.
///
/// `checked_at` is the moment the caller observed the result (the runtime
/// stamps `checkedAt` at cycle completion, which a direct probe bypasses).
pub(crate) fn format_row(
    id: &str,
    name: &str,
    checked_at: &str,
    result: &Result<ProviderUsageDto, ProviderFailure>,
) -> String {
    let mut out = format!("== {name} ({id}) ==\n");
    match result {
        Ok(usage) => {
            // Canonical health, computed by the same runtime vocabulary the
            // snapshot broadcasts (`ProviderHealth::as_str`).
            out.push_str(&format!("health: {}\n", usage.health.as_str()));
            out.push_str(&format!("checkedAt: {checked_at}\n"));
            // Some manual/local providers intentionally return a normalized
            // DTO for an unavailable attempt instead of a transport Err.
            // Surface only their stable category, never the free-form error.
            if let Some(category) = &usage.error_category {
                out.push_str(&format!("reason: {category}\n"));
            }
            if let Some(freshness) = usage.data_freshness {
                out.push_str(&format!("freshness: {freshness}\n"));
            }
            if let Some(source_updated_at) = &usage.source_updated_at {
                out.push_str(&format!("sourceUpdatedAt: {source_updated_at}\n"));
            }
            match &usage.account {
                Some(account) => {
                    out.push_str(&format!("credential: {}\n", account.label));
                    if let Some(identity) = &account.identity {
                        out.push_str(&format!("account: {identity}\n"));
                    }
                }
                None => {
                    // Antigravity's accepted v0.6 attribution is WEAK
                    // (activeIndex selection, no identity); the source label
                    // still names where the data comes from.
                    if id == "antigravity" {
                        out.push_str("credential: Antigravity plugin cache (active account selection)\n");
                    } else {
                        out.push_str("credential: <unattributed>\n");
                    }
                    out.push_str("account: <none>\n");
                }
            }
            if usage.limits.is_empty() {
                out.push_str("windows: <none reported>\n");
            }
            for limit in &usage.limits {
                out.push_str(&format!(
                    "  {}: used {:.1}% reset {}\n",
                    limit.label,
                    limit.used_percent,
                    limit.reset_at.as_deref().unwrap_or("-")
                ));
            }
        }
        // A failure still produces a useful row: canonical health category,
        // the stable failure code (never the raw message or provider text),
        // and the masked identity of the credential that was attempted, when
        // it could be resolved.
        Err(failure) => {
            out.push_str(&format!("health: {}\n", failure_health(failure).as_str()));
            out.push_str(&format!("checkedAt: {checked_at}\n"));
            out.push_str(&format!("reason: {}\n", failure.code));
            if let Some(status) = failure.http_status {
                out.push_str(&format!("httpStatus: {status}\n"));
            }
            match &failure.identity {
                Some(identity) => out.push_str(&format!("account: {identity}\n")),
                None => out.push_str("account: <none resolved>\n"),
            }
        }
    }
    out
}

/// Credential-shaped patterns the diagnostic output must never contain.
/// Mirrored by the PowerShell wrapper's scan over the captured output; kept
/// here so the redaction contract is pinned by tests in one place.
const SECRET_SCAN_PATTERNS: &[(&str, &str)] = &[
    ("Bearer token", r"(?i)bearer\s+[A-Za-z0-9._~+/=-]{8,}"),
    ("Authorization header", r"(?i)authorization\s*:"),
    ("sk- API key", r"sk-[A-Za-z0-9_-]{8,}"),
    ("JWT-shaped token", r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
    ("refresh token field", r"(?i)refresh[_-]?token"),
    ("access token field", r"(?i)access[_-]?token"),
    // Any PEM block; the prefix alone is enough for an output scan (the
    // full header phrase is deliberately not spelled out so the repo-wide
    // source secret scanner cannot trip over this file).
    ("PEM private key", r"-----BEGIN"),
];

/// The first credential-shaped pattern found in `text`, if any.
pub(crate) fn secret_shaped_material(text: &str) -> Option<&'static str> {
    use regex::Regex;
    for (name, pattern) in SECRET_SCAN_PATTERNS {
        if Regex::new(pattern).unwrap().is_match(text) {
            return Some(name);
        }
    }
    None
}

// The release declaration is deliberately independent of application settings.
// Deserialize into a list so duplicate family declarations cannot be hidden by JSON maps.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseScope {
    schema_version: u32,
    providers: Vec<ScopeEntry>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeEntry {
    id: String,
    state: ScopeState,
    date: String,
    reason: String,
}

#[derive(serde::Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ScopeState { Active, Inactive }

// Only the Codex family has account-specific IDs. All other IDs are exact.
fn family(id: &str) -> &str {
    if id == "openai-codex:secondary" { "openai-codex" } else { id }
}

fn validated_scope(json: &str, ids: &[&str], today: &str) -> Result<ReleaseScope, String> {
    let scope: ReleaseScope = serde_json::from_str(json).map_err(|_| "invalid scope JSON or fields")?;
    if scope.schema_version != 1 { return Err("unsupported scope schema".into()); }
    let mut seen = std::collections::HashSet::new();
    for entry in &scope.providers {
        if !ids.iter().any(|id| family(id) == entry.id) || !seen.insert(&entry.id) {
            return Err("unknown or duplicate provider family".into());
        }
        // Require a fresh explicit recertification for each release day (UTC).
        if entry.date != today { return Err("scope date must be today in UTC".into()); }
        let reason = entry.reason.trim();
        if reason.is_empty() || reason.chars().count() > 200 || reason.chars().any(|c| c.is_control())
            || secret_shaped_material(reason).is_some() {
            return Err("invalid scope reason".into());
        }
    }
    Ok(scope)
}

fn inactive_entry<'a>(scope: Option<&'a ReleaseScope>, id: &str) -> Option<&'a ScopeEntry> {
    scope?.providers.iter().find(|entry| entry.id == family(id) && entry.state == ScopeState::Inactive)
}

fn required_ids<'a>(ids: &[&'a str], scope: Option<&ReleaseScope>) -> Vec<&'a str> {
    ids.iter().copied().filter(|id| inactive_entry(scope, id).is_none()).collect()
}

fn format_excluded(id: &str, name: &str, entry: &ScopeEntry) -> String {
    format!("== {name} ({id}) ==\nscope: EXCLUDED_INACTIVE\nresult: UNVERIFIED\ndeclarationDate: {}\nevidence: operator declaration\n", entry.date)
}

fn result_for(result: &Result<ProviderUsageDto, ProviderFailure>) -> &'static str {
    let health = match result { Ok(usage) => usage.health.as_str(), Err(failure) => failure_health(failure).as_str() };
    match health { "live" => "PASS", "error" => "FAIL", _ => "UNVERIFIED" }
}

/// Live probe: validate the entire declaration before starting any fetch. Without
/// a declaration every registered slot remains required, as before.
#[test]
#[ignore = "live probe: reads local credential stores and the network; redacted output only"]
fn five_provider_diagnostic_report() {
    let specs = production_specs();
    let ids: Vec<_> = specs.iter().map(|spec| spec.kind.id()).collect();
    let scope = match std::env::var_os("LIMITSCOPE_RELEASE_SCOPE_FILE") {
        None => None,
        Some(path) => {
            let json = std::fs::read_to_string(path).expect("cannot read release scope declaration");
            let today = Utc::now().format("%Y-%m-%d").to_string();
            Some(validated_scope(&json, &ids, &today).expect("invalid release scope declaration"))
        }
    };
    let required = required_ids(&ids, scope.as_ref());
    let rows = tauri::async_runtime::block_on(async {
        let mut join_set = tokio::task::JoinSet::new();
        for spec in specs.iter().filter(|spec| required.contains(&spec.kind.id())) {
            let spec = spec.clone();
            join_set.spawn(async move {
                let result = (spec.fetch)().await;
                (spec.kind.id().to_string(), spec.kind.name().to_string(), result)
            });
        }
        let mut rows = Vec::new();
        while let Some(joined) = join_set.join_next().await {
            rows.push(joined.expect("required provider probe failed to complete"));
        }
        rows
    });
    let mut report = String::new();
    for spec in &specs {
        let id = spec.kind.id();
        if let Some(entry) = inactive_entry(scope.as_ref(), id) {
            report.push_str(&format_excluded(id, spec.kind.name(), entry));
            report.push('\n');
        } else {
            let (id, name, result) = rows.iter().find(|(row_id, _, _)| row_id == id)
                .expect("required provider result missing");
            let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            report.push_str(&format!("{}scope: REQUIRED\nresult: {}\n", format_row(id, name, &checked_at, result), result_for(result)));
        }
    }
    assert!(secret_shaped_material(&report).is_none(), "diagnostic report failed secret scan");
    print!("{report}");
}

// ---------- redaction contract tests (deterministic, no network) ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{
        failure_health, normalize_opencode_go, ProviderHealth, ProviderKind,
    };
    use crate::opencode_go::{OpenCodeGoAccount, OpenCodeGoLimitWindow, OpenCodeGoUsage};
    use crate::provider_error::ProviderError;
    use chrono::TimeZone;

    fn fixed_checked_at() -> String {
        chrono::Utc
            .with_ymd_and_hms(2026, 9, 30, 2, 0, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn sample_live_dto() -> ProviderUsageDto {
        // The plausibility gate evaluates bounds against the fetch-time
        // clock; the fixture pins it so the Weekly window's reset stays
        // plausible regardless of when the suite runs.
        let now_ms = chrono::Utc
            .with_ymd_and_hms(2026, 9, 30, 2, 0, 0)
            .unwrap()
            .timestamp_millis();
        normalize_opencode_go(
            Ok(OpenCodeGoUsage {
                limits: vec![OpenCodeGoLimitWindow {
                    label: "Weekly".to_string(),
                    used_percent: 8.4,
                    reset_at: Some("2026-10-05T00:00:00.000Z".to_string()),
                }],
                account: Some(OpenCodeGoAccount { identity: Some("key:avmF".into()), key_hint: "avmF".to_string(),
                }),
            }),
            now_ms,
        )
        .unwrap()
    }

    fn sample_failure(code: &str, message: &str) -> ProviderFailure {
        let error = ProviderError::new(code, message).with_identity_hint(Some(
            "key:avmF".to_string(),
        ));
        match normalize_opencode_go(Err(error), 0) {
            Err(failure) => failure,
            Ok(_) => panic!("expected a failure"),
        }
    }

    // 10./12. The success row shows only masked, already-rendered fields —
    // a credential can never travel through any field the formatter reads.
    #[test]
    fn success_rows_are_redacted_and_canonical() {
        let dto = sample_live_dto();
        let row = format_row("opencode-go", "OpenCode Go", &fixed_checked_at(), &Ok(dto));
        assert!(row.contains("== OpenCode Go (opencode-go) =="), "row: {row}");
        assert!(row.contains("health: live"), "row: {row}");
        assert!(row.contains("checkedAt: 2026-09-30T02:00:00.000Z"), "row: {row}");
        assert!(row.contains("credential: key ••avmF"), "row: {row}");
        assert!(row.contains("account: key:avmF"), "row: {row}");
        assert!(row.contains("Weekly: used 8.4% reset 2026-10-05T00:00:00.000Z"), "row: {row}");
        assert!(secret_shaped_material(&row).is_none(), "row: {row}");
    }

    #[test]
    fn normalized_unavailable_dto_keeps_its_safe_reason() {
        let mut dto = sample_live_dto();
        dto.health = ProviderHealth::Unavailable;
        dto.status = ProviderHealth::Unavailable.legacy_status();
        dto.limits.clear();
        dto.error = None;
        dto.error_category = Some("manual_refresh_required".to_string());
        let row = format_row("opendesign", "OpenDesign", &fixed_checked_at(), &Ok(dto));
        assert!(row.contains("health: unavailable"), "row: {row}");
        assert!(row.contains("reason: manual_refresh_required"), "row: {row}");
    }

    // Failure rows carry the normalized category (never the message) and
    // the canonical unavailable/error health split.
    #[test]
    fn failure_rows_use_normalized_categories_and_canonical_health() {
        // Source-absent family → canonical `unavailable` (health contract).
        let failure = sample_failure("credential_missing", "SECRET-MARKER must never appear");
        let row = format_row("grok", "Grok (xAI)", &fixed_checked_at(), &Err(failure));
        assert!(row.contains("health: unavailable"), "row: {row}");
        assert!(row.contains("checkedAt: 2026-09-30T02:00:00.000Z"), "row: {row}");
        assert!(row.contains("reason: credential_missing"), "row: {row}");
        assert!(row.contains("account: key:avmF"), "row: {row}");
        assert!(!row.contains("SECRET-MARKER"), "row: {row}");
        assert!(!row.contains("must never appear"), "row: {row}");
        assert_eq!(
            failure_health(&sample_failure("credential_missing", "x")).as_str(),
            "unavailable"
        );

        // Everything else (transport, HTTP, schema) → canonical `error`.
        let failure = sample_failure("network", "offline SECRET-MARKER");
        let row = format_row("grok", "Grok (xAI)", &fixed_checked_at(), &Err(failure));
        assert!(row.contains("health: error"), "row: {row}");
        assert!(row.contains("reason: network"), "row: {row}");
        assert!(!row.contains("SECRET-MARKER"), "row: {row}");
    }

    // Unknown-health successes stay canonical, and an unattributed provider
    // row says so instead of inventing a hint.
    #[test]
    fn zero_window_success_rows_stay_canonical_and_unattributed_says_none() {
        let mut dto = sample_live_dto();
        dto.health = ProviderHealth::Unknown;
        dto.limits.clear();
        dto.account = None;
        let row = format_row("openai-codex", "OpenAI / Codex", &fixed_checked_at(), &Ok(dto));
        assert!(row.contains("health: unknown"), "row: {row}");
        assert!(row.contains("windows: <none reported>"), "row: {row}");
        assert!(row.contains("account: <none>"), "row: {row}");

        // Antigravity's WEAK attribution is accepted: the row names the
        // credential source without inventing an identity.
        let mut antigravity = sample_live_dto();
        antigravity.account = None;
        let row = format_row("antigravity", "Google Antigravity", &fixed_checked_at(), &Ok(antigravity));
        assert!(row.contains("credential: Antigravity plugin cache"), "row: {row}");
        assert!(row.contains("account: <none>"), "row: {row}");
    }

    // 12. The scan trips on every credential shape and passes the redacted
    // rows the formatter can produce.
    #[test]
    fn secret_scan_catches_credential_shapes() {
        let samples = [
            "Bearer abc123def456",
            "authorization: Bearer x",
            "key sk-abcdef1234567890",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.QUJDREVGR0hJSktMTU5PUA",
            "refresh_token: abc",
            "access-token: abc",
            // Assembled at runtime so the source itself never contains a
            // PEM-header shape the repo-wide scanner would flag.
            &format!("-----BEGIN {}-----", "PRIVATE KEY"),
        ];
        for sample in samples {
            assert!(
                secret_shaped_material(sample).is_some(),
                "missed credential shape: {sample}"
            );
        }
        // The JWT pattern must not fire on ordinary dotted text.
        assert!(secret_shaped_material("reset 2026-10-05T00:00:00.000Z v1.2.3").is_none());
    }

    // The production providers are all covered by the probe.
    #[test]
    fn the_probe_covers_the_production_registry() {
        let specs = production_specs();
        assert_eq!(specs.len(), 11);
        let ids: Vec<_> = specs.iter().map(|s| s.kind.id()).collect();
        for id in [
            "openai-codex",
            "openai-codex:secondary",
            "claude-code",
            "zai",
            "opencode-go",
            "antigravity",
            "grok",
            "qwen-token-plan",
            "minimax-token-plan",
            "mistral",
            "opendesign",
        ] {
            assert!(ids.contains(&id), "probe misses {id}");
        }
        // ProviderKind must be nameable for the report; silence the unused
        // import if the registry drifts.
        let _ = ProviderKind::Codex.id();
    }
    #[test]
    fn release_scope_validates_and_selects_registry_slots_offline() {
        // Fixture only: production_specs() resolves Codex home metadata.
        let ids = vec!["openai-codex", "openai-codex:secondary", "claude-code", "zai", "opencode-go", "antigravity", "grok", "qwen-token-plan", "minimax-token-plan", "opendesign"];
        let date = "2026-11-17";
        let json = r#"{"schemaVersion":1,"providers":[{"id":"openai-codex","state":"INACTIVE","date":"2026-11-17","reason":"Not active for this release"}]}"#;
        let scope = validated_scope(json, &ids, date).unwrap();
        assert!(inactive_entry(Some(&scope), "openai-codex").is_some());
        let secondary = inactive_entry(Some(&scope), "openai-codex:secondary").unwrap();
        let row = format_excluded("openai-codex:secondary", "Codex", secondary);
        assert!(row.contains("openai-codex:secondary"));
        assert!(row.contains("scope: EXCLUDED_INACTIVE\nresult: UNVERIFIED"));
        assert!(row.contains("declarationDate: 2026-11-17\nevidence: operator declaration"));
        assert!(!row.contains("Not active for this release"));
        for id in ids.iter().filter(|id| family(id) != "openai-codex") {
            assert!(inactive_entry(Some(&scope), id).is_none());
        }
        assert_eq!(required_ids(&ids, None), ids);
        // A fixture for the five operator-declared families; never a default.
        let families = ["openai-codex", "zai", "opencode-go", "antigravity", "grok"];
        let entries: Vec<_> = families.iter().map(|id| serde_json::json!({
            "id": id, "state": "INACTIVE", "date": date, "reason": "fixture"
        })).collect();
        let five = serde_json::json!({"schemaVersion": 1, "providers": entries}).to_string();
        let five = validated_scope(&five, &ids, date).unwrap();
        let scheduled = required_ids(&ids, Some(&five));
        assert!(ids.iter().filter(|id| families.contains(&family(id))).all(|id| !scheduled.contains(id)));
        assert!(ids.iter().filter(|id| !families.contains(&family(id))).all(|id| scheduled.contains(id)));
        let active = json.replace("INACTIVE", "ACTIVE");
        let active = validated_scope(&active, &ids, date).unwrap();
        assert!(inactive_entry(Some(&active), "openai-codex").is_none());
        for invalid in [
            json.replace("schemaVersion\":1", "schemaVersion\":2"),
            json.replace("openai-codex", "unknown"),
            json.replace("openai-codex", "openai-codex:secondary"),
            json.replace("INACTIVE", "inactive"),
            json.replace("2026-11-17", "2026-11-16"),
            json.replace("Not active for this release", "   "),
            json.replace("\"providers\":", "\"extra\":true,\"providers\":"),
            json.replace("}]}", "},{\"id\":\"openai-codex\",\"state\":\"ACTIVE\",\"date\":\"2026-11-17\",\"reason\":\"duplicate\"}]}"),
            "not json".to_string(),
        ] { assert!(validated_scope(&invalid, &ids, date).is_err(), "accepted {invalid}"); }
    }

    #[test]
    fn release_scope_reason_counts_unicode_characters() {
        let ids = ["zai"];
        let date = "2026-11-17";
        for (length, valid) in [(200, true), (201, false)] {
            let json = serde_json::json!({
                "schemaVersion": 1,
                "providers": [{
                    "id": "zai", "state": "INACTIVE", "date": date,
                    "reason": "\u{0105}".repeat(length)
                }]
            }).to_string();
            assert_eq!(validated_scope(&json, &ids, date).is_ok(), valid);
        }
    }

    #[test]
    fn release_scope_result_preserves_non_live_results() {
        let live = sample_live_dto();
        assert_eq!(result_for(&Ok(live.clone())), "PASS");
        let mut unknown = live;
        unknown.health = ProviderHealth::Unknown;
        assert_eq!(result_for(&Ok(unknown)), "UNVERIFIED");
        assert_eq!(result_for(&Err(sample_failure("network", "offline"))), "FAIL");
        assert_eq!(result_for(&Err(sample_failure("credential_missing", "missing"))), "UNVERIFIED");
    }

}
