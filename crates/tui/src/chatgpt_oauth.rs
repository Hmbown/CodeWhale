//! Native ChatGPT / Codex subscription login (authorization-code + PKCE).
//!
//! This adapter signs in through the published OpenAI issuer
//! (`https://auth.openai.com`) and stores refreshable tokens in
//! Codewhale-owned storage. It never reads Codex CLI cookies and never
//! writes `~/.codex/auth.json`.
//!
//! # Terms boundary
//!
//! OpenAI publishes Sign in with ChatGPT for the Codex app, CLI, and IDE
//! ([learn.chatgpt.com/docs/auth](https://learn.chatgpt.com/docs/auth)) and
//! advertises authorization-code + PKCE S256 plus `refresh_token` on the
//! issuer's OIDC discovery document
//! (`https://auth.openai.com/.well-known/openid-configuration`). OpenAI has
//! not published a third-party client-registration path for this public
//! Codex client. This adapter:
//!
//! - uses the Apache-2.0 Codex CLI public client id with `originator=codewhale`
//!   (never `codex_cli_rs`)
//! - uses the loopback redirect ports that public client registers
//!   (`1455`, fallback `1457`)
//! - uses `/oauth/authorize` and `/oauth/token` on the published issuer, the
//!   paths that public client is registered against
//! - posts best-effort remote revoke to the fixed
//!   `{issuer}/api/accounts/oauth/revoke`, the path that public client is
//!   registered against. This is deliberately not read from the discovery
//!   document: revoke must still clear local credentials when the issuer is
//!   unreachable, so adding a discovery fetch would only add a failure mode to
//!   a path whose contract is to clean up regardless
//! - does **not** call unpublished device-auth paths (`/api/accounts/deviceauth/*`);
//!   the issuer does not advertise `device_authorization_endpoint`
//!
//! External Codex CLI import remains an explicit alternative, not a
//! prerequisite. Token values are never logged.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::{ApiProvider, Config};

/// Codex CLI public OAuth client (public PKCE client; no secret).
pub const CHATGPT_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Published issuer.
pub const CHATGPT_OAUTH_ISSUER: &str = "https://auth.openai.com";
/// Honest originator; never impersonate `codex_cli_rs`.
pub const CHATGPT_OAUTH_ORIGINATOR: &str = "codewhale";
pub const CHATGPT_OAUTH_SCOPE: &str = "openid profile email offline_access";
pub const CHATGPT_OAUTH_CALLBACK_PATH: &str = "/auth/callback";
pub const CHATGPT_OAUTH_LOOPBACK_PORTS: [u16; 2] = [1455, 1457];
const REFRESH_SKEW_SECS: i64 = 60;
const OAUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const OAUTH_RESPONSE_BODY_LIMIT: u64 = 64 * 1024;
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
const CALLBACK_HTML_OK: &str = "<!doctype html><html><body><p>Signed in to Codewhale. You can close this tab.</p></body></html>";
const CALLBACK_HTML_ERR: &str = "<!doctype html><html><body><p>Sign-in did not complete. You can close this tab and retry in Codewhale.</p></body></html>";

#[derive(Clone)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
}

#[derive(Clone)]
pub struct AuthRequest {
    pub state: String,
    pub pkce: PkceChallenge,
    pub redirect_uri: String,
    pub authorize_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackOutcome {
    Success {
        code: String,
        state: String,
    },
    Error {
        error: String,
        description: Option<String>,
        state: Option<String>,
    },
}

#[derive(Clone, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    id_token: Option<String>,
    error: Option<String>,
    #[allow(dead_code)]
    error_description: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ChatgptAuthEntry {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    oidc_issuer: Option<String>,
    #[serde(default)]
    oidc_client_id: Option<String>,
    #[serde(default)]
    originator: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

/// Resolved bearer credential ready for the Responses route.
#[derive(Clone)]
pub struct ChatgptOAuthCredentials {
    pub access_token: String,
    pub account_id: Option<String>,
    #[allow(dead_code)]
    pub refresh_token: Option<String>,
    #[allow(dead_code)]
    pub expires_at: Option<String>,
}

/// Successful PKCE exchange that has not yet been made active.
#[derive(Debug)]
pub struct PendingChatgptPkceLogin {
    issuer: String,
    client_id: String,
    token: TokenResponse,
}

/// Receipt for the committed Codewhale-owned ChatGPT OAuth generation.
#[derive(Debug)]
pub struct ChatgptPkceActivation {
    #[allow(dead_code)]
    pub credentials: ChatgptOAuthCredentials,
    pub config_path: PathBuf,
    pub auth_path: PathBuf,
}

pub(crate) trait TokenClient {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String)>;
}

struct ReqwestTokenClient;

impl TokenClient for ReqwestTokenClient {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String)> {
        #[cfg(test)]
        crate::external_credentials::record_oauth_network();
        let client = crate::tls::reqwest_blocking_client_builder()
            .timeout(OAUTH_REQUEST_TIMEOUT)
            .build()
            .context("Failed to build ChatGPT OAuth HTTP client")?;
        let response = client
            .post(url)
            .form(form)
            .send()
            .context("ChatGPT OAuth token request failed")?;
        let status = response.status().as_u16();
        let mut reader = response.take(OAUTH_RESPONSE_BODY_LIMIT + 1);
        let mut body = Vec::new();
        reader
            .read_to_end(&mut body)
            .context("reading ChatGPT OAuth token response")?;
        if body.len() as u64 > OAUTH_RESPONSE_BODY_LIMIT {
            body.truncate(OAUTH_RESPONSE_BODY_LIMIT as usize);
        }
        let text = String::from_utf8(body).unwrap_or_default();
        Ok((status, text))
    }
}

fn redacted(present: bool) -> &'static str {
    if present { "<redacted>" } else { "<none>" }
}

impl std::fmt::Debug for PkceChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PkceChallenge")
            .field("verifier", &redacted(true))
            .field("challenge", &self.challenge)
            .finish()
    }
}

impl std::fmt::Debug for AuthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRequest")
            .field("state", &self.state)
            .field("pkce", &self.pkce)
            .field("redirect_uri", &self.redirect_uri)
            .field("authorize_url", &self.authorize_url)
            .finish()
    }
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &redacted(self.access_token.is_some()))
            .field("refresh_token", &redacted(self.refresh_token.is_some()))
            .field("expires_in", &self.expires_in)
            .field("id_token", &redacted(self.id_token.is_some()))
            .field("error", &self.error)
            .finish()
    }
}

impl std::fmt::Debug for ChatgptAuthEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatgptAuthEntry")
            .field("access_token", &redacted(self.access_token.is_some()))
            .field("refresh_token", &redacted(self.refresh_token.is_some()))
            .field("expires_at", &self.expires_at)
            .field("id_token", &redacted(self.id_token.is_some()))
            .field("account_id", &self.account_id)
            .field("oidc_issuer", &self.oidc_issuer)
            .field("oidc_client_id", &self.oidc_client_id)
            .field("originator", &self.originator)
            .finish()
    }
}

impl std::fmt::Debug for ChatgptOAuthCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatgptOAuthCredentials")
            .field("access_token", &redacted(true))
            .field("account_id", &self.account_id)
            .field("refresh_token", &redacted(self.refresh_token.is_some()))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// RFC 7636 S256 PKCE pair.
#[must_use]
pub fn generate_pkce() -> PkceChallenge {
    let verifier = random_url_token(32);
    let digest = Sha256::digest(verifier.as_bytes());
    PkceChallenge {
        verifier,
        challenge: URL_SAFE_NO_PAD.encode(digest),
    }
}

#[must_use]
pub fn generate_state() -> String {
    random_url_token(16)
}

fn random_url_token(nbytes: usize) -> String {
    let mut bytes = vec![0u8; nbytes.max(16)];
    let mut offset = 0;
    while offset < bytes.len() {
        let chunk = uuid::Uuid::new_v4();
        let take = (bytes.len() - offset).min(16);
        bytes[offset..offset + take].copy_from_slice(&chunk.as_bytes()[..take]);
        offset += take;
    }
    URL_SAFE_NO_PAD.encode(bytes)
}

#[must_use]
pub fn authorize_endpoint(issuer: &str) -> String {
    format!("{}/oauth/authorize", issuer.trim_end_matches('/'))
}

#[must_use]
pub fn token_endpoint(issuer: &str) -> String {
    format!("{}/oauth/token", issuer.trim_end_matches('/'))
}

#[must_use]
pub fn revoke_endpoint(issuer: &str) -> String {
    format!("{}/api/accounts/oauth/revoke", issuer.trim_end_matches('/'))
}

#[must_use]
pub fn redirect_uri_for_port(port: u16) -> String {
    format!("http://localhost:{port}{CHATGPT_OAUTH_CALLBACK_PATH}")
}

pub fn build_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    pkce: &PkceChallenge,
) -> Result<String> {
    // A malformed configured issuer must fail loudly. Silently redirecting
    // the browser to the production authorize endpoint would hand OpenAI a
    // sign-in the user aimed somewhere else.
    let mut url = reqwest::Url::parse(&authorize_endpoint(issuer)).with_context(|| {
        format!(
            "ChatGPT OAuth issuer is not a valid URL ({issuer:?}) — check CODEWHALE_CHATGPT_OAUTH_ISSUER"
        )
    })?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", CHATGPT_OAUTH_SCOPE)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("originator", CHATGPT_OAUTH_ORIGINATOR);
    Ok(url.to_string())
}

pub fn parse_callback_query(query: &str) -> Result<CallbackOutcome> {
    let parsed = reqwest::Url::parse(&format!(
        "http://127.0.0.1{CHATGPT_OAUTH_CALLBACK_PATH}?{query}"
    ))
    .context("OAuth callback query is not valid")?;
    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut description = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            "error_description" => description = Some(value.into_owned()),
            _ => {}
        }
    }
    if let Some(error) = error {
        return Ok(CallbackOutcome::Error {
            error,
            description,
            state,
        });
    }
    let code = code
        .filter(|c| !c.trim().is_empty())
        .context("OAuth callback missing authorization code")?;
    let state = state
        .filter(|s| !s.trim().is_empty())
        .context("OAuth callback missing state")?;
    Ok(CallbackOutcome::Success { code, state })
}

pub fn accept_callback(expected_state: &str, outcome: CallbackOutcome) -> Result<String> {
    match outcome {
        CallbackOutcome::Success { code, state } => {
            anyhow::ensure!(
                state == expected_state,
                "OAuth callback state did not match the pending login"
            );
            Ok(code)
        }
        CallbackOutcome::Error {
            error,
            description,
            state,
        } => {
            if let Some(state) = state {
                anyhow::ensure!(
                    state == expected_state,
                    "OAuth error callback state did not match the pending login"
                );
            }
            let detail = description
                .filter(|text| !text.trim().is_empty())
                .unwrap_or(error);
            bail!("ChatGPT sign-in was not completed: {detail}")
        }
    }
}

fn parse_http_request_target(request_line: &str) -> Result<String> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    anyhow::ensure!(
        method.eq_ignore_ascii_case("GET"),
        "OAuth callback must be GET"
    );
    let target = parts
        .next()
        .context("OAuth callback missing request target")?;
    Ok(target.to_string())
}

fn query_from_target(target: &str) -> Result<&str> {
    let path = target.split('?').next().unwrap_or(target);
    anyhow::ensure!(
        path == CHATGPT_OAUTH_CALLBACK_PATH,
        "OAuth callback path was not {CHATGPT_OAUTH_CALLBACK_PATH}"
    );
    Ok(target.split_once('?').map(|(_, q)| q).unwrap_or(""))
}

pub fn start_auth_request_on(
    listeners: &[TcpListener],
    issuer: &str,
    client_id: &str,
) -> Result<AuthRequest> {
    let port = listeners
        .first()
        .context("ChatGPT OAuth callback has no bound listener")?
        .local_addr()
        .context("ChatGPT OAuth callback listener has no local address")?
        .port();
    let redirect_uri = redirect_uri_for_port(port);
    let pkce = generate_pkce();
    let state = generate_state();
    let authorize_url = build_authorize_url(issuer, client_id, &redirect_uri, &state, &pkce)?;
    Ok(AuthRequest {
        state,
        pkce,
        redirect_uri,
        authorize_url,
    })
}

/// Bind the loopback callback on both IP stacks for the first free port.
///
/// The redirect URI has to say `localhost` -- that is what is registered with
/// the authorization server, and redirect matching is exact -- but `localhost`
/// resolves to `::1` before `127.0.0.1` on IPv6-first hosts. Binding only IPv4
/// left the browser connecting to a closed port, which browsers paper over with
/// Happy Eyeballs fallback: a working sign-in becomes a slow one, and a broken
/// one wherever that fallback is disabled. Binding both is the fix that keeps
/// the registered redirect URI intact.
///
/// A host with only one stack available binds only that one and still works.
pub fn bind_loopback_callback() -> Result<Vec<TcpListener>> {
    let mut last_error = None;
    for port in CHATGPT_OAUTH_LOOPBACK_PORTS {
        let mut bound = Vec::new();
        for addr in [
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
        ] {
            match TcpListener::bind(addr) {
                Ok(listener) => {
                    listener
                        .set_nonblocking(true)
                        .context("ChatGPT OAuth callback listener could not be set non-blocking")?;
                    bound.push(listener);
                }
                Err(error) => last_error = Some(error),
            }
        }
        if !bound.is_empty() {
            return Ok(bound);
        }
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("unable to bind ChatGPT OAuth callback ports")))
        .context(
            "ChatGPT sign-in needs loopback port 1455 or 1457. Stop the process holding that port, or import Codex CLI credentials with `codewhale auth external-consent`.",
        )
}

fn wait_for_callback(listeners: &[TcpListener], expected_state: &str) -> Result<String> {
    let deadline = Instant::now() + CALLBACK_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            bail!("ChatGPT sign-in timed out waiting for the browser callback");
        }
        // Whichever stack `localhost` resolved to for the browser is the one
        // that gets the connection; poll them all.
        for listener in listeners {
            match listener.accept() {
                Ok((stream, _)) => {
                    return handle_callback_stream(stream, expected_state);
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error).context("ChatGPT OAuth callback accept failed"),
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn handle_callback_stream(mut stream: TcpStream, expected_state: &str) -> Result<String> {
    // BSD sockets (macOS) hand the accepted stream the listener's O_NONBLOCK;
    // the bounded read below needs a blocking socket with a timeout.
    stream
        .set_nonblocking(false)
        .context("ChatGPT OAuth callback stream could not be set blocking")?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    // One read is not one request: TCP may deliver the callback in
    // fragments, and a truncated query parses as a missing parameter.
    // Read until the blank line that ends the HTTP headers.
    let mut buf = [0u8; 4096];
    let mut len = 0usize;
    loop {
        if len == buf.len() {
            break;
        }
        let n = stream
            .read(&mut buf[len..])
            .context("reading ChatGPT OAuth callback request")?;
        if n == 0 {
            break;
        }
        len += n;
        if buf[..len].windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let request = String::from_utf8_lossy(&buf[..len]);
    let request_line = request.lines().next().unwrap_or_default();
    let result = (|| {
        let target = parse_http_request_target(request_line)?;
        let query = query_from_target(&target)?;
        let outcome = parse_callback_query(query)?;
        accept_callback(expected_state, outcome)
    })();
    let (status, body) = match &result {
        Ok(_) => ("200 OK", CALLBACK_HTML_OK),
        Err(_) => ("400 Bad Request", CALLBACK_HTML_ERR),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    result
}

fn exchange_code(
    client: &dyn TokenClient,
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> Result<TokenResponse> {
    let (status, body) = client.post_form(
        &token_endpoint(issuer),
        &[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code", code),
            ("code_verifier", verifier),
        ],
    )?;
    parse_token_response(status, &body, "authorization code exchange")
}

fn refresh_access_token(
    client: &dyn TokenClient,
    issuer: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse> {
    #[cfg(test)]
    crate::external_credentials::record_oauth_refresh();
    let (status, body) = client.post_form(
        &token_endpoint(issuer),
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ],
    )?;
    parse_token_response(status, &body, "refresh")
}

fn revoke_remote_token(
    client: &dyn TokenClient,
    issuer: &str,
    client_id: &str,
    token: &str,
) -> Result<()> {
    let (status, body) = client.post_form(
        &revoke_endpoint(issuer),
        &[("token", token), ("client_id", client_id)],
    )?;
    if !(200..300).contains(&status) {
        bail!(
            "ChatGPT OAuth revoke failed with HTTP {status}: {}",
            compact_error(&body)
        );
    }
    Ok(())
}

fn parse_token_response(status: u16, body: &str, operation: &str) -> Result<TokenResponse> {
    let parsed: TokenResponse = serde_json::from_str(body).map_err(|_| {
        anyhow::anyhow!("{operation} returned HTTP {status} that was not token JSON")
    })?;
    if !(200..300).contains(&status) || parsed.error.is_some() {
        let err = parsed.error.as_deref().unwrap_or("token_error");
        if matches!(
            err,
            "invalid_grant"
                | "refresh_token_reused"
                | "refresh_token_expired"
                | "refresh_token_invalidated"
        ) || status == 401
        {
            bail!(
                "ChatGPT OAuth {operation} failed permanently ({err}). Sign in again with `codewhale auth chatgpt`."
            );
        }
        bail!("ChatGPT OAuth {operation} failed ({err})");
    }
    anyhow::ensure!(
        parsed
            .access_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty()),
        "ChatGPT OAuth {operation} returned an empty access token"
    );
    Ok(parsed)
}

fn compact_error(body: &str) -> String {
    body.chars().filter(|c| !c.is_control()).take(80).collect()
}

/// Interactive PKCE login. Prints the authorize URL, opens a browser, and
/// waits for the localhost callback. The bearer remains pending until
/// [`activate_pkce_login`].
pub async fn pkce_login() -> Result<PendingChatgptPkceLogin> {
    let issuer = std::env::var("CODEWHALE_CHATGPT_OAUTH_ISSUER")
        .unwrap_or_else(|_| CHATGPT_OAUTH_ISSUER.to_string());
    let client_id = std::env::var("CODEWHALE_CHATGPT_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| CHATGPT_OAUTH_CLIENT_ID.to_string());
    let open_browser = std::env::var_os("CODEWHALE_CHATGPT_OAUTH_NO_BROWSER").is_none();
    tokio::task::spawn_blocking(move || pkce_login_with(&issuer, &client_id, open_browser))
        .await
        .context("ChatGPT PKCE login worker failed")?
}

fn pkce_login_with(
    issuer: &str,
    client_id: &str,
    open_browser: bool,
) -> Result<PendingChatgptPkceLogin> {
    let listeners = bind_loopback_callback()?;
    let request = start_auth_request_on(&listeners, issuer, client_id)?;
    eprintln!("ChatGPT sign-in (PKCE)");
    eprintln!("  Open:  {}", request.authorize_url);
    eprintln!("Waiting for the browser callback… (Ctrl+C to abort)");
    if open_browser && let Err(err) = webbrowser::open(&request.authorize_url) {
        eprintln!("Could not open the browser automatically: {err}");
    }
    let code = wait_for_callback(&listeners, &request.state)?;
    let token = exchange_code(
        &ReqwestTokenClient,
        issuer,
        client_id,
        &request.redirect_uri,
        &code,
        &request.pkce.verifier,
    )?;
    Ok(PendingChatgptPkceLogin {
        issuer: issuer.to_string(),
        client_id: client_id.to_string(),
        token,
    })
}

/// Commit a pending PKCE login as a uniquely named owned generation and
/// point `[providers.openai_codex]` at it under the shared config lock.
pub fn activate_pkce_login(
    pending: PendingChatgptPkceLogin,
    config_path: Option<&Path>,
    live_config: Option<&mut Config>,
) -> Result<ChatgptPkceActivation> {
    codewhale_config::with_xai_oauth_lifecycle_lock(move |store| {
        activate_pkce_login_locked(pending, config_path, live_config, store)
    })
}

fn activate_pkce_login_locked(
    pending: PendingChatgptPkceLogin,
    config_path: Option<&Path>,
    live_config: Option<&mut Config>,
    store: &codewhale_config::XaiOAuthCredentialStore,
) -> Result<ChatgptPkceActivation> {
    let config_path = crate::config_persistence::config_toml_path(config_path)?;
    let generation = format!(
        "{}{}{}",
        codewhale_config::CHATGPT_OAUTH_GENERATION_PREFIX,
        uuid::Uuid::new_v4().simple(),
        codewhale_config::CHATGPT_OAUTH_GENERATION_SUFFIX
    );
    codewhale_config::validate_chatgpt_oauth_generation(&generation)?;
    let auth_path = store.path_for(&generation)?;
    let key_inside = crate::config::provider_config_key(ApiProvider::OpenaiCodex)
        .context("openai-codex auth mode key")?;
    let mut stage_written = false;

    let activation = codewhale_config::mutate_config_document(&config_path, |document| {
        let previous_generation_item = document
            .get("providers")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|providers| providers.get(key_inside))
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|provider| provider.get("oauth_credential_generation"));
        let previous_generation = previous_generation_item
            .map(|item| {
                item.as_str()
                    .context(
                        "refusing ChatGPT login because the existing credential generation pointer is not a string",
                    )
                    .map(ToOwned::to_owned)
            })
            .transpose()?;
        if let Some(previous) = previous_generation.as_deref() {
            codewhale_config::validate_chatgpt_oauth_generation(previous).with_context(|| {
                "refusing ChatGPT login because the existing credential generation pointer is invalid"
            })?;
        }
        let previous_owned_name = match previous_generation.as_deref() {
            Some(previous) => Some(previous.to_string()),
            None if store
                .read_to_string(codewhale_config::LEGACY_CHATGPT_OAUTH_FILE_NAME)?
                .is_some() =>
            {
                Some(codewhale_config::LEGACY_CHATGPT_OAUTH_FILE_NAME.to_string())
            }
            None => None,
        };
        let mut file = BTreeMap::new();
        let scope = format!("{}::{}", pending.issuer, pending.client_id);
        let mut entry = ChatgptAuthEntry {
            access_token: None,
            refresh_token: None,
            expires_at: None,
            id_token: None,
            account_id: None,
            oidc_issuer: Some(pending.issuer.clone()),
            oidc_client_id: Some(pending.client_id.clone()),
            originator: Some(CHATGPT_OAUTH_ORIGINATOR.to_string()),
            extra: BTreeMap::new(),
        };
        apply_token_response(
            &mut entry,
            &pending.issuer,
            &pending.client_id,
            &pending.token,
        )?;
        let access = entry
            .access_token
            .clone()
            .filter(|token| !token.trim().is_empty())
            .context("ChatGPT PKCE login returned an empty access token")?;
        file.insert(scope.clone(), entry.clone());
        write_auth_file_to_store(store, &generation, &file, false)?;
        stage_written = true;

        codewhale_config::set_config_document_value(
            document,
            &["providers", key_inside, "auth_mode"],
            "oauth",
        )?;
        codewhale_config::set_config_document_value(
            document,
            &["providers", key_inside, "oauth_credential_generation"],
            generation.clone(),
        )?;
        codewhale_config::unset_config_document_value(
            document,
            &["providers", key_inside, "external_credentials"],
        )?;
        Ok((previous_owned_name, credentials_from_entry(&entry, access)))
    });

    let (previous_owned_name, credentials) = match activation {
        Ok(activation) => activation,
        Err(error) => {
            if stage_written && let Err(cleanup_error) = store.remove(&generation) {
                return Err(error).context(format!(
                    "ChatGPT login was not activated; also failed to remove unreferenced staged credentials at {}: {cleanup_error}",
                    codewhale_config::quote_os_path(&auth_path)
                ));
            }
            return Err(error)
                .context("ChatGPT login was not activated; provider configuration is unchanged");
        }
    };

    if let Some(config) = live_config {
        config.mark_codewhale_owned_chatgpt_oauth(generation.clone());
    }
    if let Some(previous) = previous_owned_name
        && previous != generation
        && let Err(error) = store.remove(&previous)
    {
        tracing::warn!(
            target: "codewhale::chatgpt_oauth",
            error = %error,
            "new ChatGPT OAuth generation committed but superseded generation cleanup failed"
        );
    }
    eprintln!(
        "Signed in with ChatGPT. Codewhale-owned credentials activated at {}.",
        codewhale_config::quote_os_path(&auth_path)
    );
    Ok(ChatgptPkceActivation {
        credentials,
        config_path,
        auth_path,
    })
}

/// Remove Codewhale-owned ChatGPT tokens and the config pointer.
///
/// Remote revoke is best-effort against the fixed
/// `{issuer}/api/accounts/oauth/revoke` (see [`revoke_endpoint`]), not a
/// discovery lookup: a failed or unreachable revoke must never stop the local
/// credentials from being removed. External Codex CLI consent is left
/// untouched.
pub fn revoke_owned_login(
    config_path: Option<&Path>,
    live_config: Option<&mut Config>,
) -> Result<()> {
    codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
        revoke_owned_login_locked(config_path, live_config, store, &ReqwestTokenClient)
    })
}

fn revoke_owned_login_locked(
    config_path: Option<&Path>,
    live_config: Option<&mut Config>,
    store: &codewhale_config::XaiOAuthCredentialStore,
    client: &dyn TokenClient,
) -> Result<()> {
    let config_path = crate::config_persistence::config_toml_path(config_path)?;
    let key_inside = crate::config::provider_config_key(ApiProvider::OpenaiCodex)
        .context("openai-codex auth mode key")?;
    let previous = codewhale_config::mutate_config_document(&config_path, |document| {
        let previous = document
            .get("providers")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|providers| providers.get(key_inside))
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|provider| provider.get("oauth_credential_generation"))
            .and_then(toml_edit::Item::as_str)
            .map(ToOwned::to_owned);
        codewhale_config::unset_config_document_value(
            document,
            &["providers", key_inside, "oauth_credential_generation"],
        )?;
        let auth_mode_is_oauth = document
            .get("providers")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|providers| providers.get(key_inside))
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|provider| provider.get("auth_mode"))
            .and_then(toml_edit::Item::as_str)
            == Some("oauth");
        if auth_mode_is_oauth {
            codewhale_config::unset_config_document_value(
                document,
                &["providers", key_inside, "auth_mode"],
            )?;
        }
        Ok(previous)
    })?;
    if let Some(config) = live_config {
        config.clear_codewhale_owned_chatgpt_oauth();
    }
    let names = match previous.as_deref() {
        Some(generation) if codewhale_config::is_valid_chatgpt_oauth_generation(generation) => {
            vec![generation.to_string()]
        }
        _ => vec![codewhale_config::LEGACY_CHATGPT_OAUTH_FILE_NAME.to_string()],
    };
    for name in names {
        if let Ok(Some(raw)) = store.read_to_string(&name)
            && let Ok(file) = parse_auth_file(&raw, &store.path_for(&name)?)
        {
            for entry in file.values() {
                if let Some(token) = entry
                    .refresh_token
                    .as_deref()
                    .or(entry.access_token.as_deref())
                    .filter(|token| !token.trim().is_empty())
                {
                    let issuer = entry.oidc_issuer.as_deref().unwrap_or(CHATGPT_OAUTH_ISSUER);
                    let client_id = entry
                        .oidc_client_id
                        .as_deref()
                        .unwrap_or(CHATGPT_OAUTH_CLIENT_ID);
                    if let Err(error) = revoke_remote_token(client, issuer, client_id, token) {
                        tracing::warn!(
                            target: "codewhale::chatgpt_oauth",
                            error = %error,
                            "ChatGPT OAuth remote revoke failed; local credentials will still be removed"
                        );
                    }
                }
            }
        }
        let _ = store.remove(&name);
    }
    Ok(())
}

#[must_use]
pub fn credentials_present(config: &Config) -> bool {
    credentials_valid(config)
}

#[must_use]
pub fn credentials_valid(config: &Config) -> bool {
    if let Ok(Some(path)) = configured_owned_auth_file_path(config)
        && let Ok(Some(mut file)) = load_owned_auth_file(&path)
        && let Some((_, entry)) = select_entry(&mut file)
        && (entry_access_token_is_fresh(&entry)
            || entry
                .refresh_token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty()))
    {
        return true;
    }
    false
}

fn configured_owned_auth_file_path(config: &Config) -> Result<Option<PathBuf>> {
    let generation = config
        .provider_config_for(ApiProvider::OpenaiCodex)
        .and_then(|entry| entry.oauth_credential_generation.as_deref());
    match generation {
        Some(generation) => codewhale_config::chatgpt_oauth_generation_path(generation).map(Some),
        None => Ok(None),
    }
}

pub fn get_owned_credentials(config: &Config) -> Result<ChatgptOAuthCredentials> {
    get_owned_credentials_with(config, &ReqwestTokenClient)
}

fn get_owned_credentials_with(
    config: &Config,
    client: &dyn TokenClient,
) -> Result<ChatgptOAuthCredentials> {
    let Some(path) = configured_owned_auth_file_path(config)? else {
        bail!("Codewhale-owned ChatGPT OAuth credentials are not configured");
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Codewhale-owned ChatGPT OAuth path must have a UTF-8 basename")?;
    codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
        get_owned_credentials_locked(store, name, |issuer, client_id, refresh| {
            refresh_access_token(client, issuer, client_id, refresh)
        })
    })
}

fn get_owned_credentials_locked<F>(
    store: &codewhale_config::XaiOAuthCredentialStore,
    name: &str,
    refresh_access: F,
) -> Result<ChatgptOAuthCredentials>
where
    F: FnOnce(&str, &str, &str) -> Result<TokenResponse>,
{
    let path = store.path_for(name)?;
    let mut file = load_owned_auth_file_from_store(store, name)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Codewhale-owned ChatGPT OAuth credentials were not found at {}. Run `codewhale auth chatgpt` again.",
            codewhale_config::quote_os_path(&path)
        )
    })?;
    let (scope, mut entry) = select_entry(&mut file).ok_or_else(|| {
        anyhow::anyhow!(
            "Codewhale-owned ChatGPT OAuth credentials at {} have no usable entry. Run `codewhale auth chatgpt` again.",
            codewhale_config::quote_os_path(&path)
        )
    })?;

    if entry_access_token_is_fresh(&entry) {
        let token = entry
            .access_token
            .clone()
            .filter(|t| !t.trim().is_empty())
            .context("ChatGPT OAuth access token is empty")?;
        return Ok(credentials_from_entry(&entry, token));
    }

    let refresh = entry
        .refresh_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .context(
            "ChatGPT OAuth access token expired and no refresh_token is stored. \
             Run `codewhale auth chatgpt` again.",
        )?;
    let issuer = entry
        .oidc_issuer
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| issuer_from_scope(&scope));
    let client_id = entry
        .oidc_client_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| client_id_from_scope(&scope));

    let refreshed = refresh_access(&issuer, &client_id, refresh)?;
    apply_token_response(&mut entry, &issuer, &client_id, &refreshed)?;
    file.insert(scope.clone(), entry.clone());
    write_auth_file_to_store(store, name, &file, true)?;

    let token = entry
        .access_token
        .clone()
        .filter(|t| !t.trim().is_empty())
        .context("ChatGPT OAuth refresh returned an empty access token")?;
    Ok(credentials_from_entry(&entry, token))
}

#[must_use]
pub fn missing_auth_message() -> String {
    format!(
        "OpenAI Codex OAuth credentials are unavailable.\n\
         \n\
         Sign in with ChatGPT (subscription billing, Codewhale-owned tokens):\n\
         `codewhale auth chatgpt` or /provider setup openai-codex.\n\
         The openai API-key route is a different billing owner.\n\
         \n\
         Alternatives:\n\
         - Process token: OPENAI_CODEX_ACCESS_TOKEN / CODEX_ACCESS_TOKEN\n\
         - Explicit Codex CLI import (not a prerequisite): after `codex login`, run \
         `codewhale auth external-consent --provider openai-codex --mode read-only --path {}`\n\
         Read-only access never refreshes or rewrites the Codex CLI file.\n\
         Revoke Codewhale-owned tokens with `codewhale auth chatgpt-revoke`.",
        codewhale_config::quote_os_path(&crate::oauth::auth_file_path())
    )
}

type AuthFile = BTreeMap<String, ChatgptAuthEntry>;

fn load_owned_auth_file(path: &Path) -> Result<Option<AuthFile>> {
    let Some(raw) = crate::external_credentials::read_codewhale_owned_to_string(path)? else {
        return Ok(None);
    };
    parse_auth_file(&raw, path).map(Some)
}

fn load_owned_auth_file_from_store(
    store: &codewhale_config::XaiOAuthCredentialStore,
    name: &str,
) -> Result<Option<AuthFile>> {
    let Some(raw) = store.read_to_string(name)? else {
        return Ok(None);
    };
    parse_auth_file(&raw, &store.path_for(name)?).map(Some)
}

fn parse_auth_file(raw: &str, path: &Path) -> Result<AuthFile> {
    let value: Value = serde_json::from_str(raw).map_err(|_| {
        anyhow::anyhow!(
            "ChatGPT credential file {} is not valid credential JSON",
            codewhale_config::quote_os_path(path)
        )
    })?;
    let obj = value.as_object().ok_or_else(|| {
        anyhow::anyhow!(
            "ChatGPT credential file {} must be a JSON object of entries",
            codewhale_config::quote_os_path(path)
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        match serde_json::from_value::<ChatgptAuthEntry>(v.clone()) {
            Ok(entry) => {
                out.insert(k.clone(), entry);
            }
            Err(_) => {
                tracing::warn!(
                    target: "codewhale::chatgpt_oauth",
                    "skipping unreadable ChatGPT auth entry"
                );
            }
        }
    }
    Ok(out)
}

fn write_auth_file_to_store(
    store: &codewhale_config::XaiOAuthCredentialStore,
    name: &str,
    file: &AuthFile,
    allow_replace: bool,
) -> Result<()> {
    let serialized =
        serde_json::to_vec_pretty(file).context("serializing ChatGPT OAuth credentials")?;
    store
        .write(name, &serialized, allow_replace)
        .with_context(|| {
            format!(
                "writing ChatGPT OAuth credentials to {}",
                codewhale_config::quote_os_path(&store.directory().join(name))
            )
        })?;
    #[cfg(test)]
    crate::external_credentials::record_owned_credential_write();
    Ok(())
}

fn select_entry(file: &mut AuthFile) -> Option<(String, ChatgptAuthEntry)> {
    let preferred_suffix = format!("::{CHATGPT_OAUTH_CLIENT_ID}");
    if let Some((k, v)) = file
        .iter()
        .find(|(k, e)| k.ends_with(&preferred_suffix) && entry_has_usable_secret(e))
    {
        return Some((k.clone(), v.clone()));
    }
    file.iter()
        .find(|(_, e)| entry_has_usable_secret(e))
        .map(|(k, v)| (k.clone(), v.clone()))
}

fn entry_has_usable_secret(entry: &ChatgptAuthEntry) -> bool {
    entry
        .access_token
        .as_deref()
        .is_some_and(|t| !t.trim().is_empty())
        || entry
            .refresh_token
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
}

fn entry_access_token_is_fresh(entry: &ChatgptAuthEntry) -> bool {
    let Some(token) = entry
        .access_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    else {
        return false;
    };
    if let Some(exp) = entry.expires_at.as_deref().and_then(parse_rfc3339_secs) {
        let now = now_unix_secs().unwrap_or(0);
        return exp - now > REFRESH_SKEW_SECS;
    }
    match jwt_expiry_seconds(token) {
        Some(exp) => {
            let now = now_unix_secs().unwrap_or(0) as u64;
            (exp as i64) - (now as i64) > REFRESH_SKEW_SECS
        }
        None => false,
    }
}

fn credentials_from_entry(
    entry: &ChatgptAuthEntry,
    access_token: String,
) -> ChatgptOAuthCredentials {
    ChatgptOAuthCredentials {
        access_token,
        account_id: entry.account_id.clone(),
        refresh_token: entry.refresh_token.clone(),
        expires_at: entry.expires_at.clone(),
    }
}

fn issuer_from_scope(scope: &str) -> String {
    scope
        .split_once("::")
        .map(|(issuer, _)| issuer.to_string())
        .unwrap_or_else(|| CHATGPT_OAUTH_ISSUER.to_string())
}

fn client_id_from_scope(scope: &str) -> String {
    scope
        .split_once("::")
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| CHATGPT_OAUTH_CLIENT_ID.to_string())
}

fn apply_token_response(
    entry: &mut ChatgptAuthEntry,
    issuer: &str,
    client_id: &str,
    token: &TokenResponse,
) -> Result<()> {
    let access = token
        .access_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .context("token response missing access_token")?;
    entry.access_token = Some(access.to_string());
    if let Some(rt) = token
        .refresh_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        entry.refresh_token = Some(rt.to_string());
    }
    entry.oidc_issuer = Some(issuer.to_string());
    entry.oidc_client_id = Some(client_id.to_string());
    entry.originator = Some(CHATGPT_OAUTH_ORIGINATOR.to_string());
    if let Some(id_token) = token.id_token.clone() {
        if let Some(account_id) = account_id_from_id_token(&id_token) {
            entry.account_id = Some(account_id);
        }
        entry.id_token = Some(id_token);
    }
    if let Some(expires_in) = token.expires_in {
        entry.expires_at = Some(rfc3339_from_now(expires_in));
    } else if let Some(exp) = jwt_expiry_seconds(access) {
        entry.expires_at = Some(rfc3339_from_unix(exp as i64));
    }
    Ok(())
}

fn jwt_expiry_seconds(token: &str) -> Option<u64> {
    jwt_payload(token)?.get("exp")?.as_u64()
}

fn account_id_from_id_token(token: &str) -> Option<String> {
    let payload = jwt_payload(token)?;
    if let Some(id) = payload.get("chatgpt_account_id").and_then(Value::as_str) {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    payload
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn now_unix_secs() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
}

fn parse_rfc3339_secs(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp())
}

fn rfc3339_from_now(expires_in: u64) -> String {
    let ts = now_unix_secs().unwrap_or(0) + expires_in as i64;
    rfc3339_from_unix(ts)
}

fn rfc3339_from_unix(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| format!("{ts}"))
}

#[cfg(test)]
pub(crate) fn pending_pkce_login_for_test(
    access_token: &str,
    refresh_token: &str,
    id_token: Option<&str>,
) -> PendingChatgptPkceLogin {
    PendingChatgptPkceLogin {
        issuer: CHATGPT_OAUTH_ISSUER.to_string(),
        client_id: CHATGPT_OAUTH_CLIENT_ID.to_string(),
        token: TokenResponse {
            access_token: Some(access_token.to_string()),
            refresh_token: Some(refresh_token.to_string()),
            expires_in: Some(3600),
            id_token: id_token.map(ToOwned::to_owned),
            error: None,
            error_description: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type MockForm = Vec<(String, String)>;
    type MockPost = (String, MockForm);

    struct MockTokenClient {
        responses: Mutex<Vec<(u16, String)>>,
        posts: Mutex<Vec<MockPost>>,
    }

    impl MockTokenClient {
        fn new(responses: Vec<(u16, String)>) -> Self {
            Self {
                responses: Mutex::new(responses),
                posts: Mutex::new(Vec::new()),
            }
        }
    }

    impl TokenClient for MockTokenClient {
        fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String)> {
            self.posts.lock().expect("posts").push((
                url.to_string(),
                form.iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ));
            let mut responses = self.responses.lock().expect("responses");
            anyhow::ensure!(
                !responses.is_empty(),
                "mock issuer has no remaining responses"
            );
            Ok(responses.remove(0))
        }
    }

    fn jwt_with_exp(exp: u64) -> String {
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("header.{payload}.sig")
    }

    fn jwt_with_account(account: &str) -> String {
        let payload = URL_SAFE_NO_PAD.encode(format!(
            r#"{{"https://api.openai.com/auth":{{"chatgpt_account_id":"{account}"}}}}"#
        ));
        format!("header.{payload}.sig")
    }

    #[test]
    fn pkce_verifier_and_challenge_are_s256() {
        let pkce = generate_pkce();
        assert!(pkce.verifier.len() >= 43);
        assert_eq!(
            pkce.challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()))
        );
        let other = generate_pkce();
        assert_ne!(pkce.verifier, other.verifier);
        assert_ne!(generate_state(), generate_state());
    }

    #[test]
    fn malformed_issuer_fails_loudly_not_to_production() {
        let pkce = PkceChallenge {
            verifier: "verifier".into(),
            challenge: "challenge".into(),
        };
        let err = build_authorize_url(
            "not a url \\ ",
            CHATGPT_OAUTH_CLIENT_ID,
            "http://localhost:1455/auth/callback",
            "state-1",
            &pkce,
        )
        .expect_err("malformed issuer must not produce an authorize URL");
        assert!(format!("{err:#}").contains("CODEWHALE_CHATGPT_OAUTH_ISSUER"));
    }

    #[test]
    fn authorize_url_is_honest_originator_and_pkce() {
        let pkce = PkceChallenge {
            verifier: "verifier".into(),
            challenge: "challenge".into(),
        };
        let url = build_authorize_url(
            CHATGPT_OAUTH_ISSUER,
            CHATGPT_OAUTH_CLIENT_ID,
            "http://localhost:1455/auth/callback",
            "state-1",
            &pkce,
        );
        let url = url.expect("static issuer parses");
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("originator=codewhale"));
        assert!(!url.contains("codex_cli_rs"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        assert!(url.contains("id_token_add_organizations=true"));
    }

    #[test]
    fn callback_success_requires_matching_state() {
        let ok = parse_callback_query("code=abc&state=s1").unwrap();
        assert_eq!(accept_callback("s1", ok).unwrap(), "abc");
        let mismatch = parse_callback_query("code=abc&state=other").unwrap();
        let err = accept_callback("s1", mismatch).unwrap_err().to_string();
        assert!(err.contains("state did not match"), "{err}");
    }

    #[test]
    fn callback_error_is_user_visible_without_code() {
        let outcome =
            parse_callback_query("error=access_denied&error_description=nope&state=s1").unwrap();
        let err = accept_callback("s1", outcome).unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
        assert!(!err.contains("access_token"));
    }

    #[test]
    fn callback_missing_code_fails() {
        let err = parse_callback_query("state=s1").unwrap_err().to_string();
        assert!(err.contains("missing authorization code"), "{err}");
    }

    #[test]
    fn token_exchange_uses_pkce_verifier_against_mock_issuer() {
        let client = MockTokenClient::new(vec![(
            200,
            serde_json::json!({
                "access_token": "at-1",
                "refresh_token": "rt-1",
                "expires_in": 3600,
                "id_token": jwt_with_account("acct-9")
            })
            .to_string(),
        )]);
        let token = exchange_code(
            &client,
            CHATGPT_OAUTH_ISSUER,
            CHATGPT_OAUTH_CLIENT_ID,
            "http://localhost:1455/auth/callback",
            "auth-code",
            "verifier",
        )
        .unwrap();
        assert_eq!(token.access_token.as_deref(), Some("at-1"));
        let posts = client.posts.lock().unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].0, "https://auth.openai.com/oauth/token");
        let form: BTreeMap<_, _> = posts[0].1.iter().cloned().collect();
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["code_verifier"], "verifier");
        assert_eq!(form["code"], "auth-code");
    }

    #[test]
    fn token_exchange_error_does_not_echo_body_secrets() {
        let client = MockTokenClient::new(vec![(
            400,
            serde_json::json!({
                "error": "invalid_grant",
                "error_description": "secret-must-not-leak"
            })
            .to_string(),
        )]);
        let err = exchange_code(
            &client,
            CHATGPT_OAUTH_ISSUER,
            CHATGPT_OAUTH_CLIENT_ID,
            "http://localhost:1455/auth/callback",
            "bad",
            "verifier",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("permanently"), "{err}");
        assert!(!err.contains("secret-must-not-leak"), "{err}");
    }

    #[test]
    fn store_persist_refresh_and_revoke_use_mock_issuer() {
        let _lock = crate::test_support::lock_test_env();
        let home = tempfile::tempdir().expect("temp home");
        let root = home.path().canonicalize().expect("canonical home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &root);
        let config_path = root.join("config.toml");
        std::fs::write(&config_path, "").expect("empty config");

        let pending =
            pending_pkce_login_for_test("access-1", "refresh-1", Some(&jwt_with_account("acct-7")));
        let activation = activate_pkce_login(pending, Some(&config_path), None).expect("activate");
        assert!(activation.auth_path.exists());
        let persisted = std::fs::read_to_string(root.join("config.toml")).expect("config");
        assert!(persisted.contains("chatgpt-auth-"));
        assert!(persisted.contains("auth_mode = \"oauth\""));
        assert!(!persisted.contains("access-1"), "{persisted}");

        let generation = toml::from_str::<toml::Value>(&persisted)
            .unwrap()["providers"]["openai_codex"]["oauth_credential_generation"]
            .as_str()
            .unwrap()
            .to_string();
        let mut config = Config {
            provider: Some(ApiProvider::OpenaiCodex.as_str().to_string()),
            ..Config::default()
        };
        config.mark_codewhale_owned_chatgpt_oauth(generation.clone());
        assert!(credentials_valid(&config));

        let stale = jwt_with_exp(1_000_000_000);
        let scope = format!("{CHATGPT_OAUTH_ISSUER}::{CHATGPT_OAUTH_CLIENT_ID}");
        let raw = serde_json::json!({
            &scope: {
                "access_token": stale,
                "refresh_token": "refresh-old",
                "expires_at": "2000-01-01T00:00:00Z",
                "oidc_issuer": CHATGPT_OAUTH_ISSUER,
                "oidc_client_id": CHATGPT_OAUTH_CLIENT_ID,
                "originator": CHATGPT_OAUTH_ORIGINATOR
            }
        });
        codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
            store.write(
                &generation,
                serde_json::to_vec_pretty(&raw).unwrap().as_slice(),
                true,
            )
        })
        .unwrap();

        let mock = MockTokenClient::new(vec![(
            200,
            serde_json::json!({
                "access_token": "access-2",
                "refresh_token": "refresh-2",
                "expires_in": 3600
            })
            .to_string(),
        )]);
        let refreshed = get_owned_credentials_with(&config, &mock).expect("refresh");
        assert_eq!(refreshed.access_token, "access-2");
        let stored = codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
            store.read_to_string(&generation)
        })
        .unwrap()
        .unwrap();
        assert!(stored.contains("refresh-2"), "{stored}");
        assert!(!stored.contains("refresh-old"), "{stored}");

        let revoke_mock = MockTokenClient::new(vec![(200, String::new())]);
        codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
            revoke_owned_login_locked(Some(&config_path), None, store, &revoke_mock)
        })
        .expect("revoke");
        let after = std::fs::read_to_string(&config_path).expect("config after revoke");
        assert!(!after.contains("chatgpt-auth-"), "{after}");
        let posts = revoke_mock.posts.lock().unwrap();
        assert!(
            posts.iter().any(|(url, _)| url.contains("/oauth/revoke")),
            "{posts:?}"
        );
    }

    #[test]
    fn callback_server_handles_success_and_error_requests() {
        use std::io::Write as _;
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test port");
        listener.set_nonblocking(false).unwrap();
        let addr = listener.local_addr().unwrap();
        let state = "state-xyz".to_string();
        let expected = state.clone();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_callback_stream(stream, &expected)
        });
        let mut client = std::net::TcpStream::connect(addr).expect("connect");
        write!(
            client,
            "GET /auth/callback?code=tok&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
        )
        .unwrap();
        let code = server.join().expect("server").expect("callback ok");
        assert_eq!(code, "tok");

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind error port");
        listener.set_nonblocking(false).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_callback_stream(stream, "state-xyz")
        });
        let mut client = std::net::TcpStream::connect(addr).expect("connect");
        write!(
            client,
            "GET /auth/callback?error=access_denied&state=state-xyz HTTP/1.1\r\nHost: localhost\r\n\r\n"
        )
        .unwrap();
        let err = server.join().expect("server").unwrap_err().to_string();
        assert!(err.contains("not completed"), "{err}");
    }

    /// The registered redirect URI says `localhost`, which resolves to `::1`
    /// as readily as `127.0.0.1`. A callback arriving on the IPv6 listener has
    /// to be accepted, or an IPv6-first browser hangs until the timeout.
    #[test]
    fn callback_is_accepted_on_either_loopback_family() {
        use std::io::Write as _;
        for addr in [
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, 0)),
        ] {
            let Ok(target) = TcpListener::bind(addr) else {
                // A host without this stack cannot exercise it; the other arm
                // still covers the polling loop.
                continue;
            };
            target.set_nonblocking(true).unwrap();
            let target_addr = target.local_addr().unwrap();

            // A second, permanently idle listener stands in for the family the
            // browser did not pick: `wait_for_callback` must poll past it.
            let idle = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .expect("bind idle listener");
            idle.set_nonblocking(true).unwrap();

            let listeners = vec![idle, target];
            let server = std::thread::spawn(move || wait_for_callback(&listeners, "state-xyz"));
            let mut client = std::net::TcpStream::connect(target_addr).expect("connect");
            write!(
                client,
                "GET /auth/callback?code=tok&state=state-xyz HTTP/1.1\r\nHost: localhost\r\n\r\n"
            )
            .unwrap();
            let code = server
                .join()
                .expect("server")
                .unwrap_or_else(|error| panic!("callback on {target_addr} rejected: {error}"));
            assert_eq!(code, "tok", "callback on {target_addr}");
        }
    }

    #[test]
    fn debug_impls_redact_secrets() {
        let token = TokenResponse {
            access_token: Some("secret-access".into()),
            refresh_token: Some("secret-refresh".into()),
            expires_in: Some(1),
            id_token: Some("secret-id".into()),
            error: None,
            error_description: None,
        };
        let rendered = format!("{token:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("secret-access"));
        assert!(!rendered.contains("secret-refresh"));
    }
}
