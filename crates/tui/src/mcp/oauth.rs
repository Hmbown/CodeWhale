use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oauth2::TokenResponse;
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rmcp::transport::AuthorizationManager;
use rmcp::transport::AuthorizationSession;
use rmcp::transport::auth::{AuthError, OAuthClientConfig, OAuthState, OAuthTokenResponse};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tiny_http::{Response, Server};
use tokio::sync::{Mutex, oneshot};
use tokio::time::timeout;
use urlencoding::decode;

use super::McpServerConfig;

const REFRESH_SKEW_MILLIS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthStatus {
    Unsupported,
    NotLoggedIn,
    BearerToken,
    OAuth,
}

impl std::fmt::Display for McpAuthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Unsupported => "Unsupported",
            Self::NotLoggedIn => "Not logged in",
            Self::BearerToken => "Bearer token",
            Self::OAuth => "OAuth",
        };
        f.write_str(text)
    }
}

pub fn error_looks_auth_required(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_ascii_lowercase();
    text.contains("401")
        || text.contains("unauthorized")
        || text.contains("authentication_required")
        || text.contains("not logged in")
        || text.contains("not-logged-in")
}

pub fn auth_required_login_hint(server_name: &str) -> String {
    format!(
        "MCP server '{server_name}' requires OAuth authentication. Run `codewhale mcp login {server_name}` to authenticate."
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredMcpOAuthTokens {
    pub server_name: String,
    pub url: String,
    pub client_id: String,
    pub token_response: WrappedOAuthTokenResponse,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedOAuthTokenResponse(pub OAuthTokenResponse);

impl PartialEq for WrappedOAuthTokenResponse {
    fn eq(&self, other: &Self) -> bool {
        match (serde_json::to_string(self), serde_json::to_string(other)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }
}

#[derive(Clone)]
pub struct McpOAuthRuntime {
    inner: Arc<McpOAuthRuntimeInner>,
}

struct McpOAuthRuntimeInner {
    server_name: String,
    url: String,
    manager: Arc<Mutex<AuthorizationManager>>,
    last_tokens: Mutex<Option<StoredMcpOAuthTokens>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthDiscovery {
    pub scopes_supported: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMcpOAuthScopes {
    pub scopes: Vec<String>,
    pub source: McpOAuthScopesSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpOAuthScopesSource {
    Explicit,
    Configured,
    Discovered,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProviderError {
    error: Option<String>,
    error_description: Option<String>,
}

impl OAuthProviderError {
    fn new(error: Option<String>, error_description: Option<String>) -> Self {
        Self {
            error,
            error_description,
        }
    }
}

impl std::fmt::Display for OAuthProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.error.as_deref(), self.error_description.as_deref()) {
            (Some(error), Some(description)) => {
                write!(f, "OAuth provider returned `{error}`: {description}")
            }
            (Some(error), None) => write!(f, "OAuth provider returned `{error}`"),
            (None, Some(description)) => write!(f, "OAuth error: {description}"),
            (None, None) => write!(f, "OAuth provider returned an error"),
        }
    }
}

impl std::error::Error for OAuthProviderError {}

impl McpOAuthRuntime {
    pub async fn from_server_config(
        server_name: &str,
        server: &McpServerConfig,
        default_headers: HeaderMap,
    ) -> Result<Option<Self>> {
        let Some(url) = server.url.as_deref() else {
            return Ok(None);
        };
        if server_has_manual_authorization(server) {
            return Ok(None);
        }
        let Some(tokens) = load_oauth_tokens(server_name, url)? else {
            return Ok(None);
        };
        Self::from_stored_tokens(server_name, url, tokens, default_headers)
            .await
            .map(Some)
    }

    async fn from_stored_tokens(
        server_name: &str,
        url: &str,
        mut tokens: StoredMcpOAuthTokens,
        default_headers: HeaderMap,
    ) -> Result<Self> {
        refresh_expires_in_from_timestamp(&mut tokens);
        let client = apply_default_headers(crate::tls::reqwest_client_builder(), &default_headers)
            .build()
            .context("building MCP OAuth metadata client")?;
        let mut state = OAuthState::new(url.to_string(), Some(client)).await?;
        state
            .set_credentials(&tokens.client_id, tokens.token_response.0.clone())
            .await
            .context("installing stored MCP OAuth credentials")?;

        let manager = match state {
            OAuthState::Authorized(manager) | OAuthState::Unauthorized(manager) => manager,
            _ => bail!("unexpected MCP OAuth state while preparing stored credentials"),
        };

        Ok(Self {
            inner: Arc::new(McpOAuthRuntimeInner {
                server_name: server_name.to_string(),
                url: url.to_string(),
                manager: Arc::new(Mutex::new(manager)),
                last_tokens: Mutex::new(Some(tokens)),
            }),
        })
    }

    pub async fn authorization_header(&self) -> Result<Option<String>> {
        self.refresh_if_needed().await?;
        let credentials = {
            let guard = self.inner.manager.lock().await;
            let (_client_id, credentials) = guard
                .get_credentials()
                .await
                .context("reading MCP OAuth credentials")?;
            credentials
        };
        let Some(credentials) = credentials else {
            return Ok(None);
        };
        let token = credentials.access_token().secret().trim();
        if token.is_empty() {
            Ok(None)
        } else {
            Ok(Some(format!("Bearer {token}")))
        }
    }

    async fn refresh_if_needed(&self) -> Result<()> {
        let expires_at = {
            let guard = self.inner.last_tokens.lock().await;
            guard.as_ref().and_then(|tokens| tokens.expires_at)
        };
        if !token_needs_refresh(expires_at) {
            return Ok(());
        }

        let refresh_result = {
            let guard = self.inner.manager.lock().await;
            guard.refresh_token().await
        };
        if let Err(err) = refresh_result {
            let err = anyhow!(err);
            if error_looks_auth_required(&err) {
                self.clear_stored_tokens().await?;
            }
            return Err(err).with_context(|| {
                format!(
                    "refreshing MCP OAuth token for server {}",
                    self.inner.server_name
                )
            });
        }
        self.persist_if_needed().await
    }

    async fn clear_stored_tokens(&self) -> Result<()> {
        let mut last = self.inner.last_tokens.lock().await;
        if last.take().is_some() {
            delete_oauth_tokens(&self.inner.server_name, &self.inner.url)?;
        }
        Ok(())
    }

    async fn persist_if_needed(&self) -> Result<()> {
        let (client_id, credentials) = {
            let guard = self.inner.manager.lock().await;
            guard
                .get_credentials()
                .await
                .context("reading refreshed MCP OAuth credentials")?
        };
        let Some(credentials) = credentials else {
            let mut last = self.inner.last_tokens.lock().await;
            if last.take().is_some() {
                delete_oauth_tokens(&self.inner.server_name, &self.inner.url)?;
            }
            return Ok(());
        };

        let new_response = WrappedOAuthTokenResponse(credentials.clone());
        let mut last = self.inner.last_tokens.lock().await;
        let same_token = last
            .as_ref()
            .map(|previous| previous.token_response == new_response)
            .unwrap_or(false);
        let expires_at = if same_token {
            last.as_ref().and_then(|previous| previous.expires_at)
        } else {
            compute_expires_at_millis(&credentials)
        };
        let stored = StoredMcpOAuthTokens {
            server_name: self.inner.server_name.clone(),
            url: self.inner.url.clone(),
            client_id,
            token_response: new_response,
            expires_at,
        };
        if last.as_ref() != Some(&stored) {
            save_oauth_tokens(&stored)?;
            *last = Some(stored);
        }
        Ok(())
    }
}

pub async fn auth_status_for_server(name: &str, server: &McpServerConfig) -> McpAuthStatus {
    if !server.is_enabled() || server.url.is_none() {
        return McpAuthStatus::Unsupported;
    }
    if server_has_manual_authorization(server) {
        return McpAuthStatus::BearerToken;
    }
    let Some(url) = server.url.as_deref() else {
        return McpAuthStatus::Unsupported;
    };
    match load_oauth_tokens(name, url) {
        Ok(Some(tokens)) if oauth_tokens_are_usable(&tokens) => return McpAuthStatus::OAuth,
        Ok(Some(_)) => return McpAuthStatus::NotLoggedIn,
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(target: "mcp", server = %name, error = %err, "failed to read MCP OAuth tokens");
        }
    }

    let headers = match build_default_headers(&server.headers, &server.env_headers) {
        Ok(headers) => headers,
        Err(err) => {
            tracing::warn!(target: "mcp", server = %name, error = %err, "failed to build MCP OAuth discovery headers");
            return McpAuthStatus::Unsupported;
        }
    };
    match discover_streamable_http_oauth_with_headers(url, headers).await {
        Ok(Some(_)) => McpAuthStatus::NotLoggedIn,
        Ok(None) => McpAuthStatus::Unsupported,
        Err(err) => {
            tracing::debug!(target: "mcp", server = %name, error = %err, "MCP OAuth discovery failed");
            McpAuthStatus::Unsupported
        }
    }
}

pub async fn oauth_login_support(server: &McpServerConfig) -> Result<Option<McpOAuthDiscovery>> {
    let Some(url) = server.url.as_deref() else {
        return Ok(None);
    };
    if server_has_manual_authorization(server) {
        return Ok(None);
    }
    discover_streamable_http_oauth(url, server.headers.clone(), server.env_headers.clone()).await
}

pub async fn discover_streamable_http_oauth(
    url: &str,
    http_headers: HashMap<String, String>,
    env_headers: HashMap<String, String>,
) -> Result<Option<McpOAuthDiscovery>> {
    let headers = build_default_headers(&http_headers, &env_headers)?;
    discover_streamable_http_oauth_with_headers(url, headers).await
}

async fn discover_streamable_http_oauth_with_headers(
    url: &str,
    default_headers: HeaderMap,
) -> Result<Option<McpOAuthDiscovery>> {
    let client = apply_default_headers(crate::tls::reqwest_client_builder(), &default_headers)
        .timeout(Duration::from_secs(5))
        .build()
        .context("building MCP OAuth discovery client")?;
    let mut manager = AuthorizationManager::new(url).await?;
    manager.with_client(client)?;
    match manager.discover_metadata().await {
        Ok(metadata) => Ok(Some(McpOAuthDiscovery {
            scopes_supported: normalize_scopes(metadata.scopes_supported),
        })),
        Err(AuthError::NoAuthorizationSupport) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn resolve_oauth_scopes(
    explicit_scopes: Option<Vec<String>>,
    configured_scopes: Vec<String>,
    discovered_scopes: Option<Vec<String>>,
) -> ResolvedMcpOAuthScopes {
    if let Some(scopes) = explicit_scopes {
        return ResolvedMcpOAuthScopes {
            scopes,
            source: McpOAuthScopesSource::Explicit,
        };
    }
    if !configured_scopes.is_empty() {
        return ResolvedMcpOAuthScopes {
            scopes: configured_scopes,
            source: McpOAuthScopesSource::Configured,
        };
    }
    if let Some(scopes) = discovered_scopes
        && !scopes.is_empty()
    {
        return ResolvedMcpOAuthScopes {
            scopes,
            source: McpOAuthScopesSource::Discovered,
        };
    }
    ResolvedMcpOAuthScopes {
        scopes: Vec::new(),
        source: McpOAuthScopesSource::Empty,
    }
}

pub async fn perform_oauth_login_for_server(
    name: &str,
    server: &McpServerConfig,
    explicit_scopes: Option<Vec<String>>,
    callback_port: Option<u16>,
    callback_url: Option<&str>,
) -> Result<()> {
    let Some(url) = server.url.as_deref() else {
        bail!("OAuth login is only supported for URL-based MCP servers");
    };
    if server_has_manual_authorization(server) {
        bail!("MCP server '{name}' already has bearer/static Authorization configured");
    }

    let discovery = if explicit_scopes.is_none() && server.scopes.is_empty() {
        oauth_login_support(server).await?
    } else {
        None
    };
    let resolved_scopes = resolve_oauth_scopes(
        explicit_scopes,
        server.scopes.clone(),
        discovery.and_then(|discovery| discovery.scopes_supported),
    );

    match perform_oauth_login(
        name,
        url,
        server.headers.clone(),
        server.env_headers.clone(),
        &resolved_scopes.scopes,
        server.oauth_client_id(),
        server.oauth_resource.as_deref(),
        callback_port,
        callback_url,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(err)
            if resolved_scopes.source == McpOAuthScopesSource::Discovered
                && err.downcast_ref::<OAuthProviderError>().is_some() =>
        {
            println!("OAuth provider rejected discovered scopes. Retrying without scopes...");
            perform_oauth_login(
                name,
                url,
                server.headers.clone(),
                server.env_headers.clone(),
                &[],
                server.oauth_client_id(),
                server.oauth_resource.as_deref(),
                callback_port,
                callback_url,
            )
            .await
        }
        Err(err) => Err(err),
    }
}

#[allow(clippy::too_many_arguments)]
async fn perform_oauth_login(
    server_name: &str,
    server_url: &str,
    http_headers: HashMap<String, String>,
    env_headers: HashMap<String, String>,
    scopes: &[String],
    oauth_client_id: Option<&str>,
    oauth_resource: Option<&str>,
    callback_port: Option<u16>,
    callback_url: Option<&str>,
) -> Result<()> {
    OauthLoginFlow::new(
        server_name,
        server_url,
        http_headers,
        env_headers,
        scopes,
        oauth_client_id,
        oauth_resource,
        callback_port,
        callback_url,
    )
    .await?
    .finish()
    .await
}

pub fn delete_oauth_tokens_for_server(name: &str, server: &McpServerConfig) -> Result<bool> {
    let Some(url) = server.url.as_deref() else {
        bail!("OAuth logout is only supported for URL-based MCP servers");
    };
    delete_oauth_tokens(name, url)
}

fn server_has_manual_authorization(server: &McpServerConfig) -> bool {
    server.bearer_token_env_var.is_some()
        || contains_authorization_header(&server.headers)
        || contains_authorization_header(&server.env_headers)
}

pub fn build_default_headers(
    http_headers: &HashMap<String, String>,
    env_headers: &HashMap<String, String>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in http_headers {
        insert_header(&mut headers, name, value)?;
    }
    for (name, env_var) in env_headers {
        if let Ok(value) = std::env::var(env_var)
            && !value.trim().is_empty()
        {
            insert_header(&mut headers, name, &value)?;
        }
    }
    Ok(headers)
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<()> {
    if !super::headers::is_safe_custom_header(name, value) {
        bail!("unsafe MCP HTTP header '{name}'");
    }
    let name = HeaderName::from_bytes(name.as_bytes())
        .with_context(|| format!("invalid MCP HTTP header name '{name}'"))?;
    let value = HeaderValue::from_str(value).with_context(|| "invalid MCP HTTP header value")?;
    headers.insert(name, value);
    Ok(())
}

pub fn apply_default_headers(
    builder: reqwest::ClientBuilder,
    headers: &HeaderMap,
) -> reqwest::ClientBuilder {
    if headers.is_empty() {
        builder
    } else {
        builder.default_headers(headers.clone())
    }
}

fn contains_authorization_header(headers: &HashMap<String, String>) -> bool {
    headers
        .keys()
        .any(|key| key.trim().eq_ignore_ascii_case("authorization"))
}

fn normalize_scopes(scopes_supported: Option<Vec<String>>) -> Option<Vec<String>> {
    let scopes_supported = scopes_supported?;
    let mut normalized = Vec::new();
    for scope in scopes_supported {
        let scope = scope.trim();
        if scope.is_empty() {
            continue;
        }
        let scope = scope.to_string();
        if !normalized.contains(&scope) {
            normalized.push(scope);
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

fn load_oauth_tokens(server_name: &str, url: &str) -> Result<Option<StoredMcpOAuthTokens>> {
    let secrets = codewhale_secrets::Secrets::auto_detect();
    let key = store_key(server_name, url);
    let Some(serialized) = secrets
        .get(&key)
        .with_context(|| format!("reading MCP OAuth token for '{server_name}'"))?
    else {
        return Ok(None);
    };
    let mut tokens: StoredMcpOAuthTokens = serde_json::from_str(&serialized)
        .with_context(|| format!("parsing stored MCP OAuth token for '{server_name}'"))?;
    refresh_expires_in_from_timestamp(&mut tokens);
    Ok(Some(tokens))
}

fn save_oauth_tokens(tokens: &StoredMcpOAuthTokens) -> Result<()> {
    let secrets = codewhale_secrets::Secrets::auto_detect();
    let key = store_key(&tokens.server_name, &tokens.url);
    let serialized = serde_json::to_string(tokens).context("serializing MCP OAuth token")?;
    secrets
        .set(&key, &serialized)
        .with_context(|| format!("saving MCP OAuth token for '{}'", tokens.server_name))
}

fn delete_oauth_tokens(server_name: &str, url: &str) -> Result<bool> {
    let secrets = codewhale_secrets::Secrets::auto_detect();
    let key = store_key(server_name, url);
    let existed = secrets
        .get(&key)
        .with_context(|| format!("reading MCP OAuth token for '{server_name}'"))?
        .is_some();
    secrets
        .delete(&key)
        .with_context(|| format!("deleting MCP OAuth token for '{server_name}'"))?;
    Ok(existed)
}

fn store_key(server_name: &str, url: &str) -> String {
    let mut payload = Vec::with_capacity(server_name.len() + url.len() + 1);
    payload.extend_from_slice(server_name.as_bytes());
    payload.push(0);
    payload.extend_from_slice(url.as_bytes());
    let digest = Sha256::digest(&payload);
    format!("mcp_oauth_{}", URL_SAFE_NO_PAD.encode(digest))
}

fn oauth_tokens_are_usable(tokens: &StoredMcpOAuthTokens) -> bool {
    if tokens.client_id.trim().is_empty() {
        return false;
    }
    let response = &tokens.token_response.0;
    if token_needs_refresh(tokens.expires_at) {
        return response
            .refresh_token()
            .is_some_and(|token| !token.secret().trim().is_empty());
    }
    !response.access_token().secret().trim().is_empty()
}

fn refresh_expires_in_from_timestamp(tokens: &mut StoredMcpOAuthTokens) {
    let Some(expires_at) = tokens.expires_at else {
        return;
    };
    match expires_in_from_timestamp(expires_at) {
        Some(seconds) => {
            let duration = Duration::from_secs(seconds);
            tokens.token_response.0.set_expires_in(Some(&duration));
        }
        None => {
            tokens
                .token_response
                .0
                .set_expires_in(Some(&Duration::ZERO));
        }
    }
}

fn compute_expires_at_millis(response: &OAuthTokenResponse) -> Option<u64> {
    let expires = response.expires_in()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    Some(now.saturating_add(expires.as_millis() as u64))
}

fn expires_in_from_timestamp(expires_at: u64) -> Option<u64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    if expires_at <= now {
        return None;
    }
    Some((expires_at - now) / 1000)
}

fn token_needs_refresh(expires_at: Option<u64>) -> bool {
    let Some(expires_at) = expires_at else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    now.saturating_add(REFRESH_SKEW_MILLIS) >= expires_at
}

struct CallbackServerGuard {
    server: Arc<Server>,
}

impl Drop for CallbackServerGuard {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

struct OauthLoginFlow {
    auth_url: String,
    oauth_state: OAuthState,
    rx: oneshot::Receiver<CallbackResult>,
    guard: CallbackServerGuard,
    server_name: String,
    server_url: String,
}

impl OauthLoginFlow {
    #[allow(clippy::too_many_arguments)]
    async fn new(
        server_name: &str,
        server_url: &str,
        http_headers: HashMap<String, String>,
        env_headers: HashMap<String, String>,
        scopes: &[String],
        oauth_client_id: Option<&str>,
        oauth_resource: Option<&str>,
        callback_port: Option<u16>,
        callback_url: Option<&str>,
    ) -> Result<Self> {
        let bind_host = callback_bind_host(callback_url);
        let bind_addr = match callback_port {
            Some(0) => bail!("invalid MCP OAuth callback port 0"),
            Some(port) => format!("{bind_host}:{port}"),
            None => format!("{bind_host}:0"),
        };
        let server = Arc::new(Server::http(&bind_addr).map_err(|err| anyhow!(err))?);
        let guard = CallbackServerGuard {
            server: Arc::clone(&server),
        };
        let redirect_uri = resolve_redirect_uri(&server, callback_url)?;
        let callback_id = callback_id_from_server_url(server_url)?;
        let redirect_uri = append_callback_id_to_redirect_uri(&redirect_uri, &callback_id)?;
        let callback_path = callback_path_from_redirect_uri(&redirect_uri)?;

        let (tx, rx) = oneshot::channel();
        spawn_callback_server(server, tx, callback_path);

        let headers = build_default_headers(&http_headers, &env_headers)?;
        let client = apply_default_headers(crate::tls::reqwest_client_builder(), &headers)
            .build()
            .context("building MCP OAuth login client")?;
        let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
        let oauth_state = start_authorization(
            server_url,
            client,
            &scope_refs,
            &redirect_uri,
            oauth_client_id,
        )
        .await?;
        let auth_url = append_query_param(
            &oauth_state.get_authorization_url().await?,
            "resource",
            oauth_resource,
        );

        Ok(Self {
            auth_url,
            oauth_state,
            rx,
            guard,
            server_name: server_name.to_string(),
            server_url: server_url.to_string(),
        })
    }

    async fn finish(mut self) -> Result<()> {
        println!(
            "Authorize `{}` by opening this URL in your browser:\n{}\n",
            self.server_name, self.auth_url
        );
        if webbrowser::open(&self.auth_url).is_err() {
            eprintln!("Browser launch failed; copy the URL above manually.");
        }
        println!(
            "Waiting for browser authorization for MCP server '{}'...",
            self.server_name
        );

        let result = async {
            let callback = timeout(Duration::from_secs(300), &mut self.rx)
                .await
                .with_context(|| {
                    format!(
                        "timed out waiting for OAuth callback for MCP server '{}'. Retry from a terminal, or use task_shell_start/background shell if an agent is running the login flow.",
                        self.server_name
                    )
                })?
                .context("OAuth callback was cancelled")?;
            let OauthCallbackResult { code, state } = match callback {
                CallbackResult::Success(callback) => callback,
                CallbackResult::Error(error) => return Err(anyhow!(error)),
            };

            self.oauth_state
                .handle_callback(&code, &state)
                .await
                .context("handling MCP OAuth callback")?;

            let (client_id, credentials) = self
                .oauth_state
                .get_credentials()
                .await
                .context("reading MCP OAuth credentials")?;
            let credentials =
                credentials.ok_or_else(|| anyhow!("OAuth provider did not return credentials"))?;
            let stored = StoredMcpOAuthTokens {
                server_name: self.server_name.clone(),
                url: self.server_url.clone(),
                client_id,
                expires_at: compute_expires_at_millis(&credentials),
                token_response: WrappedOAuthTokenResponse(credentials),
            };
            save_oauth_tokens(&stored)
        }
        .await;

        drop(self.guard);
        result
    }
}

async fn start_authorization(
    server_url: &str,
    client: reqwest::Client,
    scopes: &[&str],
    redirect_uri: &str,
    oauth_client_id: Option<&str>,
) -> Result<OAuthState> {
    let Some(client_id) = oauth_client_id.filter(|client_id| !client_id.trim().is_empty()) else {
        let mut oauth_state = OAuthState::new(server_url, Some(client)).await?;
        oauth_state
            .start_authorization(scopes, redirect_uri, Some("CodeWhale"))
            .await?;
        return Ok(oauth_state);
    };

    let mut manager = AuthorizationManager::new(server_url).await?;
    manager.with_client(client)?;
    let metadata = manager.discover_metadata().await?;
    manager.set_metadata(metadata);
    manager.configure_client(
        OAuthClientConfig::new(client_id, redirect_uri)
            .with_scopes(scopes.iter().map(|scope| (*scope).to_string()).collect()),
    )?;
    let auth_url = manager.get_authorization_url(scopes).await?;
    Ok(OAuthState::Session(
        AuthorizationSession::for_scope_upgrade(manager, auth_url, redirect_uri),
    ))
}

fn spawn_callback_server(
    server: Arc<Server>,
    tx: oneshot::Sender<CallbackResult>,
    expected_callback_path: String,
) {
    tokio::task::spawn_blocking(move || {
        while let Ok(request) = server.recv() {
            let path = request.url().to_string();
            match parse_oauth_callback(&path, &expected_callback_path) {
                CallbackOutcome::Success(callback) => {
                    let response = Response::from_string(
                        "Authentication complete. You may close this window.",
                    );
                    let _ = request.respond(response);
                    let _ = tx.send(CallbackResult::Success(callback));
                    break;
                }
                CallbackOutcome::Error(error) => {
                    let response = Response::from_string(error.to_string()).with_status_code(400);
                    let _ = request.respond(response);
                    let _ = tx.send(CallbackResult::Error(error));
                    break;
                }
                CallbackOutcome::Invalid => {
                    let response =
                        Response::from_string("Invalid OAuth callback").with_status_code(400);
                    let _ = request.respond(response);
                }
            }
        }
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OauthCallbackResult {
    code: String,
    state: String,
}

enum CallbackResult {
    Success(OauthCallbackResult),
    Error(OAuthProviderError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CallbackOutcome {
    Success(OauthCallbackResult),
    Error(OAuthProviderError),
    Invalid,
}

fn parse_oauth_callback(path: &str, expected_callback_path: &str) -> CallbackOutcome {
    let Some((route, query)) = path.split_once('?') else {
        return CallbackOutcome::Invalid;
    };
    if route != expected_callback_path {
        return CallbackOutcome::Invalid;
    }

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let Ok(decoded) = decode(value) else {
            continue;
        };
        let decoded = decoded.into_owned();
        match key {
            "code" => code = Some(decoded),
            "state" => state = Some(decoded),
            "error" => error = Some(decoded),
            "error_description" => error_description = Some(decoded),
            _ => {}
        }
    }

    if let (Some(code), Some(state)) = (code, state) {
        return CallbackOutcome::Success(OauthCallbackResult { code, state });
    }
    if error.is_some() || error_description.is_some() {
        return CallbackOutcome::Error(OAuthProviderError::new(error, error_description));
    }
    CallbackOutcome::Invalid
}

fn local_redirect_uri(server: &Server) -> Result<String> {
    match server.server_addr() {
        tiny_http::ListenAddr::IP(std::net::SocketAddr::V4(addr)) => {
            Ok(format!("http://{}:{}/callback", addr.ip(), addr.port()))
        }
        tiny_http::ListenAddr::IP(std::net::SocketAddr::V6(addr)) => {
            Ok(format!("http://[{}]:{}/callback", addr.ip(), addr.port()))
        }
        #[cfg(not(target_os = "windows"))]
        _ => Err(anyhow!("unable to determine callback address")),
    }
}

fn resolve_redirect_uri(server: &Server, callback_url: Option<&str>) -> Result<String> {
    let Some(callback_url) = callback_url else {
        return local_redirect_uri(server);
    };
    Url::parse(callback_url)
        .with_context(|| format!("invalid MCP OAuth callback URL '{callback_url}'"))?;
    Ok(callback_url.to_string())
}

fn callback_bind_host(callback_url: Option<&str>) -> &'static str {
    let Some(callback_url) = callback_url else {
        return "127.0.0.1";
    };
    let Ok(parsed) = Url::parse(callback_url) else {
        return "127.0.0.1";
    };
    match parsed.host_str() {
        Some("localhost" | "127.0.0.1" | "::1") | None => "127.0.0.1",
        Some(_) => "0.0.0.0",
    }
}

fn callback_id_from_server_url(server_url: &str) -> Result<String> {
    let mut parsed =
        Url::parse(server_url).with_context(|| format!("invalid MCP server URL '{server_url}'"))?;
    parsed
        .host_str()
        .ok_or_else(|| anyhow!("MCP server URL '{server_url}' must include a host"))?;
    parsed.set_fragment(None);
    let digest = Sha256::digest(parsed.as_str().as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(&digest[..9]))
}

fn append_callback_id_to_redirect_uri(redirect_uri: &str, callback_id: &str) -> Result<String> {
    let mut parsed = Url::parse(redirect_uri)
        .with_context(|| format!("invalid redirect URI '{redirect_uri}'"))?;
    let path = parsed.path();
    let new_path = if path.ends_with('/') {
        format!("{path}{callback_id}")
    } else {
        format!("{path}/{callback_id}")
    };
    parsed.set_path(&new_path);
    Ok(parsed.to_string())
}

fn callback_path_from_redirect_uri(redirect_uri: &str) -> Result<String> {
    let parsed = Url::parse(redirect_uri)
        .with_context(|| format!("invalid redirect URI '{redirect_uri}'"))?;
    Ok(parsed.path().to_string())
}

fn append_query_param(url: &str, key: &str, value: Option<&str>) -> String {
    let Some(value) = value else {
        return url.to_string();
    };
    let value = value.trim();
    if value.is_empty() {
        return url.to_string();
    }
    if let Ok(mut parsed) = Url::parse(url) {
        parsed.query_pairs_mut().append_pair(key, value);
        return parsed.to_string();
    }
    let separator = if url.contains('?') { "&" } else { "?" };
    format!("{url}{separator}{key}={}", urlencoding::encode(value))
}

impl McpServerConfig {
    pub fn oauth_client_id(&self) -> Option<&str> {
        self.oauth
            .as_ref()
            .and_then(|oauth| oauth.client_id.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_oauth_scopes_prefers_explicit() {
        let resolved = resolve_oauth_scopes(
            Some(vec!["explicit".to_string()]),
            vec!["configured".to_string()],
            Some(vec!["discovered".to_string()]),
        );
        assert_eq!(resolved.source, McpOAuthScopesSource::Explicit);
        assert_eq!(resolved.scopes, vec!["explicit"]);
    }

    #[test]
    fn parse_oauth_callback_accepts_success() {
        let parsed = parse_oauth_callback("/callback/id?code=abc&state=xyz", "/callback/id");
        assert!(matches!(parsed, CallbackOutcome::Success(_)));
    }

    #[test]
    fn parse_oauth_callback_accepts_provider_error() {
        let parsed = parse_oauth_callback(
            "/callback/id?error=invalid_scope&error_description=nope",
            "/callback/id",
        );
        assert!(matches!(parsed, CallbackOutcome::Error(_)));
    }

    #[test]
    fn store_key_does_not_include_raw_url_or_name() {
        let key = store_key("github", "https://example.com/mcp");
        assert!(key.starts_with("mcp_oauth_"));
        assert!(!key.contains("github"));
        assert!(!key.contains("example.com"));
    }

    #[test]
    fn auth_required_classifier_matches_http_401_shapes() {
        let err = anyhow!("MCP Streamable HTTP rejected status=401 Unauthorized");
        assert!(error_looks_auth_required(&err));

        let err = anyhow!("authentication_required for remote server");
        assert!(error_looks_auth_required(&err));

        let err = anyhow!("connection refused");
        assert!(!error_looks_auth_required(&err));
    }

    #[test]
    fn auth_required_login_hint_names_server() {
        let hint = auth_required_login_hint("nordic-mcp");
        assert!(hint.contains("nordic-mcp"));
        assert!(hint.contains("codewhale mcp login nordic-mcp"));
    }
}
