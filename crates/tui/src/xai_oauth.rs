//! xAI / Grok OAuth credential loading, refresh, and device-code login.
//!
//! Two paths, matching [#4257](https://github.com/Hmbown/CodeWhale/issues/4257):
//!
//! 1. **Delegate-login** — reuse the official Grok CLI token file at
//!    `~/.grok/auth.json` (or `$GROK_HOME/auth.json` / `$GROK_AUTH_PATH`).
//! 2. **Native device-code** — request a code from `auth.x.ai`, print the
//!    verification URL + user code, poll the token endpoint, and write tokens
//!    back to the Grok CLI auth file shape (so both tools stay compatible).
//!
//! Access tokens are sent as `Authorization: Bearer` on the OpenAI-compatible
//! xAI Chat Completions route (`https://api.x.ai/v1`). Token values are never
//! logged.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Official Grok CLI public OIDC client id (public client; no secret).
pub const GROK_OIDC_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
/// Default issuer / authorization server.
pub const XAI_OIDC_ISSUER: &str = "https://auth.x.ai";
/// User-principal scopes requested by device-code login.
///
/// Although xAI advertises `team:read` in discovery metadata, its device-code
/// endpoint rejects that scope for User principals. Keep team enrichment out of
/// the user-principal login request.
pub const DEFAULT_SCOPES: &str = "openid profile email offline_access api:access grok-cli:access";
const REFRESH_SKEW_SECS: i64 = 60;
const DEVICE_POLL_DEFAULT_SECS: u64 = 5;
const DEVICE_POLL_MAX_SECS: u64 = 900;
/// RFC 8628 §3.5: on `slow_down` the polling interval increases by 5 seconds.
const DEVICE_SLOW_DOWN_STEP_SECS: u64 = 5;
const OAUTH_RESPONSE_BODY_LIMIT: u64 = 64 * 1024;
const OAUTH_ERROR_DETAIL_LIMIT: usize = 256;

/// One entry in `~/.grok/auth.json` (map key = `{issuer}::{client_id}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrokAuthEntry {
    /// Access token (JWT). Field name matches the Grok CLI (`key`).
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// RFC3339 expiry timestamp written by the Grok CLI.
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub oidc_issuer: Option<String>,
    #[serde(default)]
    pub oidc_client_id: Option<String>,
    #[serde(default)]
    pub auth_mode: Option<String>,
    /// Preserve unknown CLI fields on rewrite.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Token endpoint response (device-code exchange or refresh).
#[derive(Debug, Clone, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceCodeResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Clone)]
struct DeviceCodeGrant {
    device_code: String,
    user_code: String,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct OidcDiscoveryResponse {
    issuer: Option<String>,
    device_authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceOauthEndpoints {
    device_authorization_endpoint: String,
    token_endpoint: String,
}

/// Resolved bearer credential ready for API use.
#[derive(Debug, Clone)]
pub struct XaiOAuthCredentials {
    pub access_token: String,
    #[allow(dead_code)]
    pub refresh_token: Option<String>,
    #[allow(dead_code)]
    pub expires_at: Option<String>,
    #[allow(dead_code)]
    pub issuer: String,
    #[allow(dead_code)]
    pub client_id: String,
}

/// Whether `[providers.xai] auth_mode` selects the OAuth path.
#[must_use]
pub fn auth_mode_uses_xai_oauth(mode: &str) -> bool {
    matches!(
        normalize_auth_mode(mode).as_str(),
        "oauth"
            | "xai_oauth"
            | "xai"
            | "grok"
            | "grok_oauth"
            | "grok_cli"
            | "device"
            | "device_code"
            | "device_auth"
    )
}

fn normalize_auth_mode(mode: &str) -> String {
    mode.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

/// Resolve the Grok CLI auth file path.
///
/// Priority:
/// 1. `GROK_AUTH_PATH` / `XAI_AUTH_PATH`
/// 2. `$GROK_HOME/auth.json`
/// 3. `~/.grok/auth.json`
#[must_use]
pub fn auth_file_path() -> PathBuf {
    for key in ["GROK_AUTH_PATH", "XAI_AUTH_PATH"] {
        if let Ok(path) = std::env::var(key) {
            let p = PathBuf::from(path.trim());
            if !p.as_os_str().is_empty() {
                return p;
            }
        }
    }
    if let Ok(home) = std::env::var("GROK_HOME") {
        let p = PathBuf::from(home.trim());
        if !p.as_os_str().is_empty() {
            return p.join("auth.json");
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
        .join("auth.json")
}

#[must_use]
pub fn credentials_present() -> bool {
    auth_file_path().exists()
}

/// Prompt-free structural check for Grok/xAI OAuth material. Never refreshes
/// or writes: a fresh access token or a non-empty refresh token is enough to
/// keep the route selectable, while malformed/empty files count as missing.
#[must_use]
pub fn credentials_valid() -> bool {
    let path = auth_file_path();
    let Ok(mut file) = load_auth_file(&path) else {
        return false;
    };
    let Some((_, entry)) = select_entry(&mut file) else {
        return false;
    };
    entry_access_token_is_fresh(&entry)
        || entry
            .refresh_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
}

/// Load + refresh OAuth credentials from the Grok CLI auth file.
pub fn get_access_token() -> Result<String> {
    Ok(get_credentials()?.access_token)
}

pub fn get_credentials() -> Result<XaiOAuthCredentials> {
    let path = auth_file_path();
    if !path.exists() {
        bail!("{}", missing_auth_message());
    }
    let mut file = load_auth_file(&path)?;
    let (scope, mut entry) = select_entry(&mut file).ok_or_else(|| {
        anyhow::anyhow!(
            "xAI OAuth credentials at {} have no usable entry. Run `grok login` \
             or `codewhale auth xai-device` (device-code).",
            path.display()
        )
    })?;

    if entry_access_token_is_fresh(&entry) {
        let token = entry
            .key
            .clone()
            .filter(|t| !t.trim().is_empty())
            .context("xAI OAuth access token is empty")?;
        return Ok(credentials_from_entry(scope, &entry, token));
    }

    let refresh = entry
        .refresh_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .context(
            "xAI OAuth access token expired and no refresh_token is stored. \
             Run `grok login` or `codewhale auth xai-device` again.",
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

    let refreshed = refresh_access_token(&issuer, &client_id, refresh)?;
    apply_token_response(&mut entry, &issuer, &client_id, &refreshed)?;
    file.insert(scope.clone(), entry.clone());
    write_auth_file(&path, &file)?;

    let token = entry
        .key
        .clone()
        .filter(|t| !t.trim().is_empty())
        .context("xAI OAuth refresh returned an empty access token")?;
    Ok(credentials_from_entry(scope, &entry, token))
}

/// Interactive device-code login. Prints verification URL + user code to
/// `stderr`, polls until approved, and writes `~/.grok/auth.json`.
///
/// Public residual entry point for CLI/TUI wiring (`codewhale auth` /
/// slash command). Call from a headless or TUI surface that can print the
/// verification URL.
pub fn device_code_login() -> Result<XaiOAuthCredentials> {
    let issuer = std::env::var("GROK_OIDC_ISSUER")
        .or_else(|_| std::env::var("XAI_OIDC_ISSUER"))
        .unwrap_or_else(|_| XAI_OIDC_ISSUER.to_string());
    let client_id = std::env::var("GROK_OIDC_CLIENT_ID")
        .or_else(|_| std::env::var("XAI_OIDC_CLIENT_ID"))
        .unwrap_or_else(|_| GROK_OIDC_CLIENT_ID.to_string());
    let scopes = std::env::var("GROK_OIDC_SCOPES")
        .or_else(|_| std::env::var("XAI_OIDC_SCOPES"))
        .unwrap_or_else(|_| DEFAULT_SCOPES.to_string());
    let auth_path = auth_file_path();
    let open_browser = std::env::var_os("CODEWHALE_XAI_OAUTH_NO_BROWSER").is_none();

    device_code_login_with(&issuer, &client_id, &scopes, &auth_path, open_browser)
}

fn device_code_login_with(
    issuer: &str,
    client_id: &str,
    scopes: &str,
    auth_path: &Path,
    open_browser: bool,
) -> Result<XaiOAuthCredentials> {
    let endpoints = resolve_device_oauth_endpoints(issuer);
    let device = request_device_code(&endpoints.device_authorization_endpoint, client_id, scopes)?;
    let verify = device
        .verification_uri_complete
        .clone()
        .or(device.verification_uri.clone())
        .unwrap_or_else(|| format!("{issuer}/device"));

    eprintln!("xAI device-code login");
    eprintln!("  Open:  {verify}");
    eprintln!("  Code:  {}", device.user_code);
    eprintln!("Waiting for approval in the browser… (Ctrl+C to abort)");
    if open_browser && let Err(err) = webbrowser::open(&verify) {
        eprintln!("Could not open the browser automatically: {err}");
    }

    let mut interval = device.interval.unwrap_or(DEVICE_POLL_DEFAULT_SECS).max(1);
    let deadline = std::time::Instant::now()
        + Duration::from_secs(device.expires_in.unwrap_or(DEVICE_POLL_MAX_SECS).max(30));

    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            bail!(
                "xAI device-code authorization timed out. Re-run device login \
                 and approve the code before it expires."
            );
        }
        // Never sleep past the code's expiry, even after slow_down backoff.
        thread::sleep(Duration::from_secs(interval).min(deadline - now));
        match poll_device_token(&endpoints.token_endpoint, client_id, &device.device_code) {
            Ok(token) => {
                let mut file = if auth_path.exists() {
                    load_auth_file(auth_path).unwrap_or_default()
                } else {
                    BTreeMap::new()
                };
                let scope = format!("{issuer}::{client_id}");
                let mut entry = file.remove(&scope).unwrap_or(GrokAuthEntry {
                    key: None,
                    refresh_token: None,
                    expires_at: None,
                    oidc_issuer: Some(issuer.to_string()),
                    oidc_client_id: Some(client_id.to_string()),
                    auth_mode: Some("oidc".to_string()),
                    extra: BTreeMap::new(),
                });
                apply_token_response(&mut entry, issuer, client_id, &token)?;
                file.insert(scope.clone(), entry.clone());
                if let Some(parent) = auth_path.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("creating xAI OAuth auth directory {}", parent.display())
                    })?;
                }
                write_auth_file(auth_path, &file)?;
                let access = entry
                    .key
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .context("xAI device-code login returned an empty access token")?;
                eprintln!(
                    "Signed in. Tokens stored at {} (mode 0600).",
                    auth_path.display()
                );
                return Ok(credentials_from_entry(scope, &entry, access));
            }
            Err(err) => {
                let msg = err.to_string();
                match device_poll_backoff(interval, &msg) {
                    Some(next_interval) => {
                        interval = next_interval;
                        continue;
                    }
                    None => return Err(err),
                }
            }
        }
    }
}

#[must_use]
pub fn missing_auth_message() -> String {
    format!(
        "xAI OAuth credentials not found.\n\
         Options:\n\
         1. Run `grok login` (or `grok login --device-auth`) and set \
         [providers.xai] auth_mode = \"oauth\"\n\
         2. Run device-code login, then set auth_mode = \"oauth\"\n\
         3. Or use API-key auth: export XAI_API_KEY=... / \
         codewhale auth set --provider xai\n\
         Looked for: {}",
        auth_file_path().display()
    )
}

// ── internals ──────────────────────────────────────────────────────────────

type AuthFile = BTreeMap<String, GrokAuthEntry>;

fn load_auth_file(path: &Path) -> Result<AuthFile> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading xAI/Grok auth file {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing xAI/Grok auth file {}", path.display()))?;
    let obj = value.as_object().with_context(|| {
        format!(
            "xAI/Grok auth file {} must be a JSON object of scope → entry",
            path.display()
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        match serde_json::from_value::<GrokAuthEntry>(v.clone()) {
            Ok(entry) => {
                out.insert(k.clone(), entry);
            }
            Err(err) => {
                tracing::warn!(
                    target: "codewhale::xai_oauth",
                    scope = %k,
                    error = %err,
                    "skipping unreadable xAI auth entry"
                );
            }
        }
    }
    Ok(out)
}

fn write_auth_file(path: &Path, file: &AuthFile) -> Result<()> {
    let serialized =
        serde_json::to_vec_pretty(file).context("serializing xAI OAuth credentials")?;
    crate::utils::write_atomic(path, &serialized)
        .with_context(|| format!("writing xAI OAuth credentials to {}", path.display()))?;
    #[cfg(unix)]
    if let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(
            target: "codewhale::xai_oauth",
            path = %path.display(),
            error = %err,
            "could not enforce 0o600 on xAI OAuth credentials; relying on host ACLs"
        );
    }
    Ok(())
}

fn select_entry(file: &mut AuthFile) -> Option<(String, GrokAuthEntry)> {
    // Prefer the official Grok CLI client id scope when present.
    let preferred_suffix = format!("::{GROK_OIDC_CLIENT_ID}");
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

fn entry_has_usable_secret(entry: &GrokAuthEntry) -> bool {
    entry.key.as_deref().is_some_and(|t| !t.trim().is_empty())
        || entry
            .refresh_token
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
}

fn entry_access_token_is_fresh(entry: &GrokAuthEntry) -> bool {
    let Some(token) = entry.key.as_deref().filter(|t| !t.trim().is_empty()) else {
        return false;
    };
    if let Some(exp) = entry.expires_at.as_deref().and_then(parse_rfc3339_secs) {
        let now = now_unix_secs().unwrap_or(0);
        return exp - now > REFRESH_SKEW_SECS;
    }
    // Fall back to JWT exp claim when expires_at is missing.
    match jwt_expiry_seconds(token) {
        Some(exp) => {
            let now = now_unix_secs().unwrap_or(0) as u64;
            (exp as i64) - (now as i64) > REFRESH_SKEW_SECS
        }
        // Unknown expiry → treat as stale so refresh runs.
        None => false,
    }
}

fn credentials_from_entry(
    scope: String,
    entry: &GrokAuthEntry,
    access_token: String,
) -> XaiOAuthCredentials {
    XaiOAuthCredentials {
        access_token,
        refresh_token: entry.refresh_token.clone(),
        expires_at: entry.expires_at.clone(),
        issuer: entry
            .oidc_issuer
            .clone()
            .unwrap_or_else(|| issuer_from_scope(&scope)),
        client_id: entry
            .oidc_client_id
            .clone()
            .unwrap_or_else(|| client_id_from_scope(&scope)),
    }
}

fn issuer_from_scope(scope: &str) -> String {
    scope
        .split_once("::")
        .map(|(issuer, _)| issuer.to_string())
        .unwrap_or_else(|| XAI_OIDC_ISSUER.to_string())
}

fn client_id_from_scope(scope: &str) -> String {
    scope
        .split_once("::")
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| GROK_OIDC_CLIENT_ID.to_string())
}

fn apply_token_response(
    entry: &mut GrokAuthEntry,
    issuer: &str,
    client_id: &str,
    token: &TokenResponse,
) -> Result<()> {
    let access = token
        .access_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .context("token response missing access_token")?;
    entry.key = Some(access.to_string());
    if let Some(rt) = token
        .refresh_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        entry.refresh_token = Some(rt.to_string());
    }
    entry.oidc_issuer = Some(issuer.to_string());
    entry.oidc_client_id = Some(client_id.to_string());
    entry.auth_mode = Some("oidc".to_string());
    if let Some(expires_in) = token.expires_in {
        entry.expires_at = Some(rfc3339_from_now(expires_in));
    } else if let Some(exp) = jwt_expiry_seconds(access) {
        entry.expires_at = Some(rfc3339_from_unix(exp as i64));
    }
    Ok(())
}

fn fallback_device_oauth_endpoints(issuer: &str) -> DeviceOauthEndpoints {
    let issuer = issuer.trim_end_matches('/');
    DeviceOauthEndpoints {
        device_authorization_endpoint: format!("{issuer}/oauth2/device/code"),
        token_endpoint: format!("{issuer}/oauth2/token"),
    }
}

/// RFC 8628 §3.5 polling update for a failed token poll.
///
/// Returns the interval to use for the next poll when polling should
/// continue: `authorization_pending` keeps the current interval, `slow_down`
/// increases it by [`DEVICE_SLOW_DOWN_STEP_SECS`]. Any other error is
/// terminal and returns `None`.
fn device_poll_backoff(interval: u64, error: &str) -> Option<u64> {
    if error.contains("authorization_pending") {
        Some(interval)
    } else if error.contains("slow_down") {
        Some(interval + DEVICE_SLOW_DOWN_STEP_SECS)
    } else {
        None
    }
}

fn resolve_device_oauth_endpoints(issuer: &str) -> DeviceOauthEndpoints {
    match discover_device_oauth_endpoints(issuer) {
        Ok(endpoints) => endpoints,
        Err(err) => {
            let fallback = fallback_device_oauth_endpoints(issuer);
            tracing::warn!(
                target: "codewhale::xai_oauth",
                error = %err,
                device_authorization_endpoint = %fallback.device_authorization_endpoint,
                token_endpoint = %fallback.token_endpoint,
                "xAI OIDC discovery failed; using documented endpoint fallback"
            );
            fallback
        }
    }
}

fn discover_device_oauth_endpoints(issuer: &str) -> Result<DeviceOauthEndpoints> {
    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let client = crate::tls::reqwest_blocking_client_builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("Failed to build xAI OIDC discovery client")?;
    let response = client
        .get(&discovery_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .context("xAI OIDC discovery request failed")?;
    let (status, discovery): (_, OidcDiscoveryResponse) =
        parse_oauth_json_response(response, "xAI OIDC discovery")?;
    if !status.is_success() {
        bail!("xAI OIDC discovery failed with HTTP {status}");
    }
    validate_discovered_issuer(discovery.issuer, issuer)?;

    Ok(DeviceOauthEndpoints {
        device_authorization_endpoint: validate_discovered_oauth_endpoint(
            discovery.device_authorization_endpoint,
            "device_authorization_endpoint",
            issuer,
        )?,
        token_endpoint: validate_discovered_oauth_endpoint(
            discovery.token_endpoint,
            "token_endpoint",
            issuer,
        )?,
    })
}

fn validate_discovered_issuer(discovered: Option<String>, expected: &str) -> Result<()> {
    let discovered = discovered
        .as_deref()
        .map(str::trim)
        .filter(|issuer| !issuer.is_empty())
        .context("xAI OIDC discovery missing issuer")?;
    if discovered.trim_end_matches('/') != expected.trim_end_matches('/') {
        bail!("xAI OIDC discovery issuer does not match the requested issuer");
    }
    Ok(())
}

fn validate_discovered_oauth_endpoint(
    endpoint: Option<String>,
    field: &str,
    issuer: &str,
) -> Result<String> {
    let endpoint = endpoint
        .as_deref()
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .with_context(|| format!("xAI OIDC discovery missing {field}"))?;
    let parsed = reqwest::Url::parse(endpoint)
        .with_context(|| format!("xAI OIDC discovery returned an invalid {field}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("xAI OIDC discovery returned unsupported {field} scheme");
    }
    let issuer = reqwest::Url::parse(issuer).context("xAI OIDC issuer is not a valid URL")?;
    if issuer.scheme() == "https" && parsed.scheme() != "https" {
        bail!("xAI OIDC discovery attempted to downgrade {field} from HTTPS");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("xAI OIDC discovery returned credentials in {field}");
    }
    Ok(endpoint.to_string())
}

fn parse_oauth_json_response<T: DeserializeOwned>(
    response: reqwest::blocking::Response,
    operation: &str,
) -> Result<(reqwest::StatusCode, T)> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("missing")
        .to_string();
    let mut reader = response.take(OAUTH_RESPONSE_BODY_LIMIT + 1);
    let mut body = Vec::new();
    reader
        .read_to_end(&mut body)
        .with_context(|| format!("reading {operation} response"))?;
    let truncated = body.len() as u64 > OAUTH_RESPONSE_BODY_LIMIT;
    if truncated {
        body.truncate(OAUTH_RESPONSE_BODY_LIMIT as usize);
    }

    let parsed = serde_json::from_slice(&body).map_err(|_| {
        let limit = if truncated {
            " (body exceeded the 64 KiB diagnostic limit)"
        } else {
            ""
        };
        anyhow::anyhow!(
            "{operation} returned HTTP {status} with content type {content_type}; expected JSON{limit}"
        )
    })?;
    Ok((status, parsed))
}

fn oauth_failure_detail(
    error: Option<&str>,
    description: Option<&str>,
    status: reqwest::StatusCode,
) -> String {
    let mut code = bounded_oauth_error_text(error.unwrap_or("request_failed"));
    if code.is_empty() {
        code = "request_failed".to_string();
    }
    let description = description
        .map(bounded_oauth_error_text)
        .filter(|description| !description.is_empty() && description != &code);
    match description {
        Some(description) => format!("{code}: {description}; HTTP {status}"),
        None => format!("{code}; HTTP {status}"),
    }
}

fn bounded_oauth_error_text(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len().min(OAUTH_ERROR_DETAIL_LIMIT));
    let mut previous_was_space = false;
    let mut written = 0;
    for character in raw.chars() {
        let character = if character.is_whitespace() {
            ' '
        } else if character.is_control() {
            continue;
        } else {
            character
        };
        if character == ' ' && previous_was_space {
            continue;
        }
        if written == OAUTH_ERROR_DETAIL_LIMIT {
            break;
        }
        output.push(character);
        previous_was_space = character == ' ';
        written += 1;
    }
    output.trim().to_string()
}

fn refresh_access_token(
    issuer: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse> {
    let token_endpoint = resolve_device_oauth_endpoints(issuer).token_endpoint;
    let client = crate::tls::reqwest_blocking_client_builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("Failed to build xAI OAuth refresh client")?;
    let params = [
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    let response = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .context("xAI OAuth refresh request failed")?;
    let (status, body): (_, TokenResponse) =
        parse_oauth_json_response(response, "xAI OAuth refresh")?;
    if !status.is_success() || body.error.is_some() {
        // Refresh requests carry a credential. Do not echo a server-provided
        // description that could reflect the submitted refresh token.
        let err = oauth_failure_detail(body.error.as_deref(), None, status);
        bail!(
            "xAI OAuth refresh failed ({err}). Run `grok login` or device-code login again. \
             If SuperGrok OAuth returns HTTP 403, use XAI_API_KEY instead."
        );
    }
    Ok(body)
}

fn request_device_code(
    device_authorization_endpoint: &str,
    client_id: &str,
    scopes: &str,
) -> Result<DeviceCodeGrant> {
    let client = crate::tls::reqwest_blocking_client_builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("Failed to build xAI device-code client")?;
    let params = [("client_id", client_id), ("scope", scopes)];
    let response = client
        .post(device_authorization_endpoint)
        .form(&params)
        .send()
        .context("xAI device-code request failed")?;
    let (status, body): (_, DeviceCodeResponse) =
        parse_oauth_json_response(response, "xAI device-code request")?;
    if !status.is_success() || body.error.is_some() {
        let err = oauth_failure_detail(
            body.error.as_deref(),
            body.error_description.as_deref(),
            status,
        );
        bail!("xAI device-code request failed ({err})");
    }
    let device_code = body
        .device_code
        .filter(|value| !value.trim().is_empty())
        .context("xAI device-code response missing device_code")?;
    let user_code = body
        .user_code
        .filter(|value| !value.trim().is_empty())
        .context("xAI device-code response missing user_code")?;
    Ok(DeviceCodeGrant {
        device_code,
        user_code,
        verification_uri: body.verification_uri,
        verification_uri_complete: body.verification_uri_complete,
        expires_in: body.expires_in,
        interval: body.interval,
    })
}

fn poll_device_token(
    token_endpoint: &str,
    client_id: &str,
    device_code: &str,
) -> Result<TokenResponse> {
    let client = crate::tls::reqwest_blocking_client_builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("Failed to build xAI device-code poll client")?;
    let params = [
        ("client_id", client_id),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("device_code", device_code),
    ];
    let response = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .context("xAI device-code token poll failed")?;
    let (status, body): (_, TokenResponse) =
        parse_oauth_json_response(response, "xAI device-code token exchange")?;
    if let Some(err) = body.error.as_deref() {
        if matches!(err, "authorization_pending" | "slow_down") {
            bail!("{err}");
        }
        // Poll requests carry the device credential. Keep diagnostics to the
        // standard error code and HTTP status rather than echoing descriptions.
        let detail = oauth_failure_detail(Some(err), None, status);
        bail!("xAI device-code token exchange failed: {detail}");
    }
    if !status.is_success() {
        let detail = oauth_failure_detail(None, None, status);
        bail!("xAI device-code token exchange failed: {detail}");
    }
    Ok(body)
}

fn jwt_expiry_seconds(token: &str) -> Option<u64> {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    claims.get("exp")?.as_u64()
}

fn now_unix_secs() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

fn parse_rfc3339_secs(raw: &str) -> Option<i64> {
    // Prefer chrono when available for full RFC3339; fall back to simple UTC forms.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.timestamp());
    }
    // e.g. 2026-07-09T12:00:00Z
    let trimmed = raw.trim().trim_end_matches('Z');
    let (date, time) = trimmed.split_once('T')?;
    let mut d = date.split('-');
    let y: i32 = d.next()?.parse().ok()?;
    let m: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let time = time.split('+').next()?.split('-').next()?;
    let mut t = time.split(':');
    let hh: u32 = t.next()?.parse().ok()?;
    let mm: u32 = t.next()?.parse().ok()?;
    let ss: u32 = t
        .next()
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let ndt = chrono::NaiveDate::from_ymd_opt(y, m, day)?.and_hms_opt(hh, mm, ss)?;
    Some(ndt.and_utc().timestamp())
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

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn auth_mode_accepts_oauth_aliases() {
        for mode in [
            "oauth",
            "xai_oauth",
            "XAI-OAuth",
            "grok",
            "grok_cli",
            "device_code",
            "device-auth",
        ] {
            assert!(
                auth_mode_uses_xai_oauth(mode),
                "expected oauth mode: {mode}"
            );
        }
        assert!(!auth_mode_uses_xai_oauth("api_key"));
        assert!(!auth_mode_uses_xai_oauth("keyring"));
    }

    #[test]
    fn loads_fresh_token_from_grok_auth_json() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        let future = rfc3339_from_now(3600);
        let scope = format!("{XAI_OIDC_ISSUER}::{GROK_OIDC_CLIENT_ID}");
        let file = serde_json::json!({
            scope: {
                "key": "test-access-token",
                "refresh_token": "test-refresh",
                "expires_at": future,
                "oidc_issuer": XAI_OIDC_ISSUER,
                "oidc_client_id": GROK_OIDC_CLIENT_ID,
                "auth_mode": "oidc"
            }
        });
        fs::write(&path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();
        // SAFETY: serialized by the process-wide test environment lock;
        // restored below.
        unsafe {
            std::env::set_var("GROK_AUTH_PATH", &path);
        }
        let result = get_credentials();
        unsafe {
            std::env::remove_var("GROK_AUTH_PATH");
        }
        let creds = result.expect("load");
        assert_eq!(creds.access_token, "test-access-token");
        assert_eq!(creds.client_id, GROK_OIDC_CLIENT_ID);
    }

    #[test]
    fn missing_file_message_mentions_oauth_paths() {
        let _guard = crate::test_support::lock_test_env();
        let msg = missing_auth_message();
        assert!(msg.contains("xAI OAuth credentials not found"), "{msg}");
        assert!(msg.contains("auth_mode"), "{msg}");
        assert!(msg.contains("XAI_API_KEY"), "{msg}");
    }

    #[test]
    fn parse_rfc3339_accepts_zulu() {
        let ts = parse_rfc3339_secs("2026-07-09T12:00:00.000Z").expect("parse");
        assert!(ts > 0);
    }

    #[test]
    fn device_code_constants_match_discovery_shape() {
        assert_eq!(
            DEFAULT_SCOPES.split_whitespace().collect::<Vec<_>>(),
            [
                "openid",
                "profile",
                "email",
                "offline_access",
                "api:access",
                "grok-cli:access",
            ]
        );
        assert_eq!(XAI_OIDC_ISSUER, "https://auth.x.ai");
        assert_eq!(GROK_OIDC_CLIENT_ID.len(), 36);
        // Keep device_code_login referenced so the residual entry point stays linked.
        let _ = device_code_login as fn() -> Result<XaiOAuthCredentials>;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovers_device_authorization_and_token_endpoints() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": server.uri(),
                "device_authorization_endpoint": format!("{}/oauth2/device-advertised", server.uri()),
                "token_endpoint": format!("{}/oauth2/token-advertised", server.uri())
            })))
            .expect(1)
            .mount(&server)
            .await;

        let endpoints = tokio::task::block_in_place(|| {
            discover_device_oauth_endpoints(&server.uri()).expect("discover endpoints")
        });

        assert_eq!(
            endpoints,
            DeviceOauthEndpoints {
                device_authorization_endpoint: format!("{}/oauth2/device-advertised", server.uri()),
                token_endpoint: format!("{}/oauth2/token-advertised", server.uri()),
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refresh_uses_discovered_token_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": server.uri(),
                "device_authorization_endpoint": format!("{}/oauth2/device-advertised", server.uri()),
                "token_endpoint": format!("{}/oauth2/token-advertised", server.uri())
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token-advertised"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "refreshed-access",
                "refresh_token": "rotated-refresh",
                "expires_in": 3600
            })))
            .expect(1)
            .mount(&server)
            .await;

        let token = tokio::task::block_in_place(|| {
            refresh_access_token(&server.uri(), GROK_OIDC_CLIENT_ID, "refresh-secret")
                .expect("refresh token")
        });

        assert_eq!(token.access_token.as_deref(), Some("refreshed-access"));
        assert_eq!(token.refresh_token.as_deref(), Some("rotated-refresh"));
    }

    #[test]
    fn https_discovery_rejects_plaintext_endpoint_downgrade() {
        let error = validate_discovered_oauth_endpoint(
            Some("http://auth.x.ai/oauth2/device/code".to_string()),
            "device_authorization_endpoint",
            XAI_OIDC_ISSUER,
        )
        .expect_err("HTTPS issuer must reject an HTTP endpoint");

        assert!(error.to_string().contains("downgrade"), "{error}");
    }

    #[test]
    fn discovery_rejects_mismatched_issuer() {
        let error = validate_discovered_issuer(
            Some("https://attacker.example".to_string()),
            XAI_OIDC_ISSUER,
        )
        .expect_err("discovery issuer must bind to the request issuer");

        assert!(error.to_string().contains("does not match"), "{error}");
    }

    #[test]
    fn oauth_error_details_collapse_control_whitespace() {
        let detail = oauth_failure_detail(
            Some("invalid_scope\nforged"),
            Some("bad\t scope\r\nnext line"),
            reqwest::StatusCode::BAD_REQUEST,
        );

        assert!(
            !detail
                .chars()
                .any(|character| matches!(character, '\n' | '\r' | '\t')),
            "{detail}"
        );
        assert!(detail.contains("invalid_scope forged"), "{detail}");
        assert!(detail.contains("bad scope next line"), "{detail}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovery_failure_uses_documented_endpoint_fallback() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_body_raw("<html>temporarily unavailable</html>", "text/html"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let endpoints =
            tokio::task::block_in_place(|| resolve_device_oauth_endpoints(&server.uri()));

        assert_eq!(
            endpoints,
            DeviceOauthEndpoints {
                device_authorization_endpoint: format!("{}/oauth2/device/code", server.uri()),
                token_endpoint: format!("{}/oauth2/token", server.uri()),
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn current_device_endpoint_surfaces_structured_invalid_scope() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_scope",
                "error_description": "Scope 'team:read' is not valid for User principals"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let error = tokio::task::block_in_place(|| {
            request_device_code(
                &format!("{}/oauth2/device/code", server.uri()),
                GROK_OIDC_CLIENT_ID,
                "openid team:read",
            )
            .expect_err("invalid scope must fail")
        });
        let message = error.to_string();

        assert!(message.contains("invalid_scope"), "{message}");
        assert!(message.contains("team:read"), "{message}");
        assert!(message.contains("HTTP 400"), "{message}");
        assert!(!message.contains("missing device_code"), "{message}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn device_code_request_reports_non_json_without_echoing_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_raw("<html>private-upstream-detail</html>", "text/html"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let error = tokio::task::block_in_place(|| {
            request_device_code(
                &format!("{}/oauth2/device", server.uri()),
                GROK_OIDC_CLIENT_ID,
                DEFAULT_SCOPES,
            )
            .expect_err("non-JSON response must fail")
        });
        let message = error.to_string();

        assert!(message.contains("HTTP 404"), "{message}");
        assert!(message.contains("text/html"), "{message}");
        assert!(message.contains("expected JSON"), "{message}");
        assert!(!message.contains("private-upstream-detail"), "{message}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn device_code_login_exchanges_and_persists_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": server.uri(),
                "device_authorization_endpoint": format!("{}/oauth2/device-advertised", server.uri()),
                "token_endpoint": format!("{}/oauth2/token-advertised", server.uri())
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device-advertised"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains(format!(
                "client_id={GROK_OIDC_CLIENT_ID}"
            )))
            .and(body_string_contains(
                "scope=openid+profile+email+offline_access+api%3Aaccess+grok-cli%3Aaccess",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "device-token",
                "user_code": "CW-TEST",
                "verification_uri": format!("{}/verify", server.uri()),
                "expires_in": 60,
                "interval": 1
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token-advertised"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
            ))
            .and(body_string_contains(format!(
                "client_id={GROK_OIDC_CLIENT_ID}"
            )))
            .and(body_string_contains("device_code=device-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "test-xai-access",
                "refresh_token": "test-xai-refresh",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let auth_path = dir.path().join("grok-auth.json");
        let result = tokio::task::block_in_place(|| {
            device_code_login_with(
                &server.uri(),
                GROK_OIDC_CLIENT_ID,
                DEFAULT_SCOPES,
                &auth_path,
                false,
            )
        });

        let credentials = result.expect("device login");
        assert_eq!(credentials.access_token, "test-xai-access");
        assert_eq!(
            credentials.refresh_token.as_deref(),
            Some("test-xai-refresh")
        );
        let persisted = fs::read_to_string(&auth_path).expect("persisted auth file");
        assert!(persisted.contains("test-xai-access"));
        assert!(persisted.contains("test-xai-refresh"));
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&auth_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn device_poll_backoff_follows_rfc8628() {
        // authorization_pending keeps the current interval.
        assert_eq!(device_poll_backoff(5, "authorization_pending"), Some(5));
        // slow_down increases the interval by 5 seconds (RFC 8628 §3.5).
        assert_eq!(
            device_poll_backoff(5, "slow_down"),
            Some(5 + DEVICE_SLOW_DOWN_STEP_SECS)
        );
        // Terminal errors stop polling.
        assert_eq!(device_poll_backoff(5, "access_denied"), None);
        assert_eq!(device_poll_backoff(5, "expired_token"), None);
    }

    #[test]
    fn apply_token_response_sets_expiry_from_expires_in() {
        let mut entry = GrokAuthEntry {
            key: None,
            refresh_token: None,
            expires_at: None,
            oidc_issuer: None,
            oidc_client_id: None,
            auth_mode: None,
            extra: BTreeMap::new(),
        };
        let token = TokenResponse {
            access_token: Some("fresh-access".to_string()),
            refresh_token: Some("fresh-refresh".to_string()),
            expires_in: Some(3600),
            error: None,
        };
        let before = now_unix_secs().expect("clock");

        apply_token_response(&mut entry, XAI_OIDC_ISSUER, GROK_OIDC_CLIENT_ID, &token)
            .expect("apply token");

        assert_eq!(entry.key.as_deref(), Some("fresh-access"));
        assert_eq!(entry.refresh_token.as_deref(), Some("fresh-refresh"));
        let expires_at = entry
            .expires_at
            .as_deref()
            .and_then(parse_rfc3339_secs)
            .expect("expires_at set from expires_in");
        let after = now_unix_secs().expect("clock");
        assert!(
            expires_at >= before + 3600,
            "{expires_at} < {before} + 3600"
        );
        assert!(expires_at <= after + 3600, "{expires_at} > {after} + 3600");
    }

    #[test]
    fn apply_token_response_rejects_missing_access_token() {
        let mut entry = GrokAuthEntry {
            key: None,
            refresh_token: None,
            expires_at: None,
            oidc_issuer: None,
            oidc_client_id: None,
            auth_mode: None,
            extra: BTreeMap::new(),
        };
        let token = TokenResponse {
            access_token: None,
            refresh_token: None,
            expires_in: None,
            error: None,
        };

        let error = apply_token_response(&mut entry, XAI_OIDC_ISSUER, GROK_OIDC_CLIENT_ID, &token)
            .expect_err("missing access_token must fail");

        assert!(
            error.to_string().contains("missing access_token"),
            "{error}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn device_code_login_polls_through_pending_and_slow_down() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": server.uri(),
                "device_authorization_endpoint": format!("{}/oauth2/device-advertised", server.uri()),
                "token_endpoint": format!("{}/oauth2/token-advertised", server.uri())
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device-advertised"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "device-token",
                "user_code": "CW-TEST",
                "verification_uri": format!("{}/verify", server.uri()),
                "expires_in": 60,
                "interval": 1
            })))
            .expect(1)
            .mount(&server)
            .await;
        // wiremock matches mocks in mount order, so mount the one-shot
        // transient-error responses before the terminal success response:
        // poll 1 -> authorization_pending, poll 2 -> slow_down, poll 3 -> ok.
        Mock::given(method("POST"))
            .and(path("/oauth2/token-advertised"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "authorization_pending"
            })))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token-advertised"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "slow_down"
            })))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token-advertised"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "test-xai-access",
                "refresh_token": "test-xai-refresh",
                "expires_in": 3600
            })))
            .expect(1)
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let auth_path = dir.path().join("grok-auth.json");
        let result = tokio::task::block_in_place(|| {
            device_code_login_with(
                &server.uri(),
                GROK_OIDC_CLIENT_ID,
                DEFAULT_SCOPES,
                &auth_path,
                false,
            )
        });

        let credentials = result.expect("device login after pending and slow_down");
        assert_eq!(credentials.access_token, "test-xai-access");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn device_code_login_surfaces_user_denial() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": server.uri(),
                "device_authorization_endpoint": format!("{}/oauth2/device-advertised", server.uri()),
                "token_endpoint": format!("{}/oauth2/token-advertised", server.uri())
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device-advertised"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "device-token",
                "user_code": "CW-TEST",
                "verification_uri": format!("{}/verify", server.uri()),
                "expires_in": 60,
                "interval": 1
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token-advertised"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "access_denied",
                "error_description": "The user denied the authorization request"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let auth_path = dir.path().join("grok-auth.json");
        let result = tokio::task::block_in_place(|| {
            device_code_login_with(
                &server.uri(),
                GROK_OIDC_CLIENT_ID,
                DEFAULT_SCOPES,
                &auth_path,
                false,
            )
        });

        let error = result.expect_err("user denial must stop polling");
        let message = format!("{error:#}");
        assert!(message.contains("access_denied"), "{message}");
        assert!(message.contains("HTTP 400"), "{message}");
        assert!(!message.contains("authorization_pending"), "{message}");
        // The denial must not leave a partial credential on disk.
        assert!(!auth_path.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn poll_device_token_surfaces_expired_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "expired_token",
                "error_description": "The device code has expired"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let error = tokio::task::block_in_place(|| {
            poll_device_token(
                &format!("{}/oauth2/token", server.uri()),
                GROK_OIDC_CLIENT_ID,
                "device-token",
            )
            .expect_err("expired device code must fail")
        });
        let message = error.to_string();

        assert!(message.contains("expired_token"), "{message}");
        assert!(message.contains("HTTP 400"), "{message}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn poll_device_token_reports_non_json_without_raw_parse_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_body_raw("<html>upstream-maintenance-detail</html>", "text/html"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let error = tokio::task::block_in_place(|| {
            poll_device_token(
                &format!("{}/oauth2/token", server.uri()),
                GROK_OIDC_CLIENT_ID,
                "device-token",
            )
            .expect_err("non-JSON response must fail")
        });
        let message = error.to_string();

        assert!(message.contains("HTTP 503"), "{message}");
        assert!(message.contains("text/html"), "{message}");
        assert!(message.contains("expected JSON"), "{message}");
        assert!(
            !message.contains("upstream-maintenance-detail"),
            "{message}"
        );
    }
}
