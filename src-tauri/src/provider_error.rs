//! Shared structured error for the live provider backends (Codex, Z.ai,
//! OpenCode Go).
//!
//! Every live backend surfaces failures to the frontend as the wire shape
//! `{ code, message, httpStatus?, transient?, retryAfterMs? }` (see
//! `CommandError` in `src/types.ts`). This module is the single source of
//! that shape and of the retry-verdict rule, so the per-backend copies can
//! never disagree:
//!
//! - `transient` is always explicit (`Some(bool)`). `true` only for transport
//!   failures (`transient`) and 429/5xx responses (`http_failure`); every
//!   deterministic failure — auth, entitlement, schema changes, local init —
//!   carries `Some(false)` and is never retried by `src/lib/transientRetry.ts`.
//! - Transport timeouts additionally carry an internal `transport_timeout`
//!   marker (never serialized): the runtime's bounded-retry rule treats a
//!   timeout as terminal for the current cycle — the stall that consumed the
//!   whole request budget will not clear 750 ms later — and lets the next
//!   cadence/scheduler cycle serve as the retry. Fast-failing transport
//!   errors (refused connections, DNS misses) keep the one fast retry.
//! - `httpStatus` is present only when the failure came from a non-success
//!   HTTP response, and is omitted otherwise.
//! - `retryAfterMs` is present only when a 429/5xx response carried a
//!   parseable standard `Retry-After` header (delta-seconds or HTTP-date,
//!   one shared parser below); the shared runtime converts it into a
//!   capped provider cooldown.
//! - `code` reuses the shared vocabulary (`network`, `unexpected_response`,
//!   `unexpected`, `credential_missing`, `auth_unreadable`, `auth_invalid`,
//!   `auth_expired`, `not_entitled`, provider-prefixed `*_not_installed`).
//!   Provider-specific codes with actionable meaning (e.g. Codex's
//!   `not_logged_in`) are kept — codes are never renamed for uniformity.
//!
//! Messages are display-safe: they never embed tokens, API keys, or file
//! contents. The struct holds no secret fields, so nothing else can leak
//! through `Debug` or serialization.
//!
//! The passive Antigravity backend stays bespoke on purpose: it performs no
//! HTTP, all its failures are deterministic, and its two-field
//! `{ code, message }` wire shape (see `antigravity.rs`) omits `transient`
//! entirely — migrating it onto this type would add `transient: false` to
//! that shape for no consumer benefit.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Structured error surfaced to the frontend as
/// `{ code, message, httpStatus?, transient?, retryAfterMs? }`. `transient`
/// is the retry verdict consumed by `src/lib/transientRetry.ts`
/// (absent/`false` means never retried); `httpStatus` carries the offending
/// HTTP status when the failure came from a non-success response.
/// `retryAfterMs` carries a parsed server `Retry-After` hint from a 429/5xx
/// response; the shared runtime turns it into a provider cooldown.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transient: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Internal account attribution for the runtime's last-good guard: the
    /// masked identity of the credential this refresh actually attempted,
    /// set only after the local credential was resolved. Never serialized —
    /// the WebView receives the same identity through the usage DTO's
    /// `account` field.
    #[serde(skip)]
    pub identity_hint: Option<String>,
    /// Internal: the transport failure was a timeout — the request budget
    /// expired during connect, TLS, response, or body read. Never serialized;
    /// consumed only by the runtime's bounded-retry rule, which treats a
    /// transport timeout as terminal for the current cycle (an immediate
    /// second attempt cannot clear a stall that consumed the whole budget).
    #[serde(skip)]
    pub transport_timeout: bool,
}

impl ProviderError {
    /// Deterministic failure (default): the frontend never retries it.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            http_status: None,
            transient: Some(false),
            retry_after_ms: None,
            identity_hint: None,
            transport_timeout: false,
        }
    }

    /// Transient failure without an HTTP status (transport problems).
    pub fn transient(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            http_status: None,
            transient: Some(true),
            retry_after_ms: None,
            identity_hint: None,
            transport_timeout: false,
        }
    }

    /// A non-success HTTP response. 429/5xx are transient (they plausibly
    /// succeed on a second attempt); every other status is deterministic,
    /// matching the frontend's retry set.
    pub fn http_failure(status: reqwest::StatusCode, message: impl Into<String>) -> Self {
        Self {
            code: "unexpected_response".to_string(),
            message: message.into(),
            http_status: Some(status.as_u16()),
            transient: Some(
                status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error(),
            ),
            retry_after_ms: None,
            identity_hint: None,
            transport_timeout: false,
        }
    }

    /// Marks (or clears) the internal transport-timeout marker. Only the
    /// transport mapping in [`transport_failure`] sets it.
    pub fn with_transport_timeout(mut self, transport_timeout: bool) -> Self {
        self.transport_timeout = transport_timeout;
        self
    }

    /// Attaches the masked credential identity of the account this refresh
    /// attempted, so the runtime can tell a failure of account A apart from
    /// a failure that arrived while account B's credential is stored.
    pub fn with_identity_hint(mut self, identity_hint: Option<String>) -> Self {
        self.identity_hint = identity_hint;
        self
    }

    /// A non-success HTTP response with the server's `Retry-After` hint
    /// attached when one is present and parseable. Only the statuses the
    /// bounded-retry family honors (429/5xx) carry the hint; any other
    /// status ignores the header entirely.
    pub fn http_failure_with_retry_after(
        status: reqwest::StatusCode,
        message: impl Into<String>,
        retry_after_header: Option<&str>,
        now: DateTime<Utc>,
    ) -> Self {
        let retry_after_ms = if status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
        {
            retry_after_header.and_then(|value| retry_after_header_ms(value, now))
        } else {
            None
        };
        Self {
            retry_after_ms,
            ..Self::http_failure(status, message)
        }
    }

    /// Overrides the retry verdict, for failures whose transiency depends on
    /// a payload detail (Z.ai's HTTP-200 error envelope code).
    pub fn with_transient(mut self, transient: bool) -> Self {
        self.transient = Some(transient);
        self
    }
}

/// Extracts the `Retry-After` header value from a response, if readable as
/// text. Callers capture this before the response body is consumed.
pub fn retry_after_header<'a>(headers: &'a reqwest::header::HeaderMap) -> Option<&'a str> {
    headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()
}

/// The one mapping every live backend uses for a reqwest transport failure:
/// the error stays transient (`network` verdict), and a timeout — the
/// client's request budget expired during connect, TLS, response, or body
/// read — is additionally marked for the runtime's retry rule (terminal for
/// the current cycle). Fast transport refusals keep the one fast retry.
pub fn transport_failure(code: &str, message: String, error: &reqwest::Error) -> ProviderError {
    ProviderError::transient(code, message).with_transport_timeout(error.is_timeout())
}

/// The maximum cooldown the runtime will ever honor from a `Retry-After`
/// hint (24 hours). Larger hints are capped, never trusted blindly.
pub const MAX_COOLDOWN_MS: u64 = 24 * 60 * 60 * 1000;

/// Parses a standard `Retry-After` header value into a wait in
/// milliseconds: an integer delta-seconds, or an HTTP-date (only a future
/// date yields a wait; a past date means "eligible now", i.e. zero).
/// Malformed values return `None`, so the caller keeps its normal bounded
/// behavior. The raw value is uncapped — the cooldown layer applies
/// [`MAX_COOLDOWN_MS`].
pub fn retry_after_header_ms(value: &str, now: DateTime<Utc>) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(seconds) = trimmed.parse::<u64>() {
        return Some(seconds.saturating_mul(1000));
    }
    let date = DateTime::parse_from_rfc2822(trimmed).ok()?;
    let delta_ms = date
        .with_timezone(&Utc)
        .signed_duration_since(now)
        .num_milliseconds();
    Some(delta_ms.max(0) as u64)
}

// ---------- shared contract tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::Value;
    use std::net::TcpListener;
    use std::thread;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap()
    }

    fn wire_keys(error: &ProviderError) -> Vec<String> {
        // serde_json's object map yields keys in its own (sorted) order, so
        // callers compare sorted key sets, not wire order.
        let value: Value = serde_json::to_value(error).unwrap();
        let mut keys: Vec<String> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::clone)
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn wire_format_is_camel_case() {
        let wire: Value = serde_json::to_value(ProviderError::http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "HTTP 429 Too Many Requests.",
        ))
        .unwrap();
        assert_eq!(wire["code"], "unexpected_response");
        assert_eq!(wire["message"], "HTTP 429 Too Many Requests.");
        assert_eq!(wire["httpStatus"], 429);
        assert_eq!(wire["transient"], true);
        assert!(!wire.to_string().contains("http_status"));
    }

    #[test]
    fn wire_carries_no_fields_beyond_the_contract() {
        // Exactly the four documented fields — nothing else can leak.
        assert_eq!(
            wire_keys(&ProviderError::http_failure(
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                "rate limited",
            )),
            ["code", "httpStatus", "message", "transient"]
        );
    }

    #[test]
    fn absent_optionals_are_omitted_from_the_wire() {
        // Deterministic errors carry the explicit verdict but no status.
        assert_eq!(
            wire_keys(&ProviderError::new("auth_invalid", "rejected")),
            ["code", "message", "transient"]
        );
        let wire: Value = serde_json::to_value(ProviderError::new("auth_invalid", "rejected"))
            .unwrap();
        assert_eq!(wire["transient"], false);
        assert!(!wire.to_string().contains("httpStatus"));
    }

    #[test]
    fn verdict_rule_matches_the_retry_contract() {
        // Transport failures are transient.
        assert_eq!(
            ProviderError::transient("network", "offline").transient,
            Some(true)
        );
        // 429 and 5xx are transient and retain their status.
        for status in [429, 500, 502, 503, 504] {
            let error = ProviderError::http_failure(
                reqwest::StatusCode::from_u16(status).unwrap(),
                "server refused",
            );
            assert_eq!(error.transient, Some(true), "status {status}");
            assert_eq!(error.http_status, Some(status), "status {status}");
            assert_eq!(error.code, "unexpected_response");
        }
        // Every other status is deterministic but keeps the status.
        for status in [400, 401, 403, 404, 409, 422] {
            let error = ProviderError::http_failure(
                reqwest::StatusCode::from_u16(status).unwrap(),
                "refused",
            );
            assert_eq!(error.transient, Some(false), "status {status}");
            assert_eq!(error.http_status, Some(status), "status {status}");
        }
        // Local deterministic failures (auth, schema, init) are never
        // retried.
        assert_eq!(
            ProviderError::new("auth_expired", "session expired").transient,
            Some(false)
        );
        assert_eq!(
            ProviderError::new("unexpected_response", "schema changed").transient,
            Some(false)
        );
        assert_eq!(
            ProviderError::new("not_logged_in", "no login").transient,
            Some(false)
        );
    }

    #[test]
    fn with_transient_overrides_the_verdict() {
        // Envelope-style refusals classify by payload code: 429/5xx envelope
        // codes are transient, anything else is not.
        let base = ProviderError::new("unexpected_response", "envelope refused");
        assert_eq!(base.clone().with_transient(true).transient, Some(true));
        assert_eq!(base.clone().with_transient(false).transient, Some(false));
        // The override leaves code, message and status untouched.
        let overridden = base.with_transient(true);
        assert_eq!(overridden.code, "unexpected_response");
        assert_eq!(overridden.message, "envelope refused");
        assert_eq!(overridden.http_status, None);
    }

    // Delta-seconds Retry-After parses into milliseconds.
    #[test]
    fn retry_after_delta_seconds_parse() {
        assert_eq!(
            retry_after_header_ms("120", fixed_now()),
            Some(120_000)
        );
        assert_eq!(retry_after_header_ms(" 30 ", fixed_now()), Some(30_000));
        assert_eq!(retry_after_header_ms("0", fixed_now()), Some(0));
    }

    // HTTP-date Retry-After parses into the wait until that date.
    #[test]
    fn retry_after_http_date_parses_relative_to_now() {
        let header = "Tue, 29 Sep 2026 12:01:00 GMT";
        assert_eq!(retry_after_header_ms(header, fixed_now()), Some(60_000));
        // A past date means the wait is over already.
        let past = "Mon, 28 Sep 2026 12:00:00 GMT";
        assert_eq!(retry_after_header_ms(past, fixed_now()), Some(0));
    }

    // Malformed Retry-After is ignored safely: no hint, no crash.
    #[test]
    fn malformed_retry_after_is_ignored() {
        assert_eq!(retry_after_header_ms("", fixed_now()), None);
        assert_eq!(retry_after_header_ms("   ", fixed_now()), None);
        assert_eq!(retry_after_header_ms("soon", fixed_now()), None);
        assert_eq!(retry_after_header_ms("5.5", fixed_now()), None);
        assert_eq!(retry_after_header_ms("-30", fixed_now()), None);
        // Beyond u64 seconds cannot be honored and is treated as malformed.
        assert_eq!(
            retry_after_header_ms("99999999999999999999999", fixed_now()),
            None
        );
        // A date-shaped value that is not an HTTP-date is not guessed at.
        assert_eq!(
            retry_after_header_ms("2026-09-29T13:00:00Z", fixed_now()),
            None
        );
    }

    // A valid but absurd hint returns its raw (huge) value; capping is the
    // cooldown layer's job, and it never trusts more than MAX_COOLDOWN_MS.
    #[test]
    fn absurd_hints_reach_the_cooldown_cap_uncapped_from_the_parser() {
        assert_eq!(
            retry_after_header_ms("999999999", fixed_now()),
            Some(999_999_999_000)
        );
        assert_eq!(MAX_COOLDOWN_MS, 24 * 60 * 60 * 1000);
    }

    // The header hint attaches on 429/5xx only, and only when parseable.
    #[test]
    fn http_failure_attaches_retry_after_on_honored_statuses_only() {
        let now = fixed_now();
        let limited = ProviderError::http_failure_with_retry_after(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
            Some("120"),
            now,
        );
        assert_eq!(limited.retry_after_ms, Some(120_000));
        let down = ProviderError::http_failure_with_retry_after(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "down",
            Some("30"),
            now,
        );
        assert_eq!(down.retry_after_ms, Some(30_000));
        // Malformed header on an honored status: no hint at all.
        let malformed = ProviderError::http_failure_with_retry_after(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
            Some("bogus"),
            now,
        );
        assert_eq!(malformed.retry_after_ms, None);
        // Non-honored statuses ignore even a well-formed header.
        let forbidden = ProviderError::http_failure_with_retry_after(
            reqwest::StatusCode::FORBIDDEN,
            "refused",
            Some("120"),
            now,
        );
        assert_eq!(forbidden.retry_after_ms, None);
    }

    // The wire stays at the documented fields unless a hint is actually
    // carried, and the hint serializes camelCase.
    #[test]
    fn retry_after_wire_field_appears_only_when_present() {
        let without: Value = serde_json::to_value(ProviderError::http_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
        ))
        .unwrap();
        assert!(without.get("retryAfterMs").is_none());
        let with: Value = serde_json::to_value(ProviderError::http_failure_with_retry_after(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
            Some("60"),
            fixed_now(),
        ))
        .unwrap();
        assert_eq!(with["retryAfterMs"], 60_000);
    }

    // ---- transport-timeout marker (bounded-retry rule input) ----

    // A real response-phase timeout: the server accepts and then never
    // answers, so the client's request budget expires. The verdict stays
    // transient (a later cycle may succeed) and the budget-burning stall is
    // marked for the runtime's retry rule. A tiny client budget keeps the
    // fixture deterministic; production runs the same reqwest semantics with
    // the 15 s budget (connect, TLS, response, and body stalls all surface
    // through `is_timeout()`).
    #[tokio::test]
    async fn transport_failure_marks_a_real_timeout_and_keeps_the_verdict_transient() {
        let stalled = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = stalled.local_addr().unwrap();
        let holder = thread::spawn(move || {
            // Accept and deliberately hold: no response until well past the
            // client's budget. Dropping the stream ends the fixture.
            let (_stream, _) = stalled.accept().unwrap();
            thread::sleep(std::time::Duration::from_millis(400));
        });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(120))
            .build()
            .unwrap();
        let error = client
            .get(format!("http://{addr}/usage"))
            .send()
            .await
            .expect_err("a stalled server must burn the request budget");
        let failure = transport_failure("network", format!("stalled: {error}"), &error);
        assert_eq!(failure.transient, Some(true), "a timeout stays retryable later");
        assert!(failure.transport_timeout, "the stall must be marked");
        holder.join().unwrap();
    }

    // A fast-failing transport failure that is NOT a timeout: a TLS
    // handshake against a plain TCP endpoint that closes immediately. The
    // error surfaces in milliseconds — no request budget expires — so it
    // stays unmarked and keeps the one bounded in-cycle retry. (A dead-port
    // connect is NOT a reliable fixture here: some environments silently
    // drop it, which legitimately burns the budget and reads as a timeout.)
    #[tokio::test]
    async fn transport_failure_leaves_fast_non_timeout_errors_unmarked() {
        let plain = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = plain.local_addr().unwrap();
        let holder = thread::spawn(move || {
            // Accept the TLS attempt and close at once: the handshake fails
            // fast, no request budget is consumed.
            let (stream, _) = plain.accept().unwrap();
            drop(stream);
        });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let error = client
            .get(format!("https://{addr}/usage"))
            .send()
            .await
            .expect_err("a TLS handshake against a plain socket must fail");
        holder.join().unwrap();
        let failure = transport_failure("network", format!("refused: {error}"), &error);
        assert_eq!(failure.transient, Some(true));
        assert!(
            !failure.transport_timeout,
            "a fast non-timeout transport error must not be marked: {error}"
        );
    }

    // The body-read phase stalls too: a server that announces more body than
    // it ever sends burns the client budget during `bytes()`. The same
    // classifier must mark it — every backend maps both its `send()` and its
    // body-read failures through [`transport_failure`].
    #[tokio::test]
    async fn body_read_stall_is_marked_as_a_transport_timeout() {
        let stalled = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = stalled.local_addr().unwrap();
        let holder = thread::spawn(move || {
            let (mut stream, _) = stalled.accept().unwrap();
            use std::io::{Read, Write};
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            // Announce more body than is ever sent; the read stalls until
            // the client's budget expires.
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 512\r\n\r\n{\"partial\"",
            );
            thread::sleep(std::time::Duration::from_millis(400));
        });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(120))
            .build()
            .unwrap();
        let response = client
            .get(format!("http://{addr}/usage"))
            .send()
            .await
            .expect("the headers arrive");
        let error = response
            .bytes()
            .await
            .expect_err("a truncated body must burn the request budget");
        let failure = transport_failure("network", format!("cut short: {error}"), &error);
        assert_eq!(failure.transient, Some(true));
        assert!(failure.transport_timeout, "the body-read stall must be marked");
        holder.join().unwrap();
    }

    // The marker is internal state only — the wire contract stays exactly
    // the documented fields.
    #[test]
    fn transport_timeout_marker_never_reaches_the_wire() {
        let marked = ProviderError::transient("network", "stalled").with_transport_timeout(true);
        assert_eq!(
            wire_keys(&marked),
            ["code", "message", "transient"]
        );
        // Constructors default to unmarked; the builder only changes the
        // marker.
        assert!(!ProviderError::new("auth_invalid", "rejected").transport_timeout);
        assert!(!ProviderError::transient("network", "offline").transport_timeout);
    }
}
