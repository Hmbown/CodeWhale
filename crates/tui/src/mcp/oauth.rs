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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
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
    error_text_looks_auth_required(&format!("{error:#}"))
}

/// Whether the error chain carries the OAuth `invalid_grant` code: the
/// authorization server definitively rejected the presented grant (typically
/// a stale or already-rotated refresh token).
fn error_is_invalid_grant(error: &anyhow::Error) -> bool {
    format!("{error:#}")
        .to_ascii_lowercase()
        .contains("invalid_grant")
}

/// The one auth-required classifier every surface consults: the pool's
/// `◆ auth required` state, the session-boot row, the `/mcp` manager
/// recovery verb, and the synthetic `mcp_<server>_authenticate` tool all
/// derive from this predicate so a failure is never "needs login" on one
/// surface and "failed" on another. `invalid_grant` belongs here because the
/// authorization server has definitively rejected the stored grant — only a
/// fresh login recovers it.
pub fn error_text_looks_auth_required(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    // `auth required` and `requires oauth` are anchored to the shapes this
    // product and the Codex-compatible managers actually emit (`◆ auth
    // required`, `requires OAuth login/authentication/reauthentication`) —
    // bare substrings would misclassify incidental server errors like
    // "auth required parameter is missing".
    text.contains("401")
        || text.contains("unauthorized")
        || text.contains("authentication_required")
        || text.contains("invalid_grant")
        || text.contains("◆ auth required")
        || text.contains("requires oauth login")
        || text.contains("requires oauth authentication")
        || text.contains("requires oauth reauthentication")
        || text.contains("not logged in")
        || text.contains("not-logged-in")
        || text.contains("re-authorize")
        || text.contains("/mcp login")
        || text.contains("mcp login")
}

pub fn auth_required_login_hint(server_name: &str) -> String {
    format!(
        "MCP server '{server_name}' requires OAuth authentication. Run `codewhale mcp login {server_name}` to authenticate."
    )
}

/// The one recovery sentence for a server in the `◆ auth required` state,
/// chosen by how that server is allowed to authenticate. OAuth-servable
/// servers get the login command; plugin-contributed servers (OAuth is
/// disabled for them by review policy) and servers with a manual
/// Authorization configuration are told which environment-backed
/// credential to supply instead, so `/mcp login` is never named for a
/// server it would refuse. Environment variable *names* are not secrets;
/// their values never appear here.
pub(crate) fn auth_required_recovery_hint(server_name: &str, server: &McpServerConfig) -> String {
    let mut env_vars: Vec<&str> = server
        .env_headers
        .values()
        .map(String::as_str)
        .chain(server.bearer_token_env_var.as_deref())
        .collect();
    env_vars.sort_unstable();
    env_vars.dedup();
    let credential_source = if env_vars.is_empty() {
        "its configured Authorization header".to_string()
    } else {
        format!(
            "the environment variable{} {}",
            if env_vars.len() == 1 { "" } else { "s" },
            env_vars.join(", ")
        )
    };
    if let Some(source) = server.reviewed_plugin.as_ref() {
        return format!(
            "MCP server '{server_name}' is contributed by plugin '{}' and its credential comes from {credential_source} (OAuth login is disabled for plugin-contributed servers). Set the credential, then run `/mcp reload`.",
            source.authority.plugin_name
        );
    }
    if server_has_manual_authorization(server) {
        return format!(
            "MCP server '{server_name}' authenticates with {credential_source}; the server rejected that credential. Correct it, then run `/mcp reload`."
        );
    }
    auth_required_login_hint(server_name)
}

/// TUI recovery for a stale Streamable HTTP OAuth session. `/mcp auth` is not a
/// command; login is `/mcp login <name>` (CLI: `codewhale mcp login <name>`).
pub fn tui_reauth_hint() -> &'static str {
    "Re-authorize this server (/mcp login <name>) to continue."
}

pub fn tui_reauth_refresh_failed_hint() -> &'static str {
    "Re-authorize this server (/mcp login <name>) or configure a fresh bearer token."
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
    /// Why the held credential was invalidated (the provider's error code,
    /// e.g. `invalid_grant`), so every later failure names the cause even
    /// though the rejected grant is never replayed. `None` while a
    /// credential is held.
    rejection: Mutex<Option<String>>,
    /// Default headers the runtime was built with, retained so an adopted
    /// on-disk rotation can rebuild the manager with identical HTTP shape.
    default_headers: HeaderMap,
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

/// Build an `AuthorizationManager` preloaded with stored credentials, the
/// shared construction step for initial load and for adopting a credential
/// that another process rotated on disk.
async fn manager_from_stored_tokens(
    url: &str,
    tokens: &StoredMcpOAuthTokens,
    default_headers: &HeaderMap,
) -> Result<AuthorizationManager> {
    let client = apply_default_headers(crate::tls::reqwest_client_builder(), default_headers)
        .build()
        .context("building MCP OAuth metadata client")?;
    let mut state = OAuthState::new(url.to_string(), Some(client)).await?;
    state
        .set_credentials(&tokens.client_id, tokens.token_response.0.clone())
        .await
        .context("installing stored MCP OAuth credentials")?;

    match state {
        OAuthState::Authorized(manager) | OAuthState::Unauthorized(manager) => Ok(manager),
        _ => bail!("unexpected MCP OAuth state while preparing stored credentials"),
    }
}

impl McpOAuthRuntime {
    pub async fn from_server_config(
        server_name: &str,
        server: &McpServerConfig,
        default_headers: HeaderMap,
    ) -> Result<Option<Self>> {
        if server.reviewed_plugin.is_some() {
            return Ok(None);
        }
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
        let manager = manager_from_stored_tokens(url, &tokens, &default_headers).await?;

        Ok(Self {
            inner: Arc::new(McpOAuthRuntimeInner {
                server_name: server_name.to_string(),
                url: url.to_string(),
                manager: Arc::new(Mutex::new(manager)),
                last_tokens: Mutex::new(Some(tokens)),
                rejection: Mutex::new(None),
                default_headers,
            }),
        })
    }

    pub async fn authorization_header(&self) -> Result<Option<String>> {
        self.refresh_if_needed().await?;
        // Never send a credential the provider already rejected; the request
        // goes out unauthenticated and the server's 401 drives the reactive
        // refresh, which adopts a peer's login or reports auth-required.
        if self.is_invalidated().await {
            return Ok(None);
        }
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
        self.refresh_and_persist().await
    }

    /// Force a token refresh regardless of the local expiry clock (T4): a
    /// 401/403 means the server no longer accepts the token — clock skew,
    /// server-side revocation, or rotation — so the expiry-based gate must
    /// not decide alone.
    pub(crate) async fn force_refresh(&self) -> Result<()> {
        self.refresh_and_persist().await
    }

    /// Whether this runtime's credential was definitively rejected by the
    /// provider and invalidated. `last_tokens` is `Some` from construction
    /// and after every persisted refresh; only [`Self::clear_stored_tokens`]
    /// empties it.
    async fn is_invalidated(&self) -> bool {
        self.inner.last_tokens.lock().await.is_none()
    }

    async fn refresh_and_persist(&self) -> Result<()> {
        // A credential the provider definitively rejected is never replayed:
        // the `AuthorizationManager` still holds it, but every later refresh
        // with that grant is a guaranteed `invalid_grant`. The only way back
        // is a credential another process stored since (a completed login),
        // so adopt that when present and otherwise report auth-required
        // without touching the token endpoint.
        if self.is_invalidated().await {
            if !self.adopt_rotated_on_disk_tokens().await? {
                let reason = self
                    .inner
                    .rejection
                    .lock()
                    .await
                    .clone()
                    .unwrap_or_else(|| "unauthorized".to_string());
                bail!(
                    "stored MCP OAuth credential for server {} was rejected by the provider ({reason}) and removed; the server requires OAuth login again",
                    self.inner.server_name
                );
            }
            let adopted_needs_refresh = {
                let last = self.inner.last_tokens.lock().await;
                token_needs_refresh(last.as_ref().and_then(|tokens| tokens.expires_at))
            };
            if !adopted_needs_refresh {
                return Ok(());
            }
        }
        let mut err = match self.try_refresh_and_persist().await {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };
        // Refresh-race tolerance: another codewhale process sharing this token
        // store (a concurrent `mcp login`, or a peer session's refresh) may
        // have rotated the credential after this runtime loaded its copy, and
        // single-use refresh tokens then fail here with `invalid_grant`.
        // Re-read the store once; when the on-disk credential changed, adopt
        // it — using it directly while fresh, or retrying the refresh exactly
        // once with the rotated grant — before surfacing failure. When the
        // store is unchanged the grant is simply dead: the auth-required
        // branch below invalidates it so the server flips to `◆ auth
        // required` and the self-serve login tool appears, instead of every
        // later connect replaying the same rejected refresh.
        if error_is_invalid_grant(&err) && self.adopt_rotated_on_disk_tokens().await? {
            let adopted_needs_refresh = {
                let last = self.inner.last_tokens.lock().await;
                token_needs_refresh(last.as_ref().and_then(|tokens| tokens.expires_at))
            };
            if !adopted_needs_refresh {
                return Ok(());
            }
            match self.try_refresh_and_persist().await {
                Ok(()) => return Ok(()),
                Err(retry_err) => err = retry_err,
            }
        }
        if error_looks_auth_required(&err) {
            let reason = if error_is_invalid_grant(&err) {
                "invalid_grant"
            } else {
                "unauthorized"
            };
            self.clear_stored_tokens(reason).await?;
        }
        Err(err).with_context(|| {
            format!(
                "refreshing MCP OAuth token for server {}",
                self.inner.server_name
            )
        })
    }

    async fn try_refresh_and_persist(&self) -> Result<()> {
        let refresh_result = {
            let guard = self.inner.manager.lock().await;
            guard.refresh_token().await
        };
        refresh_result.map_err(|err| anyhow!(err))?;
        self.persist_if_needed().await
    }

    /// Re-read the on-disk credential after an `invalid_grant` refresh
    /// failure and, when another process rotated it, rebuild the manager
    /// around the rotated token exactly like initial construction. Returns
    /// `true` only when the stored credential actually changed; an unchanged
    /// store means the failure is ours to report.
    async fn adopt_rotated_on_disk_tokens(&self) -> Result<bool> {
        let Some(stored) = load_oauth_tokens(&self.inner.server_name, &self.inner.url)? else {
            return Ok(false);
        };
        let changed = {
            let last = self.inner.last_tokens.lock().await;
            last.as_ref() != Some(&stored)
        };
        if !changed {
            return Ok(false);
        }
        let manager =
            manager_from_stored_tokens(&self.inner.url, &stored, &self.inner.default_headers)
                .await?;
        *self.inner.manager.lock().await = manager;
        *self.inner.last_tokens.lock().await = Some(stored);
        *self.inner.rejection.lock().await = None;
        Ok(true)
    }

    /// Invalidate the credential this runtime holds after the provider
    /// definitively rejected it. Never deletes a newer durable winner: when
    /// the on-disk credential no longer matches the one we hold, another
    /// process rotated it after our copy loaded, and that credential — not
    /// ours — is the one the next connect must try. The manager is rebuilt
    /// around that winner exactly like initial construction; remembering
    /// the rotated token while keeping our dead grant would make the next
    /// `invalid_grant` compare an "unchanged" store and delete the newer
    /// valid credential.
    async fn clear_stored_tokens(&self, reason: &str) -> Result<()> {
        let held = { self.inner.last_tokens.lock().await.take() };
        let Some(held) = held else {
            return Ok(());
        };
        match load_oauth_tokens(&self.inner.server_name, &self.inner.url)? {
            Some(stored) if stored != held => {
                tracing::debug!(
                    target: "mcp",
                    server = %self.inner.server_name,
                    "MCP OAuth credential was rotated by another process; keeping the on-disk winner"
                );
                let manager = manager_from_stored_tokens(
                    &self.inner.url,
                    &stored,
                    &self.inner.default_headers,
                )
                .await?;
                *self.inner.manager.lock().await = manager;
                *self.inner.last_tokens.lock().await = Some(stored);
                *self.inner.rejection.lock().await = None;
            }
            _ => {
                delete_oauth_tokens(&self.inner.server_name, &self.inner.url)?;
                *self.inner.rejection.lock().await = Some(reason.to_string());
            }
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
    if server.reviewed_plugin.is_some() || !server.is_enabled() || server.url.is_none() {
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
    if server.reviewed_plugin.is_some() {
        return Ok(None);
    }
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
    perform_oauth_login_for_server_with_cancel(
        name,
        server,
        explicit_scopes,
        callback_port,
        callback_url,
        CancellationToken::new(),
    )
    .await
}

/// Run an MCP OAuth login that can be stopped by the caller.
///
/// Cancellation drops the in-flight OAuth future before this function returns,
/// which also closes its callback listener. A caller that replaces one login
/// with another should await the cancelled call before starting the replacement.
pub async fn perform_oauth_login_for_server_with_cancel(
    name: &str,
    server: &McpServerConfig,
    explicit_scopes: Option<Vec<String>>,
    callback_port: Option<u16>,
    callback_url: Option<&str>,
    cancellation_token: CancellationToken,
) -> Result<()> {
    if server.reviewed_plugin.is_some() {
        bail!(
            "OAuth is disabled for plugin-contributed MCP servers; use a reviewed environment-backed header or bearer token"
        );
    }
    run_cancellable_oauth(
        &cancellation_token,
        perform_oauth_login_for_server_inner(
            name,
            server,
            explicit_scopes,
            callback_port,
            callback_url,
        ),
    )
    .await
}

async fn run_cancellable_oauth<F, T>(cancellation_token: &CancellationToken, future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => bail!("OAuth login was cancelled"),
        result = future => result,
    }
}

/// Shared gate + scope resolution for `/mcp login` and the model-driven
/// authenticate tool: URL-based servers only, no manual Authorization config,
/// scopes from explicit argument, config, or discovery (in that order).
async fn resolve_oauth_login(
    name: &str,
    server: &McpServerConfig,
    explicit_scopes: Option<Vec<String>>,
) -> Result<(String, ResolvedMcpOAuthScopes)> {
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
    Ok((url.to_string(), resolved_scopes))
}

async fn perform_oauth_login_for_server_inner(
    name: &str,
    server: &McpServerConfig,
    explicit_scopes: Option<Vec<String>>,
    callback_port: Option<u16>,
    callback_url: Option<&str>,
) -> Result<()> {
    let (url, resolved_scopes) = resolve_oauth_login(name, server, explicit_scopes).await?;

    match perform_oauth_login(
        name,
        &url,
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
                &url,
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

/// How an OAuth login announces its authorization URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OAuthLoginAnnounce {
    /// `/mcp login` in a terminal: print the URL and open a browser.
    Terminal,
    /// Model-driven `mcp_<server>_authenticate` tool: never write to stdout
    /// (a tool call inside a running session is not a terminal); the result
    /// carries the URL for the model to relay to the user verbatim.
    Tool { open_browser: bool },
}

/// An in-flight OAuth login started by the model-driven authenticate tool.
///
/// The authorization URL is available immediately so the model can relay it
/// to the user verbatim; [`McpOAuthToolLogin::finish`] then blocks on the
/// loopback callback (up to 5 minutes, same as `/mcp login`) and persists
/// the issued tokens to the shared store on success.
pub struct McpOAuthToolLogin {
    server_name: String,
    server: McpServerConfig,
    scopes_source: McpOAuthScopesSource,
    flow: OauthLoginFlow,
    open_browser: bool,
}

impl McpOAuthToolLogin {
    /// The exact authorization URL the user must visit. Treat it as
    /// sensitive: never modify it or strip query parameters.
    #[must_use]
    pub fn authorization_url(&self) -> &str {
        &self.flow.auth_url
    }

    /// Block on the browser callback and persist the issued tokens. Mirrors
    /// the `/mcp login` retry: when the provider rejects scopes that came
    /// from discovery (rather than explicit config), restart once without
    /// scopes.
    pub async fn finish(self) -> Result<()> {
        let announce = OAuthLoginAnnounce::Tool {
            open_browser: self.open_browser,
        };
        let retry_without_scopes = self.scopes_source == McpOAuthScopesSource::Discovered;
        match self.flow.finish_with_announce(announce).await {
            Ok(()) => Ok(()),
            Err(err)
                if retry_without_scopes && err.downcast_ref::<OAuthProviderError>().is_some() =>
            {
                let server = &self.server;
                let url = server
                    .url
                    .as_deref()
                    .expect("tool login is gated to URL-based servers at begin");
                OauthLoginFlow::new(
                    &self.server_name,
                    url,
                    server.headers.clone(),
                    server.env_headers.clone(),
                    &[],
                    server.oauth_client_id(),
                    server.oauth_resource.as_deref(),
                    None,
                    None,
                )
                .await?
                .finish_with_announce(announce)
                .await
            }
            Err(err) => Err(err),
        }
    }
}

/// Begin the same OAuth login flow `/mcp login` runs, for the model-driven
/// `mcp_<server>_authenticate` tool. The callback listener binds an
/// ephemeral loopback port; callers needing a pre-registered redirect URI
/// keep the terminal `/mcp login <name>` path, which honors the configured
/// callback overrides.
pub async fn begin_oauth_login_for_server_tool(
    name: &str,
    server: &McpServerConfig,
    callback_port: Option<u16>,
    callback_url: Option<&str>,
) -> Result<McpOAuthToolLogin> {
    if server.reviewed_plugin.is_some() {
        bail!(
            "OAuth is disabled for plugin-contributed MCP servers; use a reviewed environment-backed header or bearer token"
        );
    }
    let (url, resolved_scopes) = resolve_oauth_login(name, server, None).await?;
    let flow = OauthLoginFlow::new(
        name,
        &url,
        server.headers.clone(),
        server.env_headers.clone(),
        &resolved_scopes.scopes,
        server.oauth_client_id(),
        server.oauth_resource.as_deref(),
        callback_port,
        callback_url,
    )
    .await?;
    Ok(McpOAuthToolLogin {
        server_name: name.to_string(),
        server: server.clone(),
        scopes_source: resolved_scopes.source,
        flow,
        // The test build drives the loopback callback itself; a real browser
        // launch from a unit test would hijack the developer's desktop.
        open_browser: !cfg!(test),
    })
}

/// Whether the self-serve OAuth login flow can run for this server at all:
/// URL-based, not plugin-contributed (plugin servers authenticate through
/// reviewed environment-backed headers, and OAuth storage is disabled for
/// them), and without a manual Authorization configuration that an OAuth
/// login would conflict with.
pub(crate) fn server_supports_oauth_login(server: &McpServerConfig) -> bool {
    server.reviewed_plugin.is_none()
        && server.url.is_some()
        && !server_has_manual_authorization(server)
}

/// Whether the shared token store already holds a usable credential for this
/// server — i.e. a login completed in another process since the caller last
/// checked. Used by the authenticate tool's already-authorized branch.
pub(crate) fn has_usable_stored_tokens(name: &str, server: &McpServerConfig) -> bool {
    let Some(url) = server.url.as_deref() else {
        return false;
    };
    load_oauth_tokens(name, url)
        .ok()
        .flatten()
        .is_some_and(|tokens| oauth_tokens_are_usable(&tokens))
}

/// Model-facing description for the synthetic `mcp_<server>_authenticate`
/// tool. The coaching contract (show the URL verbatim, the call blocks, real
/// tools replace this one on success) is pinned by tests.
pub(crate) fn authenticate_tool_description(server_name: &str) -> String {
    format!(
        "Authenticate with MCP server \"{server_name}\" via OAuth.\n\n\
This server requires an OAuth login that has not yet been completed, so its \
real tools are currently unavailable. Calling this tool starts the \
authorization flow:\n\n\
1. A browser window is opened for the user to sign in and approve the \
Codewhale client, and the exact authorization URL is shown to the user in \
the session status while this call waits. The same URL is returned in this \
call's result; if the user reports the browser did not open, show that URL \
to the user verbatim and ask them to complete the sign-in there.\n\
2. The call blocks (up to 5 minutes) until the browser flow completes on the \
local callback listener, is declined, or times out. Do not assume success \
before the call returns.\n\
3. On success the server reconnects and its real MCP tools replace this \
synthetic authenticate tool, becoming callable from the next model request \
in this session.\n\n\
Treat the URL as sensitive — do not modify it or strip query parameters. If \
the flow is declined, cancelled, or times out, the call returns an error; \
relay it truthfully and suggest `/mcp login {server_name}` in the TUI or \
`codewhale mcp login {server_name}` from a terminal."
    )
}

pub fn delete_oauth_tokens_for_server(name: &str, server: &McpServerConfig) -> Result<bool> {
    if server.reviewed_plugin.is_some() {
        bail!("OAuth storage is disabled for plugin-contributed MCP servers");
    }
    let Some(url) = server.url.as_deref() else {
        bail!("OAuth logout is only supported for URL-based MCP servers");
    };
    delete_oauth_tokens(name, url)
}

pub(crate) fn server_has_manual_authorization(server: &McpServerConfig) -> bool {
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

pub(crate) fn load_oauth_tokens(
    server_name: &str,
    url: &str,
) -> Result<Option<StoredMcpOAuthTokens>> {
    let secrets = codewhale_secrets::Secrets::auto_detect();
    let key = store_key(server_name, url);
    let Some(serialized) = secrets
        .get(&key)
        .with_context(|| format!("reading MCP OAuth token for '{server_name}'"))?
    else {
        return Ok(None);
    };
    let mut tokens = parse_stored_oauth_tokens(&serialized, server_name)?;
    refresh_expires_in_from_timestamp(&mut tokens);
    Ok(Some(tokens))
}

fn parse_stored_oauth_tokens(serialized: &str, server_name: &str) -> Result<StoredMcpOAuthTokens> {
    serde_json::from_str(serialized).map_err(|_| {
        anyhow!(
            "stored MCP OAuth token for '{server_name}' is not valid credential JSON; contents were omitted"
        )
    })
}

pub(crate) fn save_oauth_tokens(tokens: &StoredMcpOAuthTokens) -> Result<()> {
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
    accept_task: tokio::task::JoinHandle<()>,
}

impl Drop for CallbackServerGuard {
    fn drop(&mut self) {
        // Aborting drops the accept future and its owned listener instead of
        // leaving a detached task holding a fixed callback port indefinitely.
        self.accept_task.abort();
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
        let listener = TcpListener::bind(&bind_addr)
            .await
            .map_err(|err| anyhow!(err))?;
        let redirect_uri = resolve_redirect_uri(&listener, callback_url)?;
        let callback_id = callback_id_from_server_url(server_url)?;
        let redirect_uri = append_callback_id_to_redirect_uri(&redirect_uri, &callback_id)?;
        let callback_path = callback_path_from_redirect_uri(&redirect_uri)?;

        let (tx, rx) = oneshot::channel();
        let guard = CallbackServerGuard {
            accept_task: spawn_callback_server(listener, tx, callback_path),
        };

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

    async fn finish(self) -> Result<()> {
        self.finish_with_announce(OAuthLoginAnnounce::Terminal)
            .await
    }

    async fn finish_with_announce(mut self, announce: OAuthLoginAnnounce) -> Result<()> {
        match announce {
            OAuthLoginAnnounce::Terminal => {
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
            }
            OAuthLoginAnnounce::Tool { open_browser } => {
                // A tool call is not a terminal: nothing goes to stdout. The
                // tool result carries the URL for the model to relay; the
                // browser open is a best-effort convenience on top.
                if open_browser {
                    let _ = webbrowser::open(&self.auth_url);
                }
            }
        }

        let result = async {
            let callback = timeout(Duration::from_secs(300), &mut self.rx)
                .await
                .with_context(|| {
                    let retry_hint = match announce {
                        OAuthLoginAnnounce::Terminal => "Retry from a terminal, or use task_shell_start/background shell if an agent is running the login flow.".to_string(),
                        OAuthLoginAnnounce::Tool { .. } => format!(
                            "The user can complete the sign-in directly via `/mcp login {}` or `codewhale mcp login {}`, then this tool can be called again.",
                            self.server_name, self.server_name
                        ),
                    };
                    format!(
                        "timed out waiting for OAuth callback for MCP server '{}'. {retry_hint}",
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
            .start_authorization(scopes, redirect_uri, Some("Codewhale"))
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
    listener: TcpListener,
    tx: oneshot::Sender<CallbackResult>,
    expected_callback_path: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // The sender is wrapped in Option so we can take it on success/error
        let mut tx_opt = Some(tx);
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let path = match read_http_path(&mut stream).await {
                Some(p) => p,
                None => {
                    let _ = write_http_response(&mut stream, 400, "Invalid OAuth callback").await;
                    continue;
                }
            };
            match parse_oauth_callback(&path, &expected_callback_path) {
                CallbackOutcome::Success(callback) => {
                    let _ = write_http_response(
                        &mut stream,
                        200,
                        "Authentication complete. You may close this window.",
                    )
                    .await;
                    if let Some(tx) = tx_opt.take() {
                        let _ = tx.send(CallbackResult::Success(callback));
                    }
                    break;
                }
                CallbackOutcome::Error(error) => {
                    let msg = error.to_string();
                    let _ = write_http_response(&mut stream, 400, &msg).await;
                    if let Some(tx) = tx_opt.take() {
                        let _ = tx.send(CallbackResult::Error(error));
                    }
                    break;
                }
                CallbackOutcome::Invalid => {
                    let _ = write_http_response(&mut stream, 400, "Invalid OAuth callback").await;
                }
            }
        }
    })
}

async fn read_http_path(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    // Read until we have \r\n\r\n or exceed limit
    loop {
        match stream.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if buf.len() > 8192 {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    let request = String::from_utf8_lossy(&buf);
    let first_line = request.lines().next()?;
    // Expected: GET /callback?code=... HTTP/1.1
    let mut parts = first_line.split_whitespace();
    let _method = parts.next()?;
    let path = parts.next()?.to_string();
    Some(path)
}

async fn write_http_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    body: &str,
) -> std::io::Result<()> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
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

fn local_redirect_uri(listener: &TcpListener) -> Result<String> {
    let addr = listener.local_addr()?;
    match addr {
        std::net::SocketAddr::V4(v4) => Ok(format!("http://{}:{}/callback", v4.ip(), v4.port())),
        std::net::SocketAddr::V6(v6) => Ok(format!("http://[{}]:{}/callback", v6.ip(), v6.port())),
    }
}

fn resolve_redirect_uri(listener: &TcpListener, callback_url: Option<&str>) -> Result<String> {
    let Some(callback_url) = callback_url else {
        return local_redirect_uri(listener);
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
    use std::sync::atomic::{AtomicBool, Ordering};

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
    fn malformed_stored_oauth_diagnostic_omits_secret_contents_and_keys() {
        let secret = "cw-secret-mcp-oauth-4507";
        let serialized =
            format!(r#"{{"token_response":{{"access_token":"{secret}"}} trailing-junk}}"#);
        let error = parse_stored_oauth_tokens(&serialized, "private")
            .expect_err("malformed credential JSON must fail");
        let diagnostic = format!("{error:#}");
        assert!(!diagnostic.contains(secret), "{diagnostic}");
        assert!(!diagnostic.contains("access_token"), "{diagnostic}");
        assert!(diagnostic.contains("contents were omitted"), "{diagnostic}");
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
    fn auth_required_classifier_treats_rejected_grants_as_auth_required() {
        // A definitively rejected refresh grant is recoverable only by a
        // fresh login, so it must classify like a 401 on every surface.
        let err = anyhow!("refreshing MCP OAuth token for server wiki")
            .context("Server returned error response: invalid_grant: stale grant");
        assert!(error_looks_auth_required(&err));
        assert!(error_text_looks_auth_required(
            "wiki requires OAuth — run /mcp login wiki"
        ));
        assert!(error_text_looks_auth_required("wiki: ◆ auth required"));
        assert!(!error_text_looks_auth_required(
            "invalid_request: missing parameter"
        ));
    }

    #[test]
    fn auth_required_login_hint_names_server() {
        let hint = auth_required_login_hint("nordic-mcp");
        assert!(hint.contains("nordic-mcp"));
        assert!(hint.contains("codewhale mcp login nordic-mcp"));
        assert!(!hint.contains("/mcp auth"));
    }

    #[test]
    fn tui_reauth_hints_name_the_login_command() {
        for hint in [tui_reauth_hint(), tui_reauth_refresh_failed_hint()] {
            assert!(
                hint.contains("/mcp login <name>"),
                "OAuth recovery must name the implemented command"
            );
            assert!(
                !hint.contains("/mcp auth"),
                "OAuth recovery must not advertise a missing /mcp auth command"
            );
        }
        assert!(error_text_looks_auth_required(
            "MCP server rejected the request with 401 Unauthorized"
        ));
        assert!(!error_text_looks_auth_required("connection refused"));
    }

    #[tokio::test]
    async fn cancellable_oauth_drops_in_flight_flow_before_returning() {
        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let cancellation_token = CancellationToken::new();
        let cancel_from_task = cancellation_token.clone();
        let dropped = Arc::new(AtomicBool::new(false));
        let flow_dropped = Arc::clone(&dropped);
        let pending_flow = async move {
            let _guard = DropFlag(flow_dropped);
            std::future::pending::<Result<()>>().await
        };
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel_from_task.cancel();
        });

        let error = run_cancellable_oauth(&cancellation_token, pending_flow)
            .await
            .expect_err("cancellation should stop the pending OAuth flow");

        assert!(error.to_string().contains("OAuth login was cancelled"));
        assert!(
            dropped.load(Ordering::SeqCst),
            "the callback-server guard must be dropped before cancellation returns"
        );
    }

    #[tokio::test]
    async fn callback_guard_aborts_accept_task_and_releases_fixed_port() -> Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let (tx, _rx) = oneshot::channel();
        let guard = CallbackServerGuard {
            accept_task: spawn_callback_server(listener, tx, "/callback/test".to_string()),
        };

        drop(guard);

        let rebound = timeout(Duration::from_secs(1), async {
            loop {
                match TcpListener::bind(addr).await {
                    Ok(listener) => break Ok(listener),
                    Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                        tokio::task::yield_now().await;
                    }
                    Err(err) => break Err(err),
                }
            }
        })
        .await
        .context("callback listener did not release its fixed port")??;
        drop(rebound);
        Ok(())
    }
}
