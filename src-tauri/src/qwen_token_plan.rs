//! Read-only Qwen Cloud Token Plan quota from the official Bailian CLI.
//!
//! The verified source is `bl usage token-plan --console-site international
//! --console-region ap-southeast-1 --output json`. International Singapore
//! plans currently expose one monthly window: `per1MonthPercentage` is the
//! fraction used (0–1) and `per1MonthResetTime` is epoch milliseconds.
//! The adapter accepts only those fields. A missing CLI or console login is
//! unavailable; it is never reported as zero usage.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use tokio::process::Command;

use crate::provider_error::ProviderError;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MONTHLY_LABEL: &str = "Monthly";
/// Epoch milliseconds. Seconds-shaped values are rejected, not promoted.
const MIN_RESET_MS: i64 = 1_000_000_000_000;
/// 2100-01-01T00:00:00Z. Later stamps are not a monthly reset.
const MAX_RESET_MS: i64 = 4_102_444_800_000;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QwenLimit {
    pub label: String,
    pub used_percent: f64,
    pub reset_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QwenTokenPlanUsage {
    pub limits: Vec<QwenLimit>,
    pub plan_type: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BailianLaunch {
    pub node: PathBuf,
    pub script: PathBuf,
}

pub(crate) fn token_plan_args() -> [&'static str; 8] {
    [
        "usage",
        "token-plan",
        "--console-site",
        "international",
        "--console-region",
        "ap-southeast-1",
        "--output",
        "json",
    ]
}

pub(crate) async fn get_qwen_token_plan_usage() -> Result<QwenTokenPlanUsage, ProviderError> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let launch = resolve_bailian_launch(&path)?;
    let output = run_bailian(&launch).await?;
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(unexpected(
            "Bailian CLI returned an oversized Token Plan response.",
        ));
    }
    parse_token_plan_stdout(&output)
}

pub(crate) fn resolve_bailian_launch(path: &OsString) -> Result<BailianLaunch, ProviderError> {
    let mut node_on_path: Option<PathBuf> = None;
    let mut script: Option<PathBuf> = None;
    let mut shim_dir: Option<PathBuf> = None;
    for dir in std::env::split_paths(path) {
        if node_on_path.is_none() {
            node_on_path = existing_file(&dir, &["node.exe", "node"]);
        }
        if script.is_none() {
            if existing_file(&dir, &["bl.cmd", "bl", "bailian.cmd", "bailian"]).is_some() {
                let candidate = dir.join("node_modules/bailian-cli/dist/bailian.mjs");
                if candidate.is_file() {
                    script = Some(candidate);
                    shim_dir = Some(dir);
                }
            }
        }
    }
    let script = script.ok_or_else(|| {
        ProviderError::new(
            "bailian_not_installed",
            "Bailian CLI (bl) is not installed.",
        )
    })?;
    let node = shim_dir
        .as_ref()
        .and_then(|dir| existing_file(dir, &["node.exe"]))
        .or(node_on_path)
        .ok_or_else(|| {
            ProviderError::new(
                "node_not_installed",
                "Node.js is required to run the Bailian CLI.",
            )
        })?;
    Ok(BailianLaunch { node, script })
}

async fn run_bailian(launch: &BailianLaunch) -> Result<String, ProviderError> {
    let mut command = Command::new(&launch.node);
    command
        .arg(&launch.script)
        .args(token_plan_args())
        .env("NODE_USE_SYSTEM_CA", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }
    let child = command
        .spawn()
        .map_err(|_| ProviderError::transient("network", "Could not start the Bailian CLI."))?;
    match tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output()).await {
        Err(_) => {
            let mut error = ProviderError::transient("network", "Bailian CLI timed out.");
            error.transport_timeout = true;
            Err(error)
        }
        Ok(Err(_)) => Err(ProviderError::transient(
            "network",
            "Could not read the Bailian CLI response.",
        )),
        Ok(Ok(output)) => {
            if !output.status.success() {
                return Err(cli_failure(&output.stdout, &output.stderr));
            }
            stdout_text(&output.stdout)
        }
    }
}

fn cli_failure(stdout: &[u8], stderr: &[u8]) -> ProviderError {
    if looks_like_missing_console_token(stdout) || looks_like_missing_console_token(stderr) {
        return ProviderError::new(
            "not_logged_in",
            "Bailian console login is required for the international Token Plan.",
        );
    }
    unexpected("Bailian CLI could not read the Token Plan quota.")
}

fn looks_like_missing_console_token(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    text.contains("No console access token")
}

fn stdout_text(stdout: &[u8]) -> Result<String, ProviderError> {
    if stdout.len() > MAX_OUTPUT_BYTES {
        return Err(unexpected(
            "Bailian CLI returned an oversized Token Plan response.",
        ));
    }
    String::from_utf8(stdout.to_vec())
        .map_err(|_| unexpected("Bailian CLI returned a non-UTF-8 Token Plan response."))
}

pub(crate) fn parse_token_plan_stdout(stdout: &str) -> Result<QwenTokenPlanUsage, ProviderError> {
    let value: Value = serde_json::from_str(stdout.trim())
        .map_err(|_| unexpected("Bailian CLI returned an unreadable Token Plan response."))?;
    let object = value
        .as_object()
        .ok_or_else(|| unexpected("Bailian CLI returned an unreadable Token Plan response."))?;
    if object.contains_key("error") {
        return Err(cli_failure(stdout.as_bytes(), b""));
    }
    let Some(percentage) = object.get("per1MonthPercentage") else {
        return Err(unexpected(
            "Bailian CLI did not return a monthly Token Plan window.",
        ));
    };
    let used_percent = usage_percent(percentage)?;
    let reset_at = match object.get("per1MonthResetTime") {
        None | Some(Value::Null) => None,
        Some(reset) => Some(reset_rfc3339(reset)?),
    };
    Ok(QwenTokenPlanUsage {
        limits: vec![QwenLimit {
            label: MONTHLY_LABEL.to_string(),
            used_percent,
            reset_at,
        }],
        plan_type: "token-plan",
    })
}

fn usage_percent(value: &Value) -> Result<f64, ProviderError> {
    let number = value
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| unexpected("Bailian CLI returned an unreadable monthly quota."))?;
    if !(0.0..=1.0).contains(&number) {
        return Err(unexpected(
            "Bailian CLI returned a monthly quota outside the 0–1 fraction.",
        ));
    }
    Ok(number * 100.0)
}

fn reset_rfc3339(value: &Value) -> Result<String, ProviderError> {
    let ms = match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
    .filter(|ms| (MIN_RESET_MS..=MAX_RESET_MS).contains(ms))
    .ok_or_else(|| unexpected("Bailian CLI returned an unreadable monthly reset."))?;
    let stamp: DateTime<Utc> = DateTime::from_timestamp_millis(ms)
        .ok_or_else(|| unexpected("Bailian CLI returned an unreadable monthly reset."))?;
    Ok(stamp.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn unexpected(message: &str) -> ProviderError {
    ProviderError::new("unexpected_response", message)
}

fn existing_file(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_verified_monthly_fraction_and_reset() {
        let usage = parse_token_plan_stdout(
            r#"{"per1MonthPercentage":0.4377994076157289,"per1MonthResetTime":1794844800000}"#,
        )
        .expect("monthly window");
        assert_eq!(usage.plan_type, "token-plan");
        assert_eq!(usage.limits.len(), 1);
        assert_eq!(usage.limits[0].label, "Monthly");
        assert!((usage.limits[0].used_percent - 43.77994076157289).abs() < 1e-9);
        assert_eq!(
            usage.limits[0].reset_at.as_deref(),
            Some("2026-11-16T16:00:00Z")
        );
    }

    #[test]
    fn zero_usage_without_a_reset_is_a_real_window() {
        let usage = parse_token_plan_stdout(r#"{"per1MonthPercentage":0}"#).expect("zero");
        assert_eq!(usage.limits[0].used_percent, 0.0);
        assert_eq!(usage.limits[0].reset_at, None);
    }

    #[test]
    fn missing_window_is_not_reported_as_zero() {
        let error = parse_token_plan_stdout(r#"{"unused":true}"#).expect_err("missing");
        assert_eq!(error.code, "unexpected_response");
        assert!(!error.message.contains('0'));
    }

    #[test]
    fn extra_fields_and_secret_shaped_text_never_enter_the_result() {
        let raw = r#"{"per1MonthPercentage":0.25,"per1MonthResetTime":1794844800000,"apiKey":"sk-secretsecretsecretsecret","note":"ignore me"}"#;
        let usage = parse_token_plan_stdout(raw).expect("parsed");
        let rendered = format!("{usage:?}");
        assert!(!rendered.contains("sk-"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("ignore me"));
        assert!((usage.limits[0].used_percent - 25.0).abs() < 1e-9);
    }

    #[test]
    fn console_login_failure_is_unavailable_and_does_not_echo_the_body() {
        let body = r#"{"error":{"code":3,"message":"No console access token found. sk-secretsecretsecretsecret","hint":"Run bl auth login --console"}}"#;
        let error = parse_token_plan_stdout(body).expect_err("auth");
        assert_eq!(error.code, "not_logged_in");
        assert!(!error.message.contains("sk-"));
        assert!(!error.message.contains("hint"));
        assert_eq!(error.transient, Some(false));
    }

    #[test]
    fn rejects_percent_style_numbers_and_invalid_resets() {
        assert_eq!(
            parse_token_plan_stdout(r#"{"per1MonthPercentage":43.7}"#)
                .expect_err("percent")
                .code,
            "unexpected_response"
        );
        assert!(parse_token_plan_stdout(
            r#"{"per1MonthPercentage":0.5,"per1MonthResetTime":1794844800}"#
        )
        .is_err());
    }

    #[test]
    fn command_shape_is_the_verified_read_only_invocation() {
        assert_eq!(
            token_plan_args(),
            [
                "usage",
                "token-plan",
                "--console-site",
                "international",
                "--console-region",
                "ap-southeast-1",
                "--output",
                "json",
            ]
        );
    }

    #[test]
    fn resolves_the_npm_shim_without_using_a_shell() {
        let root =
            std::env::temp_dir().join(format!("limitscope-bl-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        let script = bin.join("node_modules/bailian-cli/dist/bailian.mjs");
        std::fs::create_dir_all(script.parent().expect("script parent")).expect("dirs");
        std::fs::write(bin.join("bl.cmd"), b"@echo off").expect("shim");
        std::fs::write(&script, b"export {}").expect("script");
        std::fs::write(bin.join("node.exe"), b"").expect("node");
        let launch = resolve_bailian_launch(&bin.as_os_str().to_os_string()).expect("launch");
        assert_eq!(launch.node, bin.join("node.exe"));
        assert_eq!(launch.script, script);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_cli_is_not_installed() {
        let error = resolve_bailian_launch(&OsString::from("C:\\missing")).expect_err("missing");
        assert_eq!(error.code, "bailian_not_installed");
    }

    #[tokio::test]
    #[ignore = "reads the local international Bailian console Token Plan"]
    async fn live_token_plan_returns_the_monthly_window() {
        let usage = get_qwen_token_plan_usage().await.expect("live token plan");
        assert_eq!(usage.limits.len(), 1);
        let window = &usage.limits[0];
        assert_eq!(window.label, "Monthly");
        assert!((0.0..=100.0).contains(&window.used_percent));
        let reset = DateTime::parse_from_rfc3339(window.reset_at.as_deref().expect("reset"))
            .expect("rfc3339")
            .with_timezone(&Utc);
        let now = Utc::now();
        assert!(reset > now);
        assert!(reset < now + chrono::Duration::days(35));
    }
}
