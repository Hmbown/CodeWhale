use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use clap::Parser;
use codewhale_secrets::account::{
    ACCOUNT_SESSION_SCHEMA_VERSION, AccountSession as AuthSession,
    account_auth_slot as cloud_auth_slot,
};
use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};
use serde_json::json;

use super::*;
use crate::{Cli, Commands};

struct FakeTransport {
    responses: Mutex<VecDeque<CloudResponse>>,
    requests: Mutex<Vec<CloudRequest>>,
}

impl FakeTransport {
    fn new(responses: Vec<CloudResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> std::sync::MutexGuard<'_, Vec<CloudRequest>> {
        self.requests.lock().unwrap()
    }
}

impl CloudTransport for FakeTransport {
    fn execute(&self, request: CloudRequest) -> Result<CloudResponse> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow!("fake transport exhausted"))
    }
}

fn response(status: u16, body: serde_json::Value) -> CloudResponse {
    CloudResponse {
        status,
        body: serde_json::to_vec(&body).unwrap(),
        retry_after: None,
    }
}

fn response_retry_after(status: u16, body: serde_json::Value, seconds: u64) -> CloudResponse {
    CloudResponse {
        status,
        body: serde_json::to_vec(&body).unwrap(),
        retry_after: Some(seconds),
    }
}

fn account(id: &str) -> serde_json::Value {
    json!({
        "user": {
            "id": id,
            "displayName": "Hunter",
            "email": "hunter@example.test",
            "plan": "free",
            "modelKeys": {}
        }
    })
}

fn auth(access: &str, refresh: &str, account_id: &str) -> AuthBundle {
    AuthBundle {
        token_type: "Bearer".to_string(),
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        session: Some(AuthSession {
            id: "session-1".to_string(),
            provider: "github".to_string(),
            expires_at: String::new(),
            refresh_expires_at: String::new(),
            ..AuthSession::default()
        }),
        user: Some(CloudUser {
            id: account_id.to_string(),
            display_name: "Hunter".to_string(),
            email: "hunter@example.test".to_string(),
            ..CloudUser::default()
        }),
    }
}

fn auth_json(access: &str, refresh: &str, account_id: &str) -> serde_json::Value {
    serde_json::to_value(auth(access, refresh, account_id)).unwrap()
}

fn test_secrets() -> (Secrets, Arc<InMemoryKeyringStore>) {
    let store = Arc::new(InMemoryKeyringStore::new());
    (Secrets::new(store.clone()), store)
}

fn test_config() -> (tempfile::TempDir, ConfigStore) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let config = ConfigStore::load(Some(path)).unwrap();
    (temp, config)
}

fn command(argv: &[&str]) -> CloudCommand {
    let cli = Cli::try_parse_from(argv).unwrap();
    let Some(Commands::Account(args)) = cli.command else {
        panic!("expected account command");
    };
    args.command
}

#[test]
fn parses_cloud_command_matrix_and_rejects_inline_keys() {
    assert!(matches!(
        command(&["codewhale", "account", "status"]),
        CloudCommand::Status
    ));
    assert!(matches!(
        command(&["codewhale", "cloud", "login", "--no-open"]),
        CloudCommand::Login(CloudLoginArgs { no_open: true, .. })
    ));
    assert!(matches!(
        command(&[
            "codewhale",
            "cloud",
            "keys",
            "set",
            "xiaomi-mimo",
            "--from-local"
        ]),
        CloudCommand::Keys(CloudKeysArgs {
            command: CloudKeysCommand::Set(CloudKeySetArgs {
                provider: CloudProvider::Xiaomi,
                from_local: true,
                ..
            })
        })
    ));
    assert!(
        Cli::try_parse_from([
            "codewhale",
            "cloud",
            "keys",
            "set",
            "openai",
            "sk-unsafe-inline"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "codewhale",
            "cloud",
            "keys",
            "set",
            "openai",
            "--from-local",
            "--api-key-stdin"
        ])
        .is_err()
    );
    assert!(reject_inline_api_key(None).is_ok());
    let error = reject_inline_api_key(Some("sk-never-render")).unwrap_err();
    assert!(error.to_string().contains("--api-key-stdin"));
    assert!(!error.to_string().contains("sk-never-render"));
}

#[test]
fn api_base_requires_https_or_literal_loopback_http() {
    assert_eq!(
        validate_api_base("https://api.codewhale.net/")
            .unwrap()
            .display,
        "https://api.codewhale.net"
    );
    assert!(validate_api_base("http://127.0.0.1:8787").is_ok());
    assert!(validate_api_base("http://[::1]:8787").is_ok());
    assert!(validate_api_base("http://api.codewhale.net").is_err());
    assert!(validate_api_base("https://user:secret@example.test").is_err());
    assert!(validate_api_base("https://example.test/prefix").is_err());
}

#[test]
fn verification_urls_are_pinned_to_the_app_or_loopback() {
    const CODE: &str = "ABCD-EFGH-JKLM";
    const API: &str = "https://api.codewhale.net";
    assert!(
        validate_verification_url("https://app.codewhale.net/cli/authorize", API, CODE, false,)
            .is_ok()
    );
    assert!(
        validate_verification_url(
            "https://app.codewhale.net/cli/authorize?user_code=ABCD-EFGH-JKLM",
            API,
            CODE,
            true,
        )
        .is_ok()
    );
    for unsafe_url in [
        "https://attacker.example/cli/authorize",
        "https://user@app.codewhale.net/cli/authorize",
        "https://app.codewhale.net/cli/authorize#continue",
        "https://app.codewhale.net/cli/authorize/extra",
        "https://app.codewhale.net/cli/other/../authorize",
        "https://app.codewhale.net/cli/%61uthorize",
        "https://app.codewhale.net/cli/authorize?next=https%3A%2F%2Fattacker.example",
        "https://app.codewhale.net/cli/authorize?user_code=ABCD-EFGH-JKLM&next=evil",
    ] {
        assert!(
            validate_verification_url(unsafe_url, API, CODE, unsafe_url.contains("user_code"))
                .is_err(),
            "accepted unsafe URL: {unsafe_url}"
        );
    }
    assert!(
        validate_verification_url(
            "http://localhost:3000/cli/authorize?user_code=ABCD-EFGH-JKLM",
            "http://127.0.0.1:8787",
            CODE,
            true,
        )
        .is_ok()
    );
    assert!(
        validate_verification_url(
            "https://staging-app.example/cli/authorize",
            "https://staging-api.example",
            CODE,
            false,
        )
        .is_err()
    );
}

#[test]
fn user_codes_and_key_inputs_match_the_server_contract() {
    assert!(validate_user_code("ABCD-EFGH-JKLM").is_ok());
    for invalid in [
        "CW-1234",
        "ABCI-EFGH-JKLM",
        "ABCO-EFGH-JKLM",
        "ABC1-EFGH-JKLM",
        "abcd-EFGH-JKLM",
        "ABCD_EFGH_JKLM",
    ] {
        assert!(validate_user_code(invalid).is_err(), "accepted {invalid}");
    }

    assert!(validate_device_code("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_ok());
    for invalid in [
        "too-short",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA!",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        assert!(validate_device_code(invalid).is_err(), "accepted {invalid}");
    }

    assert!(validate_api_key("1234567").is_err());
    assert!(validate_api_key("12345678").is_ok());
    assert!(validate_api_key(&"x".repeat(4096)).is_ok());
    assert!(validate_api_key(&"x".repeat(4097)).is_err());
    assert!(validate_api_key(&"é".repeat(4)).is_ok());
    assert!(validate_api_key("1234567\n8").is_err());
    assert_eq!(
        parse_key_input(format!("{}\n", "x".repeat(4096)).into_bytes()).unwrap(),
        "x".repeat(4096)
    );
    assert!(parse_key_input(vec![b'x'; MAX_API_KEY_STDIN_BYTES as usize + 1]).is_err());
    assert_eq!(
        validate_label("  Codewhale\tCLI  ").unwrap(),
        "Codewhale CLI"
    );
    assert!(validate_label(&"x".repeat(80)).is_ok());
    assert!(validate_label(&"x".repeat(81)).is_err());
}

#[test]
fn device_flow_handles_pending_then_authorized_without_printing_tokens() {
    let (temp, config) = test_config();
    let _keep_temp = temp;
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![
        response(
            200,
            json!({
                "deviceCode": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "userCode": "ABCD-EFGH-JKLM",
                "verificationUri": "https://app.codewhale.net/cli/authorize",
                "verificationUriComplete": "https://app.codewhale.net/cli/authorize?user_code=ABCD-EFGH-JKLM",
                "expiresIn": 600,
                "interval": 1
            }),
        ),
        response(202, json!({ "status": "authorization_pending" })),
        response(
            200,
            auth_json("access-never-print", "refresh-never-print", "acct-123"),
        ),
        response(200, account("acct-123")),
    ]);
    let mut output = Vec::new();
    let mut key_reader = |_| bail!("key reader should not be called");
    let mut opened = Vec::new();
    let mut opener = |url: String| {
        opened.push(url);
        true
    };
    let mut sleeper = |_| {};
    run_with(
        command(&["codewhale", "cloud", "login"]),
        "work",
        "https://api.codewhale.net",
        &config,
        &secrets,
        &secrets,
        &machine::MachineKeyEnv::default(),
        &transport,
        &mut output,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    )
    .unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("ABCD-EFGH-JKLM"));
    assert!(output.contains("Account ID: acct-123"));
    assert!(output.contains("Profile: work"));
    // No-brand invariant: login signs in the account; the internal
    // cloud-agent credential is never taught here.
    assert!(!output.to_lowercase().contains("daytona"), "{output}");
    assert!(!output.contains("set-slot"), "{output}");
    assert!(!output.contains("access-never-print"));
    assert!(!output.contains("refresh-never-print"));
    assert_eq!(opened.len(), 1);
    let requests = transport.requests();
    assert_eq!(requests[0].path, "/api/cli/device/start");
    assert_eq!(requests[1].path, "/api/cli/device/token");
    assert_eq!(requests[2].path, "/api/cli/device/token");
    assert_eq!(requests[3].path, "/api/me");
}

#[test]
fn cloud_sessions_are_isolated_by_profile_and_api_origin() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![]);
    let default = CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net");
    let work = CloudClient::new(&transport, &secrets, "work", "https://api.codewhale.net");
    let local = CloudClient::new(&transport, &secrets, "default", "http://127.0.0.1:8787");
    default
        .save_auth(auth("a-default", "r-default", "acct-default"))
        .unwrap();
    work.save_auth(auth("a-work", "r-work", "acct-work"))
        .unwrap();
    local
        .save_auth(auth("a-local", "r-local", "acct-local"))
        .unwrap();

    assert_eq!(
        default
            .load_auth()
            .unwrap()
            .unwrap()
            .bundle
            .user
            .unwrap()
            .id,
        "acct-default"
    );
    assert_eq!(
        work.load_auth().unwrap().unwrap().bundle.user.unwrap().id,
        "acct-work"
    );
    assert_eq!(
        local.load_auth().unwrap().unwrap().bundle.user.unwrap().id,
        "acct-local"
    );
}

#[test]
fn status_refreshes_once_on_unauthorized_and_never_displays_tokens() {
    let (temp, config) = test_config();
    let _keep_temp = temp;
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![
        response(401, json!({ "code": "access_token_expired" })),
        response(
            200,
            auth_json("access-new-secret", "refresh-new-secret", "acct-refresh"),
        ),
        response(200, account("acct-refresh")),
    ]);
    CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
        .save_auth(auth(
            "access-old-secret",
            "refresh-old-secret",
            "acct-refresh",
        ))
        .unwrap();
    let mut output = Vec::new();
    let mut key_reader = |_| bail!("unused");
    let mut opener = |_| true;
    let mut sleeper = |_| {};
    run_with(
        CloudCommand::Status,
        "default",
        "https://api.codewhale.net",
        &config,
        &secrets,
        &secrets,
        &machine::MachineKeyEnv::default(),
        &transport,
        &mut output,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("acct-refresh"));
    for secret in [
        "access-old-secret",
        "refresh-old-secret",
        "access-new-secret",
        "refresh-new-secret",
    ] {
        assert!(!output.contains(secret));
    }
    let requests = transport.requests();
    assert_eq!(requests[0].path, "/api/me");
    assert_eq!(requests[1].path, "/api/auth/refresh");
    assert_eq!(requests[2].path, "/api/me");
}

#[test]
fn account_pull_refuses_to_claim_unimplemented_local_import() {
    let (temp, config) = test_config();
    let config_path = config.path().to_path_buf();
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![]);
    let mut output = Vec::new();
    let mut key_reader = |_| bail!("unused");
    let mut opener = |_| true;
    let mut sleeper = |_| {};

    let error = run_with(
        command(&["codewhale", "account", "pull"]),
        "default",
        "https://api.codewhale.net",
        &config,
        &secrets,
        &secrets,
        &machine::MachineKeyEnv::default(),
        &transport,
        &mut output,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    )
    .expect_err("non-dry-run pull must fail until settings import exists");

    assert!(error.to_string().contains("import is not available"));
    assert!(error.to_string().contains("local config was not changed"));
    assert!(
        output.is_empty(),
        "a rejected pull must not print success text"
    );
    assert!(
        transport.requests().is_empty(),
        "a rejected pull needs no API call"
    );
    assert!(
        !config_path.exists(),
        "a rejected pull must not create config.toml"
    );
    drop(temp);
}

#[test]
fn account_pull_dry_run_is_truthful_and_read_only() {
    let (temp, config) = test_config();
    let config_path = config.path().to_path_buf();
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![response(200, account("acct-pull"))]);
    CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
        .save_auth(auth("access-secret", "refresh-secret", "acct-pull"))
        .unwrap();
    let mut output = Vec::new();
    let mut key_reader = |_| bail!("unused");
    let mut opener = |_| true;
    let mut sleeper = |_| {};

    run_with(
        command(&["codewhale", "account", "pull", "--dry-run"]),
        "default",
        "https://api.codewhale.net",
        &config,
        &secrets,
        &secrets,
        &machine::MachineKeyEnv::default(),
        &transport,
        &mut output,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    )
    .unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("Account settings (pull --dry-run):"));
    assert!(output.contains("Account ID: acct-pull"));
    assert!(output.contains("remote settings import is not available"));
    assert!(output.contains("local config unchanged"));
    assert!(!output.contains("Pulled account document"));
    assert!(!output.contains("would hydrate"));
    assert!(!output.contains("access-secret"));
    assert!(!output.contains("refresh-secret"));
    assert!(!config_path.exists(), "dry-run must not create config.toml");
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].method == HttpMethod::Get);
    assert_eq!(requests[0].path, "/api/me");
    drop(temp);
}

#[test]
fn non_terminal_refresh_responses_preserve_the_local_session() {
    for status in [403, 429, 500, 503] {
        let (secrets, _) = test_secrets();
        let transport = FakeTransport::new(vec![
            response(401, json!({ "code": "access_token_expired" })),
            response(status, json!({ "code": "temporarily_unavailable" })),
        ]);
        let client = CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net");
        client
            .save_auth(auth(
                "access-old-secret",
                "refresh-still-valid",
                "acct-refresh",
            ))
            .unwrap();

        let error = client
            .me()
            .err()
            .expect("refresh response should fail the request")
            .to_string();
        assert!(error.contains(&format!("HTTP {status}")));
        assert_eq!(
            client
                .load_auth()
                .unwrap()
                .expect("retryable refresh failure must preserve the session")
                .bundle
                .refresh_token,
            "refresh-still-valid"
        );
        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/api/me");
        assert_eq!(requests[1].path, "/api/auth/refresh");
    }
}

/// Revoke is defined as idempotent by the control plane (a repeat returns the
/// identical `revokedAt`), so a rate-limited revoke may be replayed — and the
/// server's own `Retry-After` decides how long CI waits, not a guess.
#[test]
fn a_rate_limited_revoke_waits_the_server_named_interval_and_replays() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![
        response_retry_after(
            429,
            json!({
                "error": "rate_limited",
                "message": "slow down",
                "details": { "code": "rate_limited" }
            }),
            4,
        ),
        response(
            200,
            json!({ "ok": true, "apiKey": { "id": "3f2a9c1e4b7d8a0f5c6e2b91", "revokedAt": "2026-02-02T00:00:00Z" } }),
        ),
    ]);
    let client = CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net");
    client
        .save_auth(auth("access-secret", "refresh-secret", "acct-1"))
        .unwrap();
    let mut slept = Vec::new();
    let mut sleeper = |duration: Duration| slept.push(duration);
    let response = client
        .execute_authenticated_with_retry(
            HttpMethod::Delete,
            "/api/account/api-keys/3f2a9c1e4b7d8a0f5c6e2b91",
            None,
            machine::Retry::Idempotent,
            &mut sleeper,
        )
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(slept, vec![Duration::from_secs(4)]);
    assert_eq!(transport.requests().len(), 2);
}

/// The one POST in this surface mints a secret shown exactly once. A replay
/// that actually succeeded server-side would leave a key the caller can never
/// revoke by id, so `Retry::Never` must mean never — even on a 429.
#[test]
fn a_rate_limited_create_is_never_replayed() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![response_retry_after(
        429,
        json!({
            "error": "rate_limited",
            "message": "slow down",
            "details": { "code": "rate_limited" }
        }),
        4,
    )]);
    let client = CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net");
    client
        .save_auth(auth("access-secret", "refresh-secret", "acct-1"))
        .unwrap();
    let mut sleeper = |_: Duration| panic!("create must never sleep-and-retry");
    let response = client
        .execute_authenticated_with_retry(
            HttpMethod::Post,
            "/api/account/api-keys",
            Some(b"{}".to_vec()),
            machine::Retry::Never,
            &mut sleeper,
        )
        .unwrap();
    assert_eq!(response.status, 429);
    assert_eq!(transport.requests().len(), 1);
}

#[test]
fn refresh_transport_failure_preserves_the_local_session() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![response(
        401,
        json!({ "code": "access_token_expired" }),
    )]);
    let client = CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net");
    client
        .save_auth(auth(
            "access-old-secret",
            "refresh-still-valid",
            "acct-refresh",
        ))
        .unwrap();

    let error = client
        .me()
        .err()
        .expect("refresh transport should fail")
        .to_string();
    assert!(error.contains("fake transport exhausted"));
    assert_eq!(
        client
            .load_auth()
            .unwrap()
            .expect("transport failure must preserve the session")
            .bundle
            .refresh_token,
        "refresh-still-valid"
    );
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].path, "/api/auth/refresh");
}

#[test]
fn terminal_refresh_auth_failures_clear_the_local_session() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![
        response(401, json!({ "code": "access_token_expired" })),
        response(401, json!({ "code": "invalid_refresh_token" })),
    ]);
    let client = CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net");
    client
        .save_auth(auth(
            "access-old-secret",
            "refresh-terminal-secret",
            "acct-refresh",
        ))
        .unwrap();

    let error = client
        .me()
        .err()
        .expect("terminal refresh response should fail the request")
        .to_string();
    assert!(error.contains("session expired"));
    assert!(
        client.load_auth().unwrap().is_none(),
        "HTTP 401 must clear the terminal session"
    );
}

#[test]
fn set_list_and_remove_use_account_routes_without_secret_output() {
    let (temp, config) = test_config();
    let _keep_temp = temp;
    let (secrets, _) = test_secrets();
    let list_account = json!({
        "user": {
            "id": "acct-keys",
            "displayName": "Hunter",
            "email": "hunter@example.test",
            "modelKeys": {
                "openai": { "configured": true, "label": "Laptop", "updatedAt": "now" }
            }
        }
    });
    let transport = FakeTransport::new(vec![
        response(200, account("acct-keys")),
        response(200, json!({ "ok": true })),
        response(200, list_account),
        response(200, account("acct-keys")),
        response(204, json!(null)),
    ]);
    CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
        .save_auth(auth("access-secret", "refresh-secret", "acct-keys"))
        .unwrap();
    let mut output = Vec::new();
    let mut key_reader = |_| Ok("sk-provider-never-print".to_string());
    let mut opener = |_| true;
    let mut sleeper = |_| {};
    for cmd in [
        command(&[
            "codewhale",
            "cloud",
            "keys",
            "set",
            "openai",
            "--api-key-stdin",
            "--label",
            "Laptop",
        ]),
        command(&["codewhale", "cloud", "keys", "list"]),
        command(&["codewhale", "cloud", "keys", "remove", "openai"]),
    ] {
        run_with(
            cmd,
            "default",
            "https://api.codewhale.net",
            &config,
            &secrets,
            &secrets,
            &machine::MachineKeyEnv::default(),
            &transport,
            &mut output,
            &mut key_reader,
            &mut opener,
            &mut sleeper,
        )
        .unwrap();
    }
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("openai: set"));
    assert!(!output.contains("Laptop"));
    assert!(output.contains("Codewhale account acct-keys"));
    assert!(!output.contains("sk-provider-never-print"));
    assert!(!output.contains("access-secret"));
    assert!(!output.contains("refresh-secret"));

    let requests = transport.requests();
    let put = requests
        .iter()
        .find(|request| request.method == HttpMethod::Put)
        .unwrap();
    assert_eq!(put.path, "/api/model-keys/openai");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(put.body.as_ref().unwrap()).unwrap(),
        json!({ "key": "sk-provider-never-print", "label": "Laptop" })
    );
    assert!(requests.iter().any(|request| {
        request.method == HttpMethod::Delete && request.path == "/api/model-keys/openai"
    }));
}

#[test]
fn from_local_uses_config_without_printing_or_requiring_an_inline_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let mut config = ConfigStore::load(Some(path)).unwrap();
    config.config.providers.anthropic.api_key = Some("sk-local-upload-secret".to_string());
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![
        response(200, account("acct-local")),
        response(200, json!({ "ok": true })),
    ]);
    CloudClient::new(&transport, &secrets, "work", "https://api.codewhale.net")
        .save_auth(auth("access", "refresh", "acct-local"))
        .unwrap();
    let mut output = Vec::new();
    let mut key_reader = |_| bail!("from-local must not prompt");
    let mut opener = |_| true;
    let mut sleeper = |_| {};
    run_with(
        command(&[
            "codewhale",
            "cloud",
            "keys",
            "set",
            "anthropic",
            "--from-local",
        ]),
        "work",
        "https://api.codewhale.net",
        &config,
        &secrets,
        &secrets,
        &machine::MachineKeyEnv::default(),
        &transport,
        &mut output,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("acct-local"));
    assert!(!output.contains("sk-local-upload-secret"));
    let requests = transport.requests();
    let put = requests
        .iter()
        .find(|request| request.method == HttpMethod::Put)
        .unwrap();
    assert!(String::from_utf8_lossy(put.body.as_ref().unwrap()).contains("sk-local-upload-secret"));
}

#[test]
fn from_local_uses_config_before_the_provider_secret_store() {
    let (temp, mut config) = test_config();
    let _keep_temp = temp;
    let (secrets, store) = test_secrets();
    store.set("openai", "sk-secret-store").unwrap();

    assert_eq!(
        resolve_local_key(&config, &secrets, CloudProvider::Openai)
            .unwrap()
            .as_deref(),
        Some("sk-secret-store")
    );
    config.config.providers.openai.api_key = Some("sk-config-first".to_string());
    assert_eq!(
        resolve_local_key(&config, &secrets, CloudProvider::Openai)
            .unwrap()
            .as_deref(),
        Some("sk-config-first")
    );
}

#[test]
fn logout_recovers_from_a_corrupt_local_session_record() {
    let (temp, config) = test_config();
    let _keep_temp = temp;
    let (secrets, store) = test_secrets();
    let slot = cloud_auth_slot("default", "https://api.codewhale.net");
    store.set(&slot, "not-json-and-not-a-token").unwrap();
    let transport = FakeTransport::new(vec![]);
    let mut output = Vec::new();
    let mut key_reader = |_| bail!("unused");
    let mut opener = |_| true;
    let mut sleeper = |_| {};
    run_with(
        CloudCommand::Logout,
        "default",
        "https://api.codewhale.net",
        &config,
        &secrets,
        &secrets,
        &machine::MachineKeyEnv::default(),
        &transport,
        &mut output,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    )
    .unwrap();
    assert!(store.get(&slot).unwrap().is_none());
    assert!(
        !String::from_utf8(output)
            .unwrap()
            .contains("not-json-and-not-a-token")
    );
}

#[test]
fn logout_clears_obsolete_or_wrong_origin_session_records() {
    let canonical_api_base = "https://api.codewhale.net";
    for (case, schema_version, stored_api_base) in [
        (
            "obsolete schema",
            ACCOUNT_SESSION_SCHEMA_VERSION.saturating_add(1),
            canonical_api_base,
        ),
        (
            "wrong origin",
            ACCOUNT_SESSION_SCHEMA_VERSION,
            "https://other.codewhale.net",
        ),
    ] {
        let (secrets, store) = test_secrets();
        let slot = cloud_auth_slot("default", canonical_api_base);
        let raw = serde_json::to_string(&StoredCloudAuth {
            schema_version,
            api_base: stored_api_base.to_string(),
            bundle: auth("access-obsolete", "refresh-obsolete", "acct-obsolete"),
        })
        .unwrap();
        store.set(&slot, &raw).unwrap();
        let transport = FakeTransport::new(vec![]);
        let client = CloudClient::new(&transport, &secrets, "default", canonical_api_base);

        assert!(
            client.load_auth().unwrap().is_none(),
            "{case} must continue to load as signed out"
        );
        assert!(!client.logout().unwrap());
        assert!(
            store.get(&slot).unwrap().is_none(),
            "logout must scrub the {case} record"
        );
        assert!(transport.requests().is_empty());
    }
}

#[test]
fn server_errors_never_echo_response_messages() {
    let error = response_error(&response(
        400,
        json!({
            "error": {
                "code": "invalid_api_key",
                "message": "The submitted key was sk-never-echo-this"
            }
        }),
    ))
    .to_string();
    assert!(error.contains("invalid_api_key"));
    assert!(!error.contains("sk-never-echo-this"));
}

#[test]
fn cloud_auth_slot_does_not_embed_profile_or_origin() {
    let slot = cloud_auth_slot("private-profile", "https://api.codewhale.net");
    assert!(!slot.contains("private-profile"));
    assert!(!slot.contains("api.codewhale.net"));
    assert_ne!(
        slot,
        cloud_auth_slot("other-profile", "https://api.codewhale.net")
    );
}

#[test]
fn fake_store_is_profile_safe() {
    let (_, store) = test_secrets();
    store.set("unrelated", "keep-me").unwrap();
    store.delete("missing").unwrap();
    assert_eq!(store.get("unrelated").unwrap().as_deref(), Some("keep-me"));
}

#[test]
fn account_login_timeout_fails_the_command() {
    // §2.3 / #5033 class: a timed-out device login printed the timeout yet the
    // process exited 0. Pin the contract at the run_with seam — the command
    // must return Err so run_cli maps it to ExitCode::FAILURE. Verified live
    // against a stub server: `error: Codewhale account login timed out` now
    // exits 1.
    let (temp, config) = test_config();
    let _keep_temp = temp;
    let (secrets, _) = test_secrets();
    // Device start succeeds once; every token poll stays pending forever.
    struct PendingLogin;
    impl CloudTransport for PendingLogin {
        fn execute(&self, request: CloudRequest) -> Result<CloudResponse> {
            if request.path == "/api/cli/device/start" {
                return Ok(response(
                    200,
                    json!({
                        "deviceCode": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "userCode": "ABCD-EFGH-JKLM",
                        "verificationUri": "https://app.codewhale.net/cli/authorize",
                        "verificationUriComplete": "https://app.codewhale.net/cli/authorize?user_code=ABCD-EFGH-JKLM",
                        "expiresIn": 600,
                        "interval": 1
                    }),
                ));
            }
            Ok(response(202, json!({ "status": "authorization_pending" })))
        }
    }
    let pending = PendingLogin;
    let mut output = Vec::new();
    let mut key_reader = |_| bail!("key reader should not be called");
    let mut opener = |_| true;
    // A real (short) sleep keeps the pending loop from busy-spinning while
    // still reaching the 1s client timeout quickly.
    let mut sleeper = |duration: std::time::Duration| {
        std::thread::sleep(duration.min(std::time::Duration::from_millis(50)))
    };
    let result = run_with(
        command(&[
            "codewhale",
            "cloud",
            "login",
            "--no-open",
            "--timeout-seconds",
            "1",
        ]),
        "default",
        "https://api.codewhale.net",
        &config,
        &secrets,
        &secrets,
        &machine::MachineKeyEnv::default(),
        &pending,
        &mut output,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    );
    let err = match result {
        Ok(()) => panic!("a timed-out login must return Err so the exit code is non-zero"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("login timed out"),
        "timeout error text: {err}"
    );
}

// ---------------------------------------------------------------------------
// Machine-token command surface, end to end through `run_with`.
// ---------------------------------------------------------------------------

const MACHINE_TOKEN: &str =
    "cwc_key_3f2a9c1e4b7d8a0f5c6e2b91_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-xQRST";

fn machine_env() -> machine::MachineKeyEnv {
    machine::MachineKeyEnv::from_raw(Some(MACHINE_TOKEN))
}

/// Drive one `codewhale account …` invocation with a scripted transport.
fn run_account(
    argv: &[&str],
    machine: &machine::MachineKeyEnv,
    secrets: &Secrets,
    transport: &FakeTransport,
) -> (Result<()>, String) {
    let (temp, config) = test_config();
    let _keep_temp = temp;
    let mut output = Vec::new();
    let mut key_reader = |_| bail!("key reader should not be called");
    let mut opener = |_| true;
    let mut sleeper = |_| {};
    let result = run_with(
        command(argv),
        "default",
        "https://api.codewhale.net",
        &config,
        secrets,
        secrets,
        machine,
        transport,
        &mut output,
        &mut key_reader,
        &mut opener,
        &mut sleeper,
    );
    (result, String::from_utf8(output).unwrap())
}

#[test]
fn whoami_with_a_machine_key_uses_the_key_route_and_never_the_session_route() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![response(
        200,
        json!({
            "account": { "id": "user_1", "displayName": "Hunter", "email": "h@example.test",
                         "region": "us-west", "plan": "free" },
            "apiKey": { "id": "3f2a9c1e4b7d8a0f5c6e2b91", "name": "github-actions",
                        "displayPrefix": "cwc_key_3f2a9c1e4b7d8a0f5c6e2b91",
                        "scopes": ["account:read", "agent:run"],
                        "createdAt": "2026-01-01T00:00:00Z" },
            "agent": { "configured": true, "modelProvider": "deepseek" }
        }),
    )]);
    let (result, output) = run_account(
        &["codewhale", "account", "whoami"],
        &machine_env(),
        &secrets,
        &transport,
    );
    result.unwrap();
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/account/api-key/whoami");
    // Exactly one credential on the wire, and it is the machine key.
    assert_eq!(requests[0].bearer.as_deref(), Some(MACHINE_TOKEN));
    assert!(output.contains("user_1"), "{output}");
    assert!(
        output.contains("cwc_key_3f2a9c1e4b7d8a0f5c6e2b91"),
        "{output}"
    );
    assert!(
        !output.contains(&MACHINE_TOKEN[32..]),
        "secret half leaked: {output}"
    );
}

/// The load-bearing failure mode: a machine credential that fails must not
/// quietly become a human one, or CI runs as the wrong identity.
#[test]
fn a_rejected_machine_key_never_falls_back_to_the_stored_session() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![response(
        401,
        json!({
            "error": "unauthorized",
            "message": "invalid key",
            "details": { "code": "api_key_invalid" }
        }),
    )]);
    // A perfectly good interactive session exists alongside the bad key.
    CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
        .save_auth(auth("access-secret", "refresh-secret", "acct-human"))
        .unwrap();
    let (result, output) = run_account(
        &["codewhale", "account", "whoami"],
        &machine_env(),
        &secrets,
        &transport,
    );
    let err = result.expect_err("an invalid machine key must fail the command");
    assert_eq!(
        transport.requests().len(),
        1,
        "there must be no second attempt"
    );
    assert!(output.is_empty(), "nothing should be printed: {output}");
    let machine_error = err
        .downcast_ref::<machine::MachineError>()
        .expect("the failure must carry a class");
    assert_eq!(machine_error.exit_code, machine::EXIT_AUTH);
    assert!(err.to_string().contains("is not valid"), "{err}");
    // The human session is untouched: a bad key is not a reason to log anyone out.
    assert!(
        CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
            .load_auth()
            .unwrap()
            .is_some()
    );
}

#[test]
fn managing_keys_with_only_a_machine_key_is_refused_before_anything_is_sent() {
    let (secrets, _) = test_secrets();
    for argv in [
        vec!["codewhale", "account", "api-keys", "list"],
        vec!["codewhale", "account", "api-keys", "create", "--name", "ci"],
        vec![
            "codewhale",
            "account",
            "api-keys",
            "revoke",
            "3f2a9c1e4b7d8a0f5c6e2b91",
        ],
    ] {
        let transport = FakeTransport::new(Vec::new());
        let (result, output) = run_account(&argv, &machine_env(), &secrets, &transport);
        let err = result.expect_err("a key cannot manage keys");
        assert!(
            transport.requests().is_empty(),
            "{argv:?} put the key on the wire"
        );
        assert!(output.is_empty(), "{argv:?}: {output}");
        assert!(
            err.to_string()
                .contains("Managing API keys needs an interactive login."),
            "{argv:?}: {err}"
        );
    }
}

#[test]
fn creating_a_key_prints_the_secret_exactly_once_and_saves_it_nowhere() {
    let (secrets, keyring) = test_secrets();
    let transport = FakeTransport::new(vec![response(
        201,
        json!({
            "apiKey": { "id": "3f2a9c1e4b7d8a0f5c6e2b91", "name": "github-actions",
                        "displayPrefix": "cwc_key_3f2a9c1e4b7d8a0f5c6e2b91",
                        "scopes": ["account:read", "agent:run"],
                        "createdAt": "2026-01-01T00:00:00Z", "expiresAt": null },
            "secret": MACHINE_TOKEN
        }),
    )]);
    CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
        .save_auth(auth("access-secret", "refresh-secret", "acct-human"))
        .unwrap();
    let (result, output) = run_account(
        &[
            "codewhale",
            "account",
            "api-keys",
            "create",
            "--name",
            "github-actions",
            "--expires-in-days",
            "90",
        ],
        &machine::MachineKeyEnv::default(),
        &secrets,
        &transport,
    );
    result.unwrap();
    assert_eq!(output.matches(MACHINE_TOKEN).count(), 1, "{output}");
    assert!(
        output.contains("ONLY TIME YOU WILL SEE THIS SECRET"),
        "{output}"
    );

    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/account/api-keys");
    let body: serde_json::Value =
        serde_json::from_slice(requests[0].body.as_ref().unwrap()).unwrap();
    assert_eq!(body["name"], "github-actions");
    assert_eq!(body["expiresInDays"], 90);
    // Scopes omitted means "both"; sending an empty array would mean nothing.
    assert!(body.get("scopes").is_none(), "{body}");

    // The plaintext exists in one response and nowhere else, ever: creating a
    // key must not write it into the session record on its way past.
    let stored = keyring
        .get(&cloud_auth_slot("default", "https://api.codewhale.net"))
        .unwrap()
        .expect("the session record is still there");
    assert!(
        !stored.contains(MACHINE_TOKEN),
        "the secret reached storage"
    );
}

#[test]
fn a_bad_key_name_is_rejected_locally_without_a_round_trip() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(Vec::new());
    CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
        .save_auth(auth("access-secret", "refresh-secret", "acct-human"))
        .unwrap();
    let (result, _) = run_account(
        &[
            "codewhale",
            "account",
            "api-keys",
            "create",
            "--name",
            "bad*name",
        ],
        &machine::MachineKeyEnv::default(),
        &secrets,
        &transport,
    );
    let err = result.expect_err("`*` is outside the server's name pattern");
    assert!(transport.requests().is_empty());
    assert!(
        err.to_string().contains("only letters, digits, spaces"),
        "{err}"
    );
}

#[test]
fn the_agent_precondition_surfaces_the_409_with_its_own_exit_class() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![response(
        409,
        json!({
            "error": "conflict",
            "message": "no agent model",
            "details": { "code": "account_agent_model_unconfigured" }
        }),
    )]);
    let (result, _) = run_account(
        &["codewhale", "account", "agent"],
        &machine_env(),
        &secrets,
        &transport,
    );
    let err = result.expect_err("machine work needs a model");
    let machine_error = err.downcast_ref::<machine::MachineError>().unwrap();
    // A configuration problem, not a credential problem — and CI must be able
    // to tell them apart from the exit code alone.
    assert_eq!(machine_error.exit_code, machine::EXIT_AGENT_UNCONFIGURED);
    assert_ne!(machine_error.exit_code, machine::EXIT_AUTH);
    assert_eq!(transport.requests().len(), 1, "409 must not be retried");
    assert!(
        err.to_string().contains("codewhale account keys set"),
        "{err}"
    );
}

#[test]
fn the_agent_command_has_no_session_fallback() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(Vec::new());
    CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
        .save_auth(auth("access-secret", "refresh-secret", "acct-human"))
        .unwrap();
    let (result, _) = run_account(
        &["codewhale", "account", "agent"],
        &machine::MachineKeyEnv::default(),
        &secrets,
        &transport,
    );
    let err = result.expect_err("the agent route is machine-key-only");
    assert!(transport.requests().is_empty());
    assert!(err.to_string().contains("CODEWHALE_API_KEY"), "{err}");
}

#[test]
fn whoami_without_a_machine_key_still_reports_the_interactive_session() {
    let (secrets, _) = test_secrets();
    let transport = FakeTransport::new(vec![response(200, account("acct-human"))]);
    CloudClient::new(&transport, &secrets, "default", "https://api.codewhale.net")
        .save_auth(auth("access-secret", "refresh-secret", "acct-human"))
        .unwrap();
    let (result, output) = run_account(
        &["codewhale", "account", "whoami"],
        &machine::MachineKeyEnv::default(),
        &secrets,
        &transport,
    );
    result.unwrap();
    assert_eq!(transport.requests()[0].path, "/api/me");
    assert!(output.contains("acct-human"), "{output}");
}

#[test]
fn the_machine_token_surface_is_a_different_noun_from_the_provider_vault() {
    // `account keys` is the BYOK provider vault; `account api-keys` is the
    // machine tokens. Merging them would let one typo revoke the wrong thing.
    assert!(matches!(
        command(&["codewhale", "account", "keys", "list"]),
        CloudCommand::Keys(_)
    ));
    assert!(matches!(
        command(&["codewhale", "account", "api-keys", "list"]),
        CloudCommand::ApiKeys(_)
    ));
}
