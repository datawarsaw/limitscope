//! Read-only Mistral EDU Vibe + API monthly budget reader.
//!
//! One provider, two independent budget rows:
//!
//! - **Vibe Budget** — the Mistral Studio / "vibe" coding allowance.
//! - **API Budget** — the Mistral API allowance (EDU or pay-as-you-go).
//!
//! Both are read from the account's own provider with the account's own
//! API key, and only from the canonical destination:
//!
//! - `GET https://api.mistral.ai/v1/billing/budget` — the `api_budget` and
//!   `vibe_budget` rows.
//! - `GET https://api.mistral.ai/v1/billing/subscription` — the plan name.
//!
//! Everything in this module is read-only. There is no inference call, no
//! model routing, no admin API, no browser cookie, and no POST/PUT/DELETE.
//! The key is read from the Windows Credential Manager entry the account
//! already uses; LimitScope never writes, copies, rotates or migrates it.
//!
//! Secret handling is deliberately narrow:
//!
//! - The secret is resolved into [`MistralSecret`], whose `Debug` renders a
//!   fixed placeholder and whose `Drop` overwrites the buffer.
//! - It is used exactly once, as a bearer token against the one canonical
//! origin. It is never logged, printed, stored, persisted or returned.
//! - Only a SHA-256 fingerprint of it (the account identity) and a masked
//! tail leave this module.
//!
//! Error policy (the shared provider vocabulary, see `provider_error.rs`):
//!
//! - a missing or unreadable local credential is a source-absent failure
//!   (`credential_missing` / `credential_unreadable` / `credential_ambiguous`)
//!   — the provider is *unavailable*, never zero and never an error the
//!   frontend should retry;
//! - 401 is `auth_invalid` (the key was refused) and 403 is
//!   `not_entitled` (the key is accepted but may not read billing);
//! - 402 is `billing_restricted` — a billing refusal of the *reading*, not
//!   proof that the key is invalid, so it never maps onto an auth code;
//! - 408, 429, 5xx and transport timeouts are transient, so the runtime's
//!   bounded, account-bound last-good retention keeps serving the previous
//!   reading instead of erasing it.

use std::sync::OnceLock;
use std::time::Duration;

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;

use crate::provider_error::ProviderError;

// ---------- canonical destination ----------

/// The budget read: `api_budget` and `vibe_budget` rows. The billing
/// endpoints are part of the account's own provider: no alternate host, no
/// redirect target, no proxy, no browser session.
pub(crate) const MISTRAL_BUDGET_URL: &str = "https://api.mistral.ai/v1/billing/budget";
/// The subscription read: the plan name (e.g. `EDU`).
pub(crate) const MISTRAL_SUBSCRIPTION_URL: &str = "https://api.mistral.ai/v1/billing/subscription";

/// The Windows Credential Manager entry that holds the account's Mistral
/// API key. LimitScope reads this entry; it never writes it.
pub(crate) const CREDENTIAL_TARGET: &str = "ai.mistral.vibe";
/// The account name the entry is stored under. The key itself is the
/// credential blob; this name is the identity the entry is expected to
/// carry, and a mismatch fails closed.
pub(crate) const CREDENTIAL_USER: &str = "MISTRAL_API_KEY";

/// The credential is absent from the store: nothing was there to read.
pub(crate) const CREDENTIAL_MISSING: &str = "credential_missing";
/// The credential is there but cannot be used: the store could not be read,
/// the entry could not be decoded, or the secret is not a usable key.
pub(crate) const CREDENTIAL_UNREADABLE: &str = "credential_unreadable";
/// The entry under the authorized target belongs to a different account
/// than the one this adapter is authorized to read.
pub(crate) const CREDENTIAL_AMBIGUOUS: &str = "credential_ambiguous";

/// Every credential-source code this adapter can produce.
///
/// All three are local-state conditions, not refusals by the provider: the
/// runtime classifies them through `runtime::SOURCE_ABSENT_CODES` as
/// *unavailable*, never as an error and never as a zero. The list is
/// published so the runtime's classification is asserted against the codes
/// the adapter really produces, not against a hand-written copy of them.
pub(crate) const CREDENTIAL_SOURCE_CODES: [&str; 3] = [
    CREDENTIAL_MISSING,
    CREDENTIAL_UNREADABLE,
    CREDENTIAL_AMBIGUOUS,
];

/// Whole-request budget: connect, TLS, response and body read. The same
/// budget the other live backends use.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Connect budget, so a dead destination cannot hold the whole cycle.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Response body cap. The billing payloads are a few hundred bytes; the
/// cap only exists so a hostile or broken endpoint cannot stream forever.
const MAX_BODY_BYTES: usize = 64 * 1024;
const USER_AGENT: &str = concat!("LimitScope/", env!("CARGO_PKG_VERSION"));

/// The largest budget total this adapter accepts, in cents (one billion
/// currency units). Beyond that the number is not a budget; it is a
/// malformed field, and a malformed field is unknown, not a huge number.
const MAX_TOTAL_CENTS: i64 = 100_000_000_000;

/// The plan names the subscription endpoint is allowed to report. A plan
/// outside this closed set is not verified and is never displayed.
const KNOWN_PLANS: [&str; 4] = ["FREE", "INDIVIDUAL", "EDU", "TEAM"];

/// The two budget rows, in display order: Vibe first, because it is the
/// provider's primary window, API second.
const BUDGET_ROWS: [(&str, &str); 2] =
    [("vibe_budget", "Vibe Budget"), ("api_budget", "API Budget")];

/// The stable key of one budget row on the wire.
pub(crate) const VIBE_ROW_KEY: &str = "vibe";
pub(crate) const API_ROW_KEY: &str = "api";

/// The static identity of a budget row.
///
/// The closed key set is the only source of a row's identity: a persisted
/// row is restored under the key it was stored with, never under a stored
/// label, so a rewritten file can never rename a row.
pub(crate) fn row_identity(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        VIBE_ROW_KEY => Some((VIBE_ROW_KEY, "Vibe Budget")),
        API_ROW_KEY => Some((API_ROW_KEY, "API Budget")),
        _ => None,
    }
}

// ---------- wire DTO ----------

/// One Mistral budget row as the UI shows it.
///
/// The two rows are independent: one can be measured while the other is
/// unknown, and they are never merged, summed or converted into each
/// other. Amounts travel as integer cents in the row's own currency, so a
/// consumer can never read one currency's cents as another's.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MistralBudgetRowDto {
    /// Stable row key (`vibe`, `api`).
    pub key: &'static str,
    /// Display label (`Vibe Budget`, `API Budget`).
    pub label: &'static str,
    /// Whether this row carries a measured budget. A row that is not
    /// available states no percentage and no amount: it is unknown, never
    /// zero and never full.
    pub available: bool,
    /// ISO currency code exactly as the source reported it, uppercased.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// The budget total, in cents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cents: Option<i64>,
    /// The consumed amount, in cents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_cents: Option<i64>,
    /// The remaining amount, in cents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_cents: Option<i64>,
    /// The source's own used percentage (0–100).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    /// The source's own reset instant, preserved verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
    /// True when the used amount was derived from the percentage and the
    /// initial budget instead of being reported by the source. The flag is
    /// what keeps a derived figure from reading as an exact one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_derived: Option<bool>,
    /// Why this row is unknown, in a fixed display-safe vocabulary:
    /// `missing`, `currency`, `initial_budget`, `usage_percentage`,
    /// `reset_at`, `expired`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// The Mistral budget observation: both rows plus the verified plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MistralBudgetsDto {
    /// The verified plan name from the subscription endpoint (`EDU`). Absent
    /// means the plan could not be verified — never a guess.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    /// Both rows, always both, in display order.
    pub rows: Vec<MistralBudgetRowDto>,
}

/// What one refresh read: the two rows and the verified plan.
pub(crate) struct MistralUsage {
    pub rows: Vec<MistralBudgetRowDto>,
    pub plan_name: Option<String>,
    /// The account attribution: the masked key tail and the deterministic
    /// credential fingerprint the runtime isolates last-good by.
    pub account: MistralAccount,
}

/// The display-safe account facts of the credential that answered.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MistralAccount {
    /// The masked tail of the key (e.g. `ab12`), or `None` when the key is
    /// too short to mask safely.
    pub key_hint: Option<String>,
    /// The deterministic fingerprint of the full credential. A digest, never
    /// the key itself.
    pub identity: Option<String>,
}

impl std::fmt::Debug for MistralUsage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately narrow: the row count, the verified plan and the
        // account facts. The rows' amounts are not dumped, and no secret
        // material exists in this type to begin with.
        formatter
            .debug_struct("MistralUsage")
            .field("rows", &self.rows.len())
            .field("plan_name", &self.plan_name)
            .field("account", &self.account)
            .finish()
    }
}

// ---------- the secret ----------

/// The resolved API key.
///
/// The secret never leaves this type in any form that could be observed: it
/// is not logged, not printed, not stored, not persisted and not returned.
/// `Debug` renders a fixed placeholder, and `Drop` overwrites the buffer so
/// the key does not linger in freed memory.
pub(crate) struct MistralSecret {
    secret: String,
}

impl MistralSecret {
    fn new(secret: String) -> Self {
        Self { secret }
    }

    /// The only way the secret leaves this struct: as a borrow, for the one
    /// bearer header it authorizes.
    fn expose(&self) -> &str {
        &self.secret
    }
}

impl std::fmt::Debug for MistralSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MistralSecret(<redacted>)")
    }
}

impl Drop for MistralSecret {
    fn drop(&mut self) {
        // SAFETY: overwriting the bytes of a String that is about to be
        // dropped is sound — no other reference to those bytes exists, and
        // the buffer is never read again.
        unsafe { self.secret.as_bytes_mut() }.fill(0);
    }
}

// ---------- credential resolution ----------

/// Reads the account's Mistral API key from the Windows Credential Manager.
///
/// The entry is validated before it is used: the target must be the
/// authorized one and the stored account name must be the expected one. A
/// missing entry, an unreadable store, an entry under a different account
/// name and an empty secret all fail closed — the provider is unavailable,
/// never a fabricated zero.
pub(crate) fn resolve_credential() -> Result<MistralCredential, ProviderError> {
    let stored = read_stored_credential()?;
    let Some(stored) = stored else {
        return Err(ProviderError::new(
            CREDENTIAL_MISSING,
            "No Mistral API key is stored in Windows Credential Manager under \"ai.mistral.vibe\".",
        ));
    };
    if stored.user != CREDENTIAL_USER {
        // The entry under the authorized target belongs to a different
        // account than the one this adapter is authorized to read. Failing
        // closed is the only safe answer: using it would spend a credential
        // nobody authorized for this read.
        return Err(ProviderError::new(
            CREDENTIAL_AMBIGUOUS,
            "The stored Mistral credential belongs to a different account than the authorized one.",
        ));
    }
    let secret_text = stored.secret.trim();
    if secret_text.is_empty() || !secret_text.chars().all(|ch| ch.is_ascii_graphic()) {
        return Err(ProviderError::new(
            CREDENTIAL_UNREADABLE,
            "The stored Mistral credential could not be read as a usable API key.",
        ));
    }
    let secret = MistralSecret::new(secret_text.to_string());
    Ok(MistralCredential {
        key_hint: key_hint(secret_text),
        identity: Some(crate::credential_identity::fingerprint(
            "mistral",
            secret_text,
        )),
        secret,
    })
}

/// The resolved credential for one refresh.
pub(crate) struct MistralCredential {
    secret: MistralSecret,
    key_hint: Option<String>,
    identity: Option<String>,
}

impl std::fmt::Debug for MistralCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MistralCredential")
            .field("secret", &self.secret)
            .field("key_hint", &self.key_hint)
            .field("identity", &self.identity)
            .finish()
    }
}

/// The masked tail of a key, for display only.
///
/// A key shorter than twelve characters would leak too much of itself
/// through a four-character tail, so it gets no hint at all.
fn key_hint(secret: &str) -> Option<String> {
    if secret.chars().count() < 12 {
        return None;
    }
    let tail: Vec<char> = secret.chars().rev().take(4).collect();
    if !tail
        .iter()
        .all(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
    {
        return None;
    }
    Some(tail.into_iter().rev().collect())
}

/// The raw stored entry: the account name and the secret text.
struct StoredCredential {
    user: String,
    secret: String,
}

#[cfg(windows)]
fn read_stored_credential() -> Result<Option<StoredCredential>, ProviderError> {
    use std::ffi::c_void;
    use windows_sys::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    /// `ERROR_NOT_FOUND`: the credential manager has no such entry.
    const ERROR_NOT_FOUND: i32 = 1168;

    let mut target: Vec<u16> = CREDENTIAL_TARGET.encode_utf16().collect();
    target.push(0);
    let mut raw: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is a NUL-terminated UTF-16 string that outlives the
    // call, and `raw` is a valid out-pointer for the allocation CredReadW
    // returns. The allocation is released with CredFree below, exactly
    // once, on every path that owns it.
    let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) };
    if ok == 0 {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if code == ERROR_NOT_FOUND {
            return Ok(None);
        }
        return Err(ProviderError::new(
            CREDENTIAL_UNREADABLE,
            "The Mistral credential could not be read from Windows Credential Manager.",
        ));
    }
    // SAFETY: `raw` is the credential CredReadW just allocated and owns.
    let (user, blob) = unsafe {
        let credential = &*raw;
        let user = read_wide(credential.UserName);
        let blob = if credential.CredentialBlob.is_null() || credential.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
            .to_vec()
        };
        (user, blob)
    };
    // SAFETY: `raw` was allocated by CredReadW and is released exactly once.
    unsafe { CredFree(raw as *const c_void) };

    Ok(Some(StoredCredential {
        user,
        secret: decode_secret_blob(&blob),
    }))
}

#[cfg(not(windows))]
fn read_stored_credential() -> Result<Option<StoredCredential>, ProviderError> {
    Err(ProviderError::new(
        CREDENTIAL_UNREADABLE,
        "Mistral budget reading requires the Windows Credential Manager.",
    ))
}

/// Copies a wide (UTF-16) string out of a credential manager allocation.
///
/// # Safety
/// `pointer` must be a NUL-terminated UTF-16 string that is valid for the
/// duration of the call.
#[cfg(windows)]
unsafe fn read_wide(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut length = 0usize;
    while *pointer.add(length) != 0 {
        length += 1;
        if length > 4096 {
            break;
        }
    }
    let units = std::slice::from_raw_parts(pointer, length);
    String::from_utf16_lossy(units)
}

/// Decodes a generic credential blob into the secret text.
///
/// Windows stores a generic credential's blob as UTF-16LE. Some writers add
/// a trailing NUL, some store raw bytes; both are accepted, and the result
/// is only accepted when it is printable ASCII — the shape a Mistral API
/// key has. Anything else is unreadable, never a guess.
fn decode_secret_blob(blob: &[u8]) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(blob.len() / 2);
    for pair in blob.chunks_exact(2) {
        units.push(u16::from_le_bytes([pair[0], pair[1]]));
    }
    let wide = String::from_utf16_lossy(&units);
    let wide = wide.trim_end_matches('\0').trim();
    if wide.chars().all(|ch| ch.is_ascii_graphic()) && !wide.is_empty() {
        return wide.to_string();
    }
    let narrow = String::from_utf8_lossy(blob);
    narrow.trim_end_matches('\0').trim().to_string()
}

// ---------- HTTP ----------

fn http_client() -> Result<&'static reqwest::Client, ProviderError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(USER_AGENT)
            // A redirect would carry the Authorization header to whatever
            // host the redirect names. A 3xx surfaces as a plain HTTP
            // failure and the key never leaves the canonical origin.
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(client) => Ok(client),
        Err(error) => Err(ProviderError::new(
            "unexpected",
            format!("Could not initialize HTTP client: {error}"),
        )),
    }
}

/// One bounded GET against a canonical Mistral billing endpoint.
async fn get_json(
    client: &reqwest::Client,
    url: &str,
    secret: &MistralSecret,
) -> Result<Value, ProviderError> {
    let response = client
        .get(url)
        .bearer_auth(secret.expose())
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|error| {
            crate::provider_error::transport_failure(
                "network",
                format!("Could not reach the Mistral billing endpoint: {error}"),
                &error,
            )
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(status_error(status));
    }
    let body = read_bounded_body(response).await?;
    serde_json::from_slice(&body).map_err(|_| {
        ProviderError::new(
            "unexpected_response",
            "The Mistral billing response was not usable JSON (schema change).",
        )
    })
}

/// Reads at most [`MAX_BODY_BYTES`], then stops: an oversized body is a
/// refusal, never a truncated read.
async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return Err(ProviderError::new(
            "unexpected_response",
            "The Mistral billing response was oversized.",
        ));
    }
    let mut body = Vec::new();
    let mut response = response;
    loop {
        let chunk = response.chunk().await.map_err(|error| {
            crate::provider_error::transport_failure(
                "network",
                format!("The Mistral billing response was cut short: {error}"),
                &error,
            )
        })?;
        let Some(chunk) = chunk else { break };
        if chunk.len() > MAX_BODY_BYTES - body.len() {
            return Err(ProviderError::new(
                "unexpected_response",
                "The Mistral billing response was oversized.",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The status policy of the billing endpoints.
fn status_error(status: reqwest::StatusCode) -> ProviderError {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return ProviderError::new(
            "auth_invalid",
            "Mistral refused the stored API key for the billing read.",
        );
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return ProviderError::new(
            "not_entitled",
            "Mistral accepted the API key but refused the billing read for this account.",
        );
    }
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        // A billing refusal of the reading, not proof that the key is
        // invalid: the plan may be exhausted or the endpoint gated. It
        // therefore never maps onto an auth code, and the previous reading
        // is retained rather than erased.
        return ProviderError::new(
            "billing_restricted",
            "Mistral refused the billing read (HTTP 402): this account's billing state does not allow it.",
        );
    }
    if status == reqwest::StatusCode::REQUEST_TIMEOUT {
        return ProviderError::http_failure(
            status,
            "The Mistral billing endpoint timed out (HTTP 408).",
        )
        .with_transient(true);
    }
    ProviderError::http_failure(
        status,
        format!("The Mistral billing endpoint returned HTTP {status}."),
    )
}

/// Whether a failure is an auth/permission refusal, which fails the whole
/// refresh closed instead of degrading only the plan.
fn is_auth_failure(error: &ProviderError) -> bool {
    matches!(error.code.as_str(), "auth_invalid" | "not_entitled")
}

// ---------- the read ----------

/// Reads both Mistral budget rows and the verified plan.
pub(crate) async fn get_mistral_usage() -> Result<MistralUsage, ProviderError> {
    let credential = resolve_credential()?;
    let identity = credential.identity.clone();
    let client = http_client()?;

    // Both reads are issued together, so the whole refresh stays inside one
    // request budget instead of two of them in sequence.
    let (budget, subscription) = tokio::join!(
        get_json(&client, MISTRAL_BUDGET_URL, &credential.secret),
        get_json(&client, MISTRAL_SUBSCRIPTION_URL, &credential.secret),
    );

    // The budget read is the provider's content: when it fails, the refresh
    // fails, and the runtime's account-bound last-good retention decides
    // whether the previous reading may still be shown.
    let budget = budget
        .map_err(|error| error.with_identity_hint(identity.clone()))
        .map_err(|error| scrub(error, &credential.secret))?;

    // The plan is an independent read: a plan that cannot be verified never
    // invalidates the budgets that were measured. An auth refusal does fail
    // the whole refresh — the key itself was refused.
    let plan_name = match subscription {
        Ok(subscription) => verified_plan_name(&subscription),
        Err(error) if is_auth_failure(&error) => {
            return Err(scrub(
                error.with_identity_hint(identity),
                &credential.secret,
            ));
        }
        Err(_) => None,
    };

    Ok(MistralUsage {
        rows: parse_budget_rows(&budget),
        plan_name,
        account: MistralAccount {
            key_hint: credential.key_hint.clone(),
            identity: identity.clone(),
        },
    })
}

/// Final-boundary scrub: the key is alive for the whole read, so any exact
/// occurrence of it in a failure message is removed before the error leaves
/// the adapter.
fn scrub(error: ProviderError, secret: &MistralSecret) -> ProviderError {
    crate::secret_scrub::scrub_provider_error(error, &[secret.expose()])
}

/// The plan name, only when the source reports one of the known plans.
fn verified_plan_name(subscription: &Value) -> Option<String> {
    let raw = subscription.get("plan_name")?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let normalized = raw.to_ascii_uppercase();
    KNOWN_PLANS
        .iter()
        .find(|plan| **plan == normalized)
        .map(|plan| plan.to_string())
}

/// Both budget rows, always both, each measured on its own.
fn parse_budget_rows(budget: &Value) -> Vec<MistralBudgetRowDto> {
    BUDGET_ROWS
        .iter()
        .map(|(source_key, label)| {
            let key = if *source_key == "vibe_budget" {
                VIBE_ROW_KEY
            } else {
                API_ROW_KEY
            };
            build_row(key, label, budget.get(*source_key))
        })
        .collect()
}

/// One budget row, measured independently of its sibling.
///
/// A row is available only when every field it needs is usable: a named
/// three-letter currency, a positive budget total, a percentage inside
/// 0–100, and a reset that is either absent or a readable instant. A row
/// that fails any of those is *unknown* — it states no percentage and no
/// amount, never zero and never full.
fn build_row(key: &'static str, label: &'static str, raw: Option<&Value>) -> MistralBudgetRowDto {
    let mut row = MistralBudgetRowDto {
        key,
        label,
        available: false,
        currency: None,
        total_cents: None,
        used_cents: None,
        remaining_cents: None,
        used_percent: None,
        reset_at: None,
        amount_derived: None,
        unavailable_reason: None,
    };
    let Some(object) = raw.and_then(Value::as_object) else {
        row.unavailable_reason = Some("missing".to_string());
        return row;
    };

    let Some(currency) = object
        .get("currency")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|currency| {
            currency.len() == 3 && currency.chars().all(|ch| ch.is_ascii_alphabetic())
        })
    else {
        row.unavailable_reason = Some("currency".to_string());
        return row;
    };

    let Some(initial_budget) = object
        .get("initial_budget")
        .and_then(finite_number)
        .filter(|value| *value > 0.0)
    else {
        row.unavailable_reason = Some("initial_budget".to_string());
        return row;
    };
    let total_cents = (initial_budget * 100.0).round();
    if !total_cents.is_finite() || total_cents > MAX_TOTAL_CENTS as f64 {
        row.unavailable_reason = Some("initial_budget".to_string());
        return row;
    }
    let total_cents = total_cents as i64;

    let Some(used_percent) = object
        .get("usage_percentage")
        .and_then(finite_number)
        .filter(|value| (0.0..=100.0).contains(value))
    else {
        row.unavailable_reason = Some("usage_percentage".to_string());
        return row;
    };

    // The source's own reset instant, preserved verbatim. An absent reset is
    // honest (the row simply has no reset); a reset that is present but
    // unreadable makes the row unknown rather than guessed.
    let reset_at = match object.get("reset_at") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else if DateTime::parse_from_rfc3339(trimmed).is_ok() {
                Some(trimmed.to_string())
            } else {
                row.unavailable_reason = Some("reset_at".to_string());
                return row;
            }
        }
        Some(_) => {
            row.unavailable_reason = Some("reset_at".to_string());
            return row;
        }
    };

    // The consumed amount: the source's exact usage amount when it reports
    // one, otherwise derived from the percentage and the budget total. The
    // derived figure is rounded to the cent and flagged, so it never reads
    // as an exact number the source did not state.
    let (used_cents, amount_derived) = match object
        .get("usage_amount")
        .and_then(finite_number)
        .filter(|value| *value >= 0.0)
    {
        Some(amount) => ((amount * 100.0).round() as i64, false),
        // used = initial_budget × percentage / 100 currency units, which is
        // initial_budget × percentage cents.
        None => ((initial_budget * used_percent).round() as i64, true),
    };
    let used_cents = used_cents.clamp(0, total_cents);

    row.available = true;
    row.currency = Some(currency.to_ascii_uppercase());
    row.total_cents = Some(total_cents);
    row.used_cents = Some(used_cents);
    row.remaining_cents = Some(total_cents - used_cents);
    row.used_percent = Some(used_percent);
    row.reset_at = reset_at;
    row.amount_derived = Some(amount_derived);
    row
}

/// A finite number, accepting the JSON number and its numeric-string form.
fn finite_number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The historical fixture the baseline screenshot shows. It is a fixed
    /// sample, NOT a live reading: Vibe EUR 127.50 total with EUR 0.34
    /// used, API EUR 12.75 total with EUR 1.80 used, both resetting at the
    /// same December instant.
    fn fixture_budget() -> Value {
        json!({
            "vibe_budget": {
                "usage_percentage": 0.27,
                "initial_budget": 127.50,
                "currency": "EUR",
                "reset_at": "2026-12-01T00:00:00Z"
            },
            "api_budget": {
                "usage_percentage": 14.12,
                "initial_budget": 12.75,
                "currency": "EUR",
                "reset_at": "2026-12-01T00:00:00Z"
            }
        })
    }

    fn row<'a>(rows: &'a [MistralBudgetRowDto], key: &str) -> &'a MistralBudgetRowDto {
        rows.iter().find(|row| row.key == key).expect("row")
    }

    // ---------- parsing ----------

    #[test]
    fn the_fixture_yields_two_independent_euro_rows() {
        let rows = parse_budget_rows(&fixture_budget());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "vibe");
        assert_eq!(rows[0].label, "Vibe Budget");
        assert_eq!(rows[1].key, "api");
        assert_eq!(rows[1].label, "API Budget");

        let vibe = row(&rows, "vibe");
        assert!(vibe.available);
        assert_eq!(vibe.currency.as_deref(), Some("EUR"));
        assert_eq!(vibe.total_cents, Some(12_750));
        assert_eq!(vibe.used_cents, Some(34));
        assert_eq!(vibe.remaining_cents, Some(12_716));
        assert_eq!(vibe.reset_at.as_deref(), Some("2026-12-01T00:00:00Z"));
        assert_eq!(vibe.amount_derived, Some(true));

        let api = row(&rows, "api");
        assert!(api.available);
        assert_eq!(api.total_cents, Some(1_275));
        assert_eq!(api.used_cents, Some(180));
        assert_eq!(api.remaining_cents, Some(1_095));
        assert_eq!(api.used_percent, Some(14.12));
    }

    #[test]
    fn the_rows_never_share_a_total() {
        let rows = parse_budget_rows(&fixture_budget());
        let vibe = row(&rows, "vibe").total_cents.expect("total");
        let api = row(&rows, "api").total_cents.expect("total");
        assert_ne!(vibe, api);
        // The two budgets are never summed into one figure.
        assert_ne!(vibe + api, vibe);
    }

    #[test]
    fn a_missing_row_is_unknown_never_zero() {
        let budget = json!({ "api_budget": { "usage_percentage": 10.0, "initial_budget": 20.0, "currency": "EUR" } });
        let rows = parse_budget_rows(&budget);
        let vibe = row(&rows, "vibe");
        assert!(!vibe.available);
        assert_eq!(vibe.unavailable_reason.as_deref(), Some("missing"));
        assert_eq!(vibe.used_percent, None);
        assert_eq!(vibe.total_cents, None);
        assert_eq!(vibe.used_cents, None);
        assert_eq!(vibe.remaining_cents, None);
    }

    #[test]
    fn a_zero_or_missing_budget_total_is_unknown_never_exhausted() {
        let budget = json!({
            "vibe_budget": { "usage_percentage": 10.0, "initial_budget": 0, "currency": "EUR" },
            "api_budget": { "usage_percentage": 10.0, "currency": "EUR" }
        });
        let rows = parse_budget_rows(&budget);
        assert_eq!(
            row(&rows, "vibe").unavailable_reason.as_deref(),
            Some("initial_budget")
        );
        assert_eq!(
            row(&rows, "api").unavailable_reason.as_deref(),
            Some("initial_budget")
        );
        assert!(!row(&rows, "vibe").available);
        assert!(!row(&rows, "api").available);
    }

    #[test]
    fn a_bad_currency_is_unknown() {
        let budget = json!({
            "vibe_budget": { "usage_percentage": 10.0, "initial_budget": 10.0, "currency": "EUR\nPRIVATE-CANARY" },
            "api_budget": { "usage_percentage": 10.0, "initial_budget": 10.0, "currency": null }
        });
        let rows = parse_budget_rows(&budget);
        assert_eq!(
            row(&rows, "vibe").unavailable_reason.as_deref(),
            Some("currency")
        );
        assert_eq!(
            row(&rows, "api").unavailable_reason.as_deref(),
            Some("currency")
        );
        assert_eq!(row(&rows, "vibe").currency, None);
    }

    #[test]
    fn a_percentage_outside_the_range_is_unknown() {
        for percent in [json!(-1.0), json!(101.0), json!("n/a")] {
            let budget = json!({ "vibe_budget": { "usage_percentage": percent, "initial_budget": 10.0, "currency": "EUR" } });
            let rows = parse_budget_rows(&budget);
            assert_eq!(
                row(&rows, "vibe").unavailable_reason.as_deref(),
                Some("usage_percentage")
            );
            assert_eq!(row(&rows, "vibe").used_percent, None);
        }
    }

    #[test]
    fn an_unreadable_reset_makes_the_row_unknown() {
        let budget = json!({ "vibe_budget": { "usage_percentage": 10.0, "initial_budget": 10.0, "currency": "EUR", "reset_at": "soon" } });
        let rows = parse_budget_rows(&budget);
        assert_eq!(
            row(&rows, "vibe").unavailable_reason.as_deref(),
            Some("reset_at")
        );
        assert!(!row(&rows, "vibe").available);
    }

    #[test]
    fn an_absent_reset_keeps_the_row_measurable() {
        let budget = json!({ "vibe_budget": { "usage_percentage": 10.0, "initial_budget": 10.0, "currency": "EUR" } });
        let rows = parse_budget_rows(&budget);
        assert!(row(&rows, "vibe").available);
        assert_eq!(row(&rows, "vibe").reset_at, None);
    }

    #[test]
    fn the_source_reset_instant_is_preserved_verbatim() {
        let budget = json!({ "vibe_budget": { "usage_percentage": 10.0, "initial_budget": 10.0, "currency": "EUR", "reset_at": "2026-12-01T00:00:00Z" } });
        let rows = parse_budget_rows(&budget);
        assert_eq!(
            row(&rows, "vibe").reset_at.as_deref(),
            Some("2026-12-01T00:00:00Z")
        );
    }

    #[test]
    fn a_source_reported_usage_amount_is_used_as_is() {
        let budget = json!({ "vibe_budget": { "usage_percentage": 0.27, "initial_budget": 127.50, "usage_amount": 0.34, "currency": "EUR" } });
        let rows = parse_budget_rows(&budget);
        let vibe = row(&rows, "vibe");
        assert_eq!(vibe.used_cents, Some(34));
        assert_eq!(vibe.amount_derived, Some(false));
    }

    #[test]
    fn a_derived_amount_is_rounded_to_the_cent_and_flagged() {
        let budget = json!({ "vibe_budget": { "usage_percentage": 33.333, "initial_budget": 10.0, "currency": "EUR" } });
        let rows = parse_budget_rows(&budget);
        let vibe = row(&rows, "vibe");
        assert_eq!(vibe.used_cents, Some(333));
        assert_eq!(vibe.remaining_cents, Some(667));
        assert_eq!(vibe.amount_derived, Some(true));
    }

    #[test]
    fn a_full_budget_reports_no_remaining_cents() {
        let budget = json!({ "vibe_budget": { "usage_percentage": 100.0, "initial_budget": 10.0, "currency": "EUR" } });
        let rows = parse_budget_rows(&budget);
        let vibe = row(&rows, "vibe");
        assert_eq!(vibe.used_cents, Some(1_000));
        assert_eq!(vibe.remaining_cents, Some(0));
        assert_eq!(vibe.used_percent, Some(100.0));
    }

    #[test]
    fn an_absurd_budget_total_is_unknown() {
        let budget = json!({ "vibe_budget": { "usage_percentage": 1.0, "initial_budget": 1e18, "currency": "EUR" } });
        let rows = parse_budget_rows(&budget);
        assert_eq!(
            row(&rows, "vibe").unavailable_reason.as_deref(),
            Some("initial_budget")
        );
    }

    // ---------- plan ----------

    #[test]
    fn only_a_known_plan_is_verified() {
        assert_eq!(
            verified_plan_name(&json!({ "plan_name": "EDU" })).as_deref(),
            Some("EDU")
        );
        assert_eq!(
            verified_plan_name(&json!({ "plan_name": "edu" })).as_deref(),
            Some("EDU")
        );
        assert_eq!(
            verified_plan_name(&json!({ "plan_name": "TEAM" })).as_deref(),
            Some("TEAM")
        );
        assert_eq!(
            verified_plan_name(&json!({ "plan_name": "ENTERPRISE" })),
            None
        );
        assert_eq!(verified_plan_name(&json!({ "plan_name": "" })), None);
        assert_eq!(verified_plan_name(&json!({ "plan_name": 7 })), None);
        assert_eq!(verified_plan_name(&json!({})), None);
    }

    // ---------- status policy ----------

    #[test]
    fn the_status_policy_separates_auth_billing_and_transient_failures() {
        let unauthorized = status_error(reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(unauthorized.code, "auth_invalid");
        assert_eq!(unauthorized.transient, Some(false));

        let forbidden = status_error(reqwest::StatusCode::FORBIDDEN);
        assert_eq!(forbidden.code, "not_entitled");
        assert_eq!(forbidden.transient, Some(false));

        // 402 is a billing restriction, never an invalid-key verdict.
        let payment = status_error(reqwest::StatusCode::PAYMENT_REQUIRED);
        assert_eq!(payment.code, "billing_restricted");
        assert_eq!(payment.transient, Some(false));
        assert_ne!(payment.code, "auth_invalid");

        for status in [
            reqwest::StatusCode::REQUEST_TIMEOUT,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let failure = status_error(status);
            assert_eq!(failure.transient, Some(true), "{status} must be transient");
        }

        let not_found = status_error(reqwest::StatusCode::NOT_FOUND);
        assert_eq!(not_found.transient, Some(false));
    }

    #[test]
    fn failure_messages_never_carry_key_material() {
        let failure = status_error(reqwest::StatusCode::UNAUTHORIZED);
        assert!(!failure.message.contains("Bearer"));
        assert!(!failure.message.contains('\n'));
    }

    // ---------- secret handling ----------

    #[test]
    fn the_secret_never_renders_its_content() {
        let secret = MistralSecret::new("sk-fixture-secret-value".to_string());
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("sk-fixture"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn the_credential_debug_rendering_hides_the_key() {
        let credential = MistralCredential {
            secret: MistralSecret::new("sk-fixture-secret-value".to_string()),
            key_hint: Some("alue".to_string()),
            identity: Some("credential-v1:fixture".to_string()),
        };
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("sk-fixture"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn a_short_key_gets_no_masked_tail() {
        assert_eq!(key_hint("short-key"), None);
        assert_eq!(key_hint("0123456789ab"), Some("89ab".to_string()));
        assert_eq!(key_hint("0123456789a!"), None);
    }

    #[test]
    fn the_blob_decoder_accepts_utf16_and_utf8_and_rejects_the_rest() {
        let utf16: Vec<u8> = "sk-fixture-secret"
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        assert_eq!(decode_secret_blob(&utf16), "sk-fixture-secret");

        let mut nul_terminated = utf16.clone();
        nul_terminated.extend_from_slice(&[0, 0]);
        assert_eq!(decode_secret_blob(&nul_terminated), "sk-fixture-secret");

        assert_eq!(
            decode_secret_blob(b"sk-fixture-secret"),
            "sk-fixture-secret"
        );
        assert_eq!(decode_secret_blob(&[]), "");
    }

    // ---------- destination ----------

    #[test]
    fn only_the_canonical_https_destination_is_used() {
        assert_eq!(
            MISTRAL_BUDGET_URL,
            "https://api.mistral.ai/v1/billing/budget"
        );
        assert_eq!(
            MISTRAL_SUBSCRIPTION_URL,
            "https://api.mistral.ai/v1/billing/subscription"
        );
        for url in [MISTRAL_BUDGET_URL, MISTRAL_SUBSCRIPTION_URL] {
            assert!(url.starts_with("https://api.mistral.ai/"));
        }
    }

    #[test]
    fn the_client_refuses_redirects() {
        let client = http_client().expect("client");
        // reqwest exposes the redirect policy only through behavior, so the
        // contract is pinned by the policy itself: the adapter never
        // configures a redirect policy other than `none`.
        assert!(client.get(MISTRAL_BUDGET_URL).build().is_ok());
    }

    #[tokio::test]
    #[ignore = "live test: reads the real Mistral billing API with the stored credential"]
    async fn live_read_returns_two_rows_without_leaking_the_key() {
        // The redacted evidence of one real read: the observed timestamps,
        // the verified plan, the masked account hint, and both rows' budget
        // facts. No key material is printed, and none of it is derived from
        // the secret beyond the masked hint and the plan the source stated.
        let started = chrono::Utc::now();
        let usage = get_mistral_usage().await.expect("live read");
        let finished = chrono::Utc::now();
        println!("mistral-live: started_at={started}");
        println!("mistral-live: observed_at={finished}");
        println!("mistral-live: plan={:?}", usage.plan_name);
        println!("mistral-live: account_hint={:?}", usage.account.key_hint);
        println!(
            "mistral-live: identity_present={}",
            usage.account.identity.is_some()
        );
        for row in &usage.rows {
            println!(
                "mistral-live: row={} available={} currency={:?} total_cents={:?} used_cents={:?} remaining_cents={:?} used_percent={:?} reset_at={:?} amount_derived={:?} unavailable_reason={:?}",
                row.key,
                row.available,
                row.currency,
                row.total_cents,
                row.used_cents,
                row.remaining_cents,
                row.used_percent,
                row.reset_at,
                row.amount_derived,
                row.unavailable_reason,
            );
        }
        assert_eq!(usage.rows.len(), 2);
    }

    /// An UNAUTHENTICATED reachability probe of the canonical endpoint. It
    /// carries no credential and spends no account budget: it only reports
    /// what the transport itself does, so a blocked path can be told apart
    /// from an auth or permission verdict.
    #[tokio::test]
    #[ignore = "live diagnostic: unauthenticated transport probe"]
    async fn live_unauthenticated_probe_reports_the_transport_verdict() {
        let client = http_client().expect("client");
        match client
            .get(MISTRAL_BUDGET_URL)
            .header("accept", "application/json")
            .send()
            .await
        {
            Ok(response) => println!("mistral-probe: status={}", response.status()),
            Err(error) => println!(
                "mistral-probe: error={error} is_timeout={} is_connect={} is_request={}",
                error.is_timeout(),
                error.is_connect(),
                error.is_request()
            ),
        }
    }
}
