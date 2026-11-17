//! Offline fixtures only. Never resolve ambient CODEX_HOME or real credentials.
use super::*;
use serde_json::json;

const ACCOUNT: &str = "fixture-workspace-primary";
const OTHER: &str = "fixture-workspace-cowork";

fn now() -> DateTime<Utc> {
    "2026-11-17T12:00:00Z".parse().unwrap()
}
fn row(id: &str, expires: Value) -> Value {
    json!({"id": id, "status": "available", "reset_type": "codex_rate_limits",
        "granted_at": "2026-11-01T00:00:00Z", "expires_at": expires})
}
fn details(rows: Vec<Value>, count: u32) -> Vec<u8> {
    serde_json::to_vec(&json!({"available_count": count, "credits": rows})).unwrap()
}
fn derive(value: Value, count: u32) -> Option<String> {
    next_credit_expiry(&serde_json::to_vec(&value).unwrap(), count, ACCOUNT, now())
}
fn valid_details() -> Vec<u8> {
    details(
        vec![row("fixture-credit-a", json!("2090-01-02T00:00:00Z"))],
        1,
    )
}

#[test]
fn complete_list_proves_nearest_across_rfc3339_offsets_and_unix_seconds() {
    let early: DateTime<Utc> = "2026-12-01T00:00:00Z".parse().unwrap();
    let rows = vec![
        row("a", json!("2026-12-02T00:00:00Z")),
        row("b", json!(early.timestamp())),
        row("c", json!("2026-12-01T02:00:00+01:00")),
    ];
    assert_eq!(
        next_credit_expiry(&details(rows, 3), 3, ACCOUNT, now()).as_deref(),
        Some("2026-12-01T00:00:00Z")
    );
    // Usage's count can establish completeness even without a details count.
    assert!(derive(
        json!({"credits": [row("a", json!("2026-12-01T00:00:00Z"))]}),
        1
    )
    .is_some());
}

#[test]
fn incomplete_capped_duplicate_or_oversized_lists_are_unknown() {
    let one = row("a", json!("2026-12-01T00:00:00Z"));
    for payload in [
        json!({"available_count": 2, "credits": [one.clone()]}),
        json!({"available_count": 1, "credits": [one.clone()]}),
        json!({"credits": [one.clone(), one.clone()]}),
        json!({"credits": []}),
        json!({"credits": null}),
        json!({}),
        json!([]),
        json!("schema drift"),
    ] {
        assert_eq!(derive(payload, 2), None);
    }
    let many: Vec<_> = (0..=MAX_CREDIT_ROWS)
        .map(|i| row(&format!("{i}"), json!("2026-12-01T00:00:00Z")))
        .collect();
    assert_eq!(
        next_credit_expiry(&details(many, 257), 257, ACCOUNT, now()),
        None
    );
    assert_eq!(
        next_credit_expiry(&vec![b' '; MAX_CREDIT_DETAILS_BYTES + 1], 1, ACCOUNT, now()),
        None
    );
    assert_eq!(
        next_credit_expiry(&valid_details(), 0, ACCOUNT, now()),
        None
    );
}

#[test]
fn schema_counts_account_and_unknown_rows_fail_closed() {
    let valid = row("a", json!("2026-12-01T00:00:00Z"));
    for count in [
        json!(0),
        json!(2),
        json!("1"),
        json!(1.0),
        json!(-1),
        json!(null),
        json!(true),
    ] {
        assert_eq!(
            derive(
                json!({"available_count": count, "credits": [valid.clone()]}),
                1
            ),
            None
        );
    }
    for (key, value) in [
        ("status", json!("expired")),
        ("status", json!(null)),
        ("reset_type", json!("unknown")),
        ("reset_type", json!(null)),
        ("id", json!(null)),
        ("id", json!("")),
    ] {
        let mut bad = valid.clone();
        bad[key] = value;
        assert_eq!(derive(json!({"credits": [bad]}), 1), None);
    }
    for key in ["status", "reset_type", "id", "granted_at", "expires_at"] {
        let mut bad = valid.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert_eq!(derive(json!({"credits": [bad]}), 1), None);
    }
    assert_eq!(
        derive(json!({"account_id": OTHER, "credits": [valid]}), 1),
        None
    );
    assert_eq!(next_credit_expiry(b"not json", 1, ACCOUNT, now()), None);
}

#[test]
fn unknown_null_expired_before_grant_and_invalid_times_never_claim_nearest() {
    for expires in [
        json!(null),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(true),
        json!("1794916800"),
        json!("not a date"),
        json!("2026-12-01"),
        json!("2026-12-01T00:00:00"),
        json!("2026-11-17T12:00:00Z"),
        json!("2026-11-17T11:59:59Z"),
        json!("2026-02-30T12:00:00Z"),
        json!(253_402_300_800_i64),
    ] {
        let rows = vec![row("a", json!("2026-12-01T00:00:00Z")), row("b", expires)];
        assert_eq!(
            next_credit_expiry(&details(rows, 2), 2, ACCOUNT, now()),
            None
        );
    }
    let mut before_grant = row("a", json!("2026-12-01T00:00:00Z"));
    before_grant["granted_at"] = json!("2026-12-02T00:00:00Z");
    assert_eq!(derive(json!({"credits": [before_grant]}), 1), None);
    for grant in [
        json!(null),
        json!("bad"),
        json!("2026-11-18T00:00:00Z"),
        json!(0),
    ] {
        let mut bad = row("a", json!("2026-12-01T00:00:00Z"));
        bad["granted_at"] = grant;
        assert_eq!(derive(json!({"credits": [bad]}), 1), None);
    }
}

#[test]
fn only_available_rows_contribute_but_unknown_status_or_type_invalidates_proof() {
    let available = row("a", json!("2026-12-01T00:00:00Z"));
    let mut redeemed = row("b", json!("2026-11-01T00:00:00Z"));
    redeemed["status"] = json!("redeemed");
    let mut redeeming = redeemed.clone();
    redeeming["id"] = json!("c");
    redeeming["status"] = json!("redeeming");
    assert!(derive(
        json!({"credits": [available.clone(), redeemed.clone(), redeeming]}),
        1
    )
    .is_some());
    assert_eq!(derive(json!({"credits": [redeemed.clone()]}), 1), None);
    redeemed["reset_type"] = json!("unknown");
    assert_eq!(derive(json!({"credits": [available, redeemed]}), 1), None);
}

fn auth(account: &str) -> CodexAuth {
    CodexAuth {
        access_token: "synthetic-not-a-real-token".into(),
        account_id: Some(account.into()),
        identity: codex_identity("fixture-user", account),
    }
}
fn usage(auth: &CodexAuth, count: u32) -> CodexUsage {
    let body = json!({"account_id": auth.account_id, "rate_limit": {"primary_window": {"used_percent": 25}},
        "rate_limit_reset_credits": {"available_count": count, "applicable_available_count": 0}});
    parse_usage_response_with_identity(
        &serde_json::to_vec(&body).unwrap(),
        Utc::now(),
        auth.account_id.as_deref(),
        auth.identity.clone(),
        #[cfg(windows)]
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn same_request_enrichment_preserves_counts_stamp_quota_and_redacts_wire() {
    let auth = auth(ACCOUNT);
    let mut usage = usage(&auth, 1);
    let before = usage.reset_credits.clone().unwrap();
    enrich_credit_expiry(
        &mut usage,
        &auth,
        || async { Some(valid_details()) },
        || auth.identity.clone(),
    )
    .await;
    let credits = usage.reset_credits.as_ref().unwrap();
    assert_eq!(
        credits.next_credit_expires_at.as_deref(),
        Some("2090-01-02T00:00:00Z")
    );
    assert_eq!(
        (
            credits.banked_credits,
            credits.currently_applicable,
            &credits.checked_at
        ),
        (
            before.banked_credits,
            before.currently_applicable,
            &before.checked_at
        )
    );
    assert_eq!(usage.limits[0].used_percent, 25.0);
    let wire = serde_json::to_string(&usage).unwrap();
    assert!(wire.contains("\"nextCreditExpiresAt\":\"2090-01-02T00:00:00Z\""));
    for secret in [
        ACCOUNT,
        "fixture-user",
        "fixture-credit-a",
        "synthetic-not-a-real-token",
        "granted_at",
        "description",
        "title",
    ] {
        assert!(!wire.contains(secret));
    }
    // Subsequent count-only success never copies/restamps yesterday's expiry.
    let next = self::usage(&auth, 1);
    assert_eq!(
        next.reset_credits.as_ref().unwrap().next_credit_expires_at,
        None
    );
    assert!(!serde_json::to_string(&next)
        .unwrap()
        .contains("nextCreditExpiresAt"));
}

#[tokio::test]
async fn optional_failure_and_identity_swap_cannot_attach_expiry() {
    let auth = auth(ACCOUNT);
    for identity in [None, self::auth(OTHER).identity] {
        let mut usage = usage(&auth, 1);
        enrich_credit_expiry(
            &mut usage,
            &auth,
            || async { Some(valid_details()) },
            || identity,
        )
        .await;
        assert_eq!(usage.reset_credits.unwrap().next_credit_expires_at, None);
    }
    for body in [
        None,
        Some(b"schema drift".to_vec()),
        Some(details(vec![], 1)),
    ] {
        let mut usage = usage(&auth, 1);
        let before = usage.reset_credits.clone();
        enrich_credit_expiry(
            &mut usage,
            &auth,
            || async { body },
            || auth.identity.clone(),
        )
        .await;
        assert_eq!(usage.reset_credits, before);
        assert_eq!(usage.limits[0].used_percent, 25.0);
    }
}

#[tokio::test]
async fn missing_explicit_local_id_unknown_identity_zero_or_stale_skips_details() {
    for case in 0..6 {
        let mut auth = auth(ACCOUNT);
        let mut usage = usage(&auth, 1);
        match case {
            0 => auth.account_id = None,
            1 => auth.identity = None,
            2 => usage.reset_credits.as_mut().unwrap().banked_credits = 0,
            3 => {
                usage.reset_credits.as_mut().unwrap().checked_at =
                    (Utc::now() - TimeDelta::seconds(901)).to_rfc3339()
            }
            4 => usage.account.as_mut().unwrap().identity = self::auth(OTHER).identity,
            _ => usage.reset_credits = None,
        }
        enrich_credit_expiry(
            &mut usage,
            &auth,
            || async { panic!("must not request details") },
            || panic!("must not read identity"),
        )
        .await;
        assert!(usage
            .reset_credits
            .is_none_or(|c| c.next_credit_expires_at.is_none()));
    }
}

#[tokio::test]
async fn configured_primary_and_cowork_homes_are_isolated_and_swap_fails_closed() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../artifacts")
        .join(format!("expiry-fixtures-{}", uuid::Uuid::new_v4()));
    let primary = root.join("primary");
    let cowork = root.join("cowork");
    fn write_fixture(dir: &Path, account: &str) {
        fs::create_dir_all(dir).unwrap();
        let claim = json!({"https://api.openai.com/auth": {"chatgpt_user_id": "fixture-user", "chatgpt_account_id": account}});
        let jwt = format!(
            "e30.{}.fixture",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claim).unwrap())
        );
        fs::write(
            dir.join("auth.json"),
            json!({"tokens": {"account_id": account, "access_token": jwt, "id_token": jwt}})
                .to_string(),
        )
        .unwrap();
    }
    write_fixture(&primary, ACCOUNT);
    write_fixture(&cowork, OTHER);
    let a = load_auth(&resolve_codex_home(primary.to_str()).unwrap()).unwrap();
    let b = load_auth(&resolve_codex_home(cowork.to_str()).unwrap()).unwrap();
    let mut first = usage(&a, 1);
    let mut second = usage(&b, 2);
    enrich_credit_expiry(
        &mut first,
        &a,
        || async { Some(valid_details()) },
        || current_codex_identity(primary.to_str()),
    )
    .await;
    enrich_credit_expiry(
        &mut second,
        &b,
        || async {
            Some(details(
                vec![
                    row("b", json!("2090-03-01T00:00:00Z")),
                    row("c", json!("2090-02-01T00:00:00Z")),
                ],
                2,
            ))
        },
        || current_codex_identity(cowork.to_str()),
    )
    .await;
    assert_eq!(
        first
            .reset_credits
            .unwrap()
            .next_credit_expires_at
            .as_deref(),
        Some("2090-01-02T00:00:00Z")
    );
    assert_eq!(
        second
            .reset_credits
            .unwrap()
            .next_credit_expires_at
            .as_deref(),
        Some("2090-02-01T00:00:00Z")
    );
    let mut swapped = usage(&b, 1);
    enrich_credit_expiry(
        &mut swapped,
        &b,
        || async {
            write_fixture(&cowork, ACCOUNT);
            Some(valid_details())
        },
        || current_codex_identity(cowork.to_str()),
    )
    .await;
    assert_eq!(swapped.reset_credits.unwrap().next_credit_expires_at, None);
    fs::remove_dir_all(root).unwrap();
}

// A loopback fixture exercises the actual bounded HTTP reader. All headers and
// body values are synthetic; the production endpoint is never contacted.
fn server(response: Vec<u8>, hold_body: bool) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/details", listener.local_addr().unwrap());
    let task = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(6)))
            .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            if socket.read(&mut byte).unwrap_or(0) == 0 {
                return;
            }
            headers.push(byte[0]);
            assert!(headers.len() < 8192);
        }
        let request = String::from_utf8(headers).unwrap().to_lowercase();
        assert!(request.starts_with("get /details http/1.1"));
        assert!(request.contains("authorization: bearer synthetic-not-a-real-token\r\n"));
        assert!(request.contains(&format!("chatgpt-account-id: {ACCOUNT}\r\n")));
        let _ = socket.write_all(&response);
        if hold_body {
            std::thread::sleep(Duration::from_millis(5200));
        }
    });
    (url, task)
}

#[tokio::test]
async fn actual_http_reader_limits_body_rejects_errors_redirects_and_schema_drift() {
    let auth = auth(ACCOUNT);
    let client = http_client().unwrap();
    let built = credit_details_request(client, &auth, RESET_CREDITS_URL)
        .build()
        .unwrap();
    assert_eq!(built.method(), reqwest::Method::GET);
    assert_eq!(built.url().as_str(), RESET_CREDITS_URL);
    assert_eq!(built.timeout(), Some(&CREDIT_DETAILS_TIMEOUT));
    let mut responses = vec![
        b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/forbidden\r\nContent-Length: 0\r\n\r\n".to_vec(),
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_CREDIT_DETAILS_BYTES + 1).into_bytes(),
    ];
    let oversized = vec![b' '; MAX_CREDIT_DETAILS_BYTES + 1];
    let mut chunked = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
        oversized.len()
    )
    .into_bytes();
    chunked.extend(oversized);
    chunked.extend(b"\r\n0\r\n\r\n");
    responses.push(chunked);
    for response in responses {
        let (url, server) = server(response, false);
        assert!(read_credit_details(
            credit_details_request(client, &auth, &url),
            #[cfg(windows)]
            None
        )
        .await
        .is_none());
        server.join().unwrap();
    }
    for body in [valid_details(), b"{\"credits\":null}".to_vec()] {
        let mut response =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        response.extend(&body);
        let (url, server) = server(response, false);
        let read = read_credit_details(
            credit_details_request(client, &auth, &url),
            #[cfg(windows)]
            None,
        )
        .await
        .unwrap();
        assert_eq!(read, body);
        server.join().unwrap();
    }
}

#[tokio::test]
async fn http_body_timeout_keeps_usage_and_counts_successful() {
    let auth = auth(ACCOUNT);
    let mut usage = usage(&auth, 1);
    let (url, server) = server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n".to_vec(),
        true,
    );
    let before = usage.reset_credits.clone();
    enrich_credit_expiry(
        &mut usage,
        &auth,
        || {
            read_credit_details(
                credit_details_request(http_client().unwrap(), &auth, &url),
                #[cfg(windows)]
                None,
            )
        },
        || auth.identity.clone(),
    )
    .await;
    assert_eq!(usage.reset_credits, before);
    assert_eq!(usage.limits[0].used_percent, 25.0);
    server.join().unwrap();
}
