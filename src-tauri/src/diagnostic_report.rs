//! Operator-facing redacted provider diagnostic (test-only module).
//!
//! The unified acceptance command (`scripts/provider-quota-diagnostic.ps1`)
//! runs the single ignored probe below (`five_provider_diagnostic_report`)
//! through `cargo test`: it invokes the five production fetch paths exactly
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

/// The sanctioned read-only five-provider probe. Runs the production fetch
/// for every registered provider (concurrently, each bounded by its own
/// request timeout) and prints the redacted rows in registry order. No
/// secrets are printed; the wrapper scans the captured output anyway.
#[test]
#[ignore = "live probe: reads local credential stores and the network; redacted output only"]
fn five_provider_diagnostic_report() {
    let specs = production_specs();
    let rows = tauri::async_runtime::block_on(async {
        let mut join_set = tokio::task::JoinSet::new();
        for spec in &specs {
            let spec = spec.clone();
            join_set.spawn(async move {
                let result = (spec.fetch)().await;
                (spec.kind.id().to_string(), spec.kind.name().to_string(), result)
            });
        }
        let mut rows = Vec::new();
        while let Some(joined) = join_set.join_next().await {
            if let Ok(row) = joined {
                rows.push(row);
            }
        }
        rows
    });
    for spec in &specs {
        if let Some((id, name, result)) =
            rows.iter().find(|(row_id, _, _)| *row_id == spec.kind.id())
        {
            let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            print!("{}", format_row(id, name, &checked_at, result));
        }
    }
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
                account: Some(OpenCodeGoAccount {
                    key_hint: "avmF".to_string(),
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

    // The five production providers are all covered by the probe.
    #[test]
    fn the_probe_covers_the_production_registry() {
        let specs = production_specs();
        assert_eq!(specs.len(), 5);
        let ids: Vec<_> = specs.iter().map(|s| s.kind.id()).collect();
        for id in ["openai-codex", "zai", "opencode-go", "antigravity", "grok"] {
            assert!(ids.contains(&id), "probe misses {id}");
        }
        // ProviderKind must be nameable for the report; silence the unused
        // import if the registry drifts.
        let _ = ProviderKind::Codex.id();
    }
}
