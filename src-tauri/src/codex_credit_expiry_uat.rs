//! Windows, opt-in libtest only. The live entrypoint never starts Tauri.
use super::*;
use std::io::Read;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::Instant;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
};

const MAX_LOCAL_BYTES: u64 = 64 * 1024;
type Category = &'static str;
type LoopbackRoute<'a> = (&'a reqwest::Client, &'a str, &'a str);

#[derive(Clone, Copy, Serialize)]
enum Slot {
    Primary,
    CoWork,
}

impl Slot {
    fn key(self) -> &'static str {
        match self {
            Self::Primary => crate::codex_accounts::PRIMARY_ACCOUNT_ID,
            Self::CoWork => crate::codex_accounts::SECONDARY_ACCOUNT_ID,
        }
    }

    fn peer(self) -> Self {
        match self {
            Self::Primary => Self::CoWork,
            Self::CoWork => Self::Primary,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    slot: Slot,
    usage_get_attempted: u8,
    detail_get_attempted: u8,
    status_category: Category,
    account_match: bool,
    expiry_available: bool,
    expiry_exactness: Category,
    latency_category: Category,
    file_metadata_changed: bool,
}

impl Report {
    fn new(slot: Slot) -> Self {
        Self {
            slot,
            usage_get_attempted: 0,
            detail_get_attempted: 0,
            status_category: "notRun",
            account_match: false,
            expiry_available: false,
            expiry_exactness: "notEvaluated",
            latency_category: "notMeasured",
            file_metadata_changed: false,
        }
    }

    fn fail(&mut self, status: Category) {
        self.status_category = status;
        self.expiry_available = false;
        self.expiry_exactness = status;
    }
}

pub(super) struct Audit<'a> {
    report: &'a mut Report,
    pub(super) fixture: Option<LoopbackRoute<'a>>,
    pub(super) usage_status: Option<reqwest::StatusCode>,
    pub(super) detail_status: Option<reqwest::StatusCode>,
    pub(super) detail_timeout: bool,
    detail_transport_error: bool,
    account_unbound: bool,
}

impl Audit<'_> {
    pub(super) fn usage_attempted(&mut self) {
        self.report.usage_get_attempted += 1;
    }

    pub(super) fn detail_attempted(&mut self) {
        self.report.detail_get_attempted += 1;
    }

    pub(super) fn bind_usage(&mut self, observed: Option<&str>, local: Option<&str>) -> bool {
        self.report.account_match = local.is_some() && bind_credit_account(observed, local);
        self.account_unbound = !self.report.account_match;
        self.report.account_match
    }

    pub(super) fn observe_detail_error(&mut self, error: Option<&reqwest::Error>) {
        if let Some(error) = error {
            self.detail_transport_error = true;
            self.detail_timeout |= error.is_timeout();
        }
    }

    pub(super) fn bind_details(&mut self, body: &[u8], local: Option<&str>) {
        if let Ok(value) = serde_json::from_slice::<Value>(body) {
            if value
                .get("account_id")
                .is_some_and(|id| id.as_str() != local)
            {
                self.report.account_match = false;
                self.account_unbound = true;
            }
        }
    }

    fn expiry(&self, usage: Result<CodexUsage, ProviderError>) -> Result<Category, Category> {
        if self.account_unbound {
            return Err("accountUnbound");
        }
        if let Some(status) = self.detail_status.filter(|status| !status.is_success()) {
            return Err(http_category(status));
        }
        if self.detail_timeout {
            return Err("timeout");
        }
        if self.detail_transport_error {
            return Err("transportError");
        }
        let usage = usage.map_err(|error| {
            if let Some(status) = self.usage_status.filter(|status| !status.is_success()) {
                http_category(status)
            } else if error.transport_timeout {
                "timeout"
            } else {
                match error.code.as_str() {
                    "auth_expired" => "localAuthExpired",
                    "network" => "transportError",
                    _ => "usageUnproven",
                }
            }
        })?;
        let credits = usage.reset_credits.ok_or("summaryUnavailable")?;
        if credits.banked_credits == 0 {
            Ok("noCredits")
        } else if credits.next_credit_expires_at.is_some() {
            Ok("exact")
        } else {
            Err("expiryUnproven")
        }
    }
}

fn http_category(status: reqwest::StatusCode) -> Category {
    match status.as_u16() {
        401 | 403 => "unauthorized",
        429 => "rateLimited",
        300..=399 => "redirect",
        400..=499 => "http4xx",
        _ => "httpOther",
    }
}

fn explicit_home(home: &Path) -> Option<PathBuf> {
    if !home.is_absolute() {
        return None;
    }
    resolve_codex_home(Some(home.to_str()?))
        .ok()?
        .canonicalize()
        .ok()
}

// Read only the production account-file mapping, never initialize the app
// store (which can supply defaults or persist data). Primary's configured
// empty field means the Windows profile default with no CODEX_HOME override.
fn mapped_home(rows: &[Value], slot: Slot) -> Option<PathBuf> {
    let mut matches = rows.iter().filter(|row| row["accountId"] == slot.key());
    let row = matches.next()?;
    if matches.next().is_some() || row["monitored"].as_bool() != Some(true) {
        return None;
    }
    let home = row["codexHome"].as_str()?;
    if home.is_empty() && matches!(slot, Slot::Primary) && std::env::var_os("CODEX_HOME").is_none()
    {
        return Some(std::env::home_dir()?.join(".codex"));
    }
    explicit_home(Path::new(home))
}

fn live_homes(slot: Slot) -> Option<(PathBuf, PathBuf)> {
    let supplied = PathBuf::from(std::env::var_os("LIMITSCOPE_UAT_CODEX_HOME")?);
    let config = PathBuf::from(std::env::var_os("LIMITSCOPE_UAT_ACCOUNTS_FILE")?);
    if !config.is_absolute()
        || config.file_name()?.to_str()? != crate::codex_accounts::CODEX_ACCOUNTS_FILE_NAME
    {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(config)
        .ok()?
        .take(MAX_LOCAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_LOCAL_BYTES {
        return None;
    }
    configured_pair(&serde_json::from_slice(&bytes).ok()?, slot, supplied)
}

fn configured_pair(envelope: &Value, slot: Slot, supplied: PathBuf) -> Option<(PathBuf, PathBuf)> {
    // The production store accepts fallback configurations. Live UAT
    // requires the versioned file to contain both explicit slot entries.
    if envelope.get("version")?.as_u64()? != 1 {
        return None;
    }
    let rows = envelope.get("accounts")?.as_array()?;
    if explicit_home(&supplied)? != explicit_home(&mapped_home(&rows, slot)?)? {
        return None;
    }
    Some((supplied, mapped_home(&rows, slot.peer())?))
}

fn metadata(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let info = fs::symlink_metadata(path).ok()?;
    if !info.is_file() || info.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return None;
    }
    Some((info.len(), info.modified().ok()?))
}

fn read_only_auth(file: &fs::File) -> Option<CodexAuth> {
    let mut raw = String::new();
    file.take(MAX_LOCAL_BYTES + 1)
        .read_to_string(&mut raw)
        .ok()?;
    if raw.len() as u64 > MAX_LOCAL_BYTES {
        return None;
    }
    let auth = parse_auth(&raw).ok()?;
    auth.identity.as_ref()?;
    auth.account_id.as_deref().and_then(account_hint)?;
    Some(auth)
}

async fn probe(
    home: &Path,
    peer: &Path,
    fixture: Option<LoopbackRoute<'_>>,
    report: &mut Report,
) -> Result<Category, Category> {
    let dir = explicit_home(home).ok_or("invalidLocalScope")?;
    let other = explicit_home(peer).ok_or("invalidLocalScope")?;
    if dir == other {
        return Err("sharedHome");
    }
    let path = dir.join("auth.json");
    let before = metadata(&path).ok_or("authUnavailable")?;
    // Read access and read sharing only. Block concurrent writers and
    // replacement while the selected auth file is open during the probe.
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&path)
        .map_err(|_| "authUnavailable")?;
    if file
        .metadata()
        .map_err(|_| "authUnavailable")?
        .file_attributes()
        & FILE_ATTRIBUTE_REPARSE_POINT.0
        != 0
    {
        return Err("authUnavailable");
    }
    let unchanged =
        || metadata(&path) == Some(before) && explicit_home(home).as_ref() == Some(&dir);
    let result = if unchanged() {
        if let Some(auth) = read_only_auth(&file) {
            let mut audit = Audit {
                report,
                fixture,
                usage_status: None,
                detail_status: None,
                detail_timeout: false,
                detail_transport_error: false,
                account_unbound: false,
            };
            let usage = fetch_codex_usage_in(&dir, &auth, Some(&mut audit)).await;
            audit.expiry(usage)
        } else {
            Err("authUnproven")
        }
    } else {
        Err("authStateChanged")
    };
    report.file_metadata_changed = !unchanged();
    if report.file_metadata_changed {
        report.account_match = false;
        return Err("authStateChanged");
    }
    result
}

fn run_redacted(
    report: &mut Report,
    run: impl FnOnce(&mut Report) -> Result<Category, Category>,
) -> bool {
    // Install this before resolving inputs. A panic payload or backtrace may
    // contain private values, so never forward either to the test harness.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let start = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(report)));
    std::panic::set_hook(previous_hook);
    if report.usage_get_attempted != 0 {
        report.latency_category = match start.elapsed().as_millis() {
            0..=999 => "under1s",
            1000..=4999 => "1to5s",
            _ => "5sOrMore",
        };
    }
    match result {
        Ok(Ok(reason)) => {
            report.status_category = "ok";
            report.expiry_exactness = reason;
            report.expiry_available = reason == "exact";
            true
        }
        Ok(Err(status)) => {
            report.fail(status);
            false
        }
        Err(_) => {
            report.account_match = false;
            report.fail("internalFailure");
            false
        }
    }
}

#[test]
#[ignore = "live production GETs; run this exact test alone with an explicit configured slot and home"]
fn live_credit_expiry_one_slot() -> Result<(), &'static str> {
    let slot = match std::env::var("LIMITSCOPE_UAT_SLOT").as_deref() {
        Ok("Primary") => Slot::Primary,
        Ok("CoWork") => Slot::CoWork,
        _ => return Err("explicit slot required"),
    };
    let mut report = Report::new(slot);
    let ok = run_redacted(&mut report, |report| {
        if std::env::var_os("SSLKEYLOGFILE").is_some() {
            return Err("unsafeTransportConfig");
        }
        let (home, peer) = live_homes(slot).ok_or("invalidConfiguredScope")?;
        tauri::async_runtime::block_on(probe(&home, &peer, None, report))
    });
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|_| "report unavailable")?
    );
    if ok {
        Ok(())
    } else {
        Err("bounded credit expiry probe failed")
    }
}

#[cfg(test)]
mod offline {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::net::{Ipv4Addr, TcpListener};

    const PRIMARY: &str = "fixture-primary-workspace";
    const COWORK: &str = "fixture-cowork-workspace";
    const DEADLINE: &str = "2090-02-01T00:00:00Z";

    struct Homes(PathBuf);

    impl Homes {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("limitscope-uat-{}", uuid::Uuid::new_v4())))
        }

        fn fixture(&self, account: &str, explicit_id: bool) -> (PathBuf, String) {
            let dir = self.0.join(account);
            fs::create_dir_all(&dir).unwrap();
            let claim = json!({"https://api.openai.com/auth": {
                "chatgpt_user_id": account, "chatgpt_account_id": account
            }});
            let token = format!("e30.{}.fixture", URL_SAFE_NO_PAD.encode(claim.to_string()));
            fs::write(dir.join("auth.json"), json!({"tokens": {
                "access_token": token, "id_token": token, "account_id": explicit_id.then_some(account)
            }}).to_string()).unwrap();
            (dir, token)
        }
    }

    impl Drop for Homes {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn usage(account: Option<&str>, count: u8) -> Value {
        json!({"account_id": account, "plan_type": "PRIVATE_PLAN",
            "rate_limit": {"primary_window": {"used_percent": 73.52}},
            "rate_limit_reset_credits": {"available_count": count}})
    }

    fn details(account: &str) -> Value {
        json!({"account_id": account, "credits": [{
            "id": "PRIVATE_CREDIT", "status": "available", "reset_type": "codex_rate_limits",
            "granted_at": "2001-01-01T00:00:00Z", "expires_at": DEADLINE
        }]})
    }

    // This server asserts actual request methods, order and headers. Every
    // connection is closed so a bad extra GET cannot go unnoticed.
    fn server(
        account: &str,
        token: &str,
        scripted: Vec<(u16, Value)>,
    ) -> ([String; 2], std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let paths = ["/wham/usage", "/wham/rate-limit-reset-credits"];
        let urls = paths.map(|path| format!("{root}{path}"));
        let expected_auth = format!("authorization: Bearer {token}");
        let expected_scope = format!("chatgpt-account-id: {account}");
        let server = std::thread::spawn(move || {
            for (path, (status, body)) in paths.into_iter().zip(scripted) {
                let start = Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                start.elapsed() < Duration::from_secs(3),
                                "missing fixture request"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => panic!("fixture accept failed"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut input = Vec::new();
                while !input.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    input.push(byte[0]);
                    assert!(input.len() < 8192);
                }
                let input = String::from_utf8(input).unwrap();
                assert!(input.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
                assert!(input.lines().any(|line| line == expected_auth));
                assert!(input.lines().any(|line| line == expected_scope));
                let body = body.to_string();
                let length = body.len() + usize::from(status == 0);
                let status = if status == 0 { 200 } else { status };
                write!(stream,
                    "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Length: {length}\r\nLocation: /wrong\r\n\r\n{body}"
                ).unwrap();
                if length != body.len() {
                    // Exercise the body deadline, including after 2xx headers.
                    std::thread::sleep(Duration::from_millis(if path == "/wham/usage" {
                        450
                    } else {
                        5200
                    }));
                }
            }
            assert!(
                matches!(
                    listener.accept(),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                ),
                "unexpected extra request"
            );
        });
        (urls, server)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn explicit_mapping_missing_cowork_and_unproven_auth_send_zero_gets() {
        let homes = Homes::new();
        let (primary, _) = homes.fixture(PRIMARY, true);
        let cowork = homes.0.join(COWORK);
        let envelope = json!({"version": 1, "accounts": [
            json!({"accountId": Slot::Primary.key(), "codexHome": primary, "monitored": true}),
            json!({"accountId": Slot::CoWork.key(), "codexHome": cowork, "monitored": true}),
        ]});
        assert!(configured_pair(&envelope, Slot::CoWork, cowork.clone()).is_none());
        assert!(configured_pair(&envelope, Slot::CoWork, primary.clone()).is_none());
        let mut report = Report::new(Slot::CoWork);
        let client = client();
        let guarded = Some((
            &client,
            "http://127.0.0.1:1/unused",
            "http://127.0.0.1:1/unused",
        ));
        assert_eq!(
            probe(&cowork, &primary, guarded, &mut report).await,
            Err("invalidLocalScope")
        );
        assert_eq!(
            probe(&primary.join("."), &primary, guarded, &mut report).await,
            Err("sharedHome")
        );
        homes.fixture(COWORK, false);
        assert!(configured_pair(&envelope, Slot::CoWork, primary.clone()).is_none());
        assert!(configured_pair(&envelope, Slot::CoWork, cowork.clone()).is_some());
        assert!(configured_pair(&envelope["accounts"], Slot::CoWork, cowork.clone()).is_none());
        let mut foreign = envelope.clone();
        foreign["version"] = json!(2);
        assert!(configured_pair(&foreign, Slot::CoWork, cowork.clone()).is_none());
        assert_eq!(
            probe(&cowork, &primary, guarded, &mut report).await,
            Err("authUnproven")
        );
        assert_eq!(
            (report.usage_get_attempted, report.detail_get_attempted),
            (0, 0)
        );
    }

    #[tokio::test]
    async fn both_routes_bind_their_headers_and_zero_skips_details_without_auth_writes() {
        let homes = Homes::new();
        let (primary, primary_token) = homes.fixture(PRIMARY, true);
        let (cowork, cowork_token) = homes.fixture(COWORK, true);
        let files = [primary.join("auth.json"), cowork.join("auth.json")];
        let before = files
            .each_ref()
            .map(|path| (fs::read(path).unwrap(), metadata(path)));
        for (slot, home, peer, account, token, script, result, detail_count) in [
            (
                Slot::Primary,
                &primary,
                &cowork,
                PRIMARY,
                &primary_token,
                vec![(200, usage(Some(PRIMARY), 1)), (200, details(PRIMARY))],
                "exact",
                1,
            ),
            (
                Slot::CoWork,
                &cowork,
                &primary,
                COWORK,
                &cowork_token,
                vec![(200, usage(Some(COWORK), 0))],
                "noCredits",
                0,
            ),
        ] {
            let (urls, server) = server(account, token, script);
            let mut report = Report::new(slot);
            assert_eq!(
                probe(
                    home,
                    peer,
                    Some((&client(), &urls[0], &urls[1])),
                    &mut report
                )
                .await,
                Ok(result)
            );
            server.join().unwrap();
            assert!(report.account_match);
            assert!(!report.file_metadata_changed);
            assert_eq!(
                (report.usage_get_attempted, report.detail_get_attempted),
                (1, detail_count)
            );
            assert!(run_redacted(&mut report, |_| Ok(result)));
            let wire = serde_json::to_string(&report).unwrap();
            for value in [
                account,
                token,
                DEADLINE,
                "PRIVATE_CREDIT",
                "PRIVATE_PLAN",
                "73.52",
                "127.0.0.1",
            ] {
                assert!(!wire.contains(value));
            }
        }
        assert_eq!(
            files
                .each_ref()
                .map(|path| (fs::read(path).unwrap(), metadata(path))),
            before
        );
    }

    #[tokio::test]
    async fn both_requests_fail_closed_on_auth_rate_limit_timeout_and_binding() {
        let homes = Homes::new();
        let (primary, token) = homes.fixture(PRIMARY, true);
        let (cowork, _) = homes.fixture(COWORK, true);
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_millis(250))
            .build()
            .unwrap();
        for (script, reason, matched) in [
            (vec![(200, usage(Some(COWORK), 1))], "accountUnbound", false),
            (vec![(200, usage(None, 1))], "accountUnbound", false),
            (
                vec![(200, usage(Some(PRIMARY), 1)), (200, details(COWORK))],
                "accountUnbound",
                false,
            ),
            (vec![(401, details(PRIMARY))], "unauthorized", false),
            (vec![(429, details(PRIMARY))], "rateLimited", false),
            (vec![(302, details(PRIMARY))], "redirect", false),
            (vec![(0, usage(Some(PRIMARY), 1))], "timeout", false),
            (
                vec![(200, usage(Some(PRIMARY), 1)), (401, details(PRIMARY))],
                "unauthorized",
                true,
            ),
            (
                vec![(200, usage(Some(PRIMARY), 1)), (429, details(PRIMARY))],
                "rateLimited",
                true,
            ),
            (
                vec![(200, usage(Some(PRIMARY), 1)), (0, details(PRIMARY))],
                "timeout",
                true,
            ),
        ] {
            let detail_count = (script.len() - 1) as u8;
            let (urls, server) = server(PRIMARY, &token, script);
            let mut report = Report::new(Slot::Primary);
            let result = probe(
                &primary,
                &cowork,
                Some((&client, &urls[0], &urls[1])),
                &mut report,
            )
            .await;
            server.join().unwrap();
            assert_eq!(result, Err(reason));
            assert!(!run_redacted(&mut report, |_| result));
            assert_eq!(report.account_match, matched);
            assert!(!report.expiry_available);
            assert_eq!(
                (report.usage_get_attempted, report.detail_get_attempted),
                (1, detail_count)
            );
        }
    }

    #[test]
    fn panic_reports_only_allowlisted_fields_and_preserves_attempts() {
        let mut report = Report::new(Slot::CoWork);
        assert!(!run_redacted(&mut report, |report| {
            report.usage_get_attempted = 1;
            report.account_match = true;
            panic!("PRIVATE PANIC PAYLOAD");
        }));
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            json!({
                "slot": "CoWork", "usageGetAttempted": 1, "detailGetAttempted": 0,
                "statusCategory": "internalFailure", "accountMatch": false, "expiryAvailable": false,
                "expiryExactness": "internalFailure", "latencyCategory": "under1s", "fileMetadataChanged": false
            })
        );
    }
}
